# Engine Design

The engine interprets the Action AST, dispatching handler invocations and advancing through the tree as results arrive. It is an `Applier` within the existing event loop.

## Interface

```rust
struct Engine {
    config: Config,
    /// Active execution state — one frame per active point in the AST.
    frames: HashMap<FrameId, Frame>,
    /// Maps in-flight task IDs back to their Invoke frame.
    task_to_frame: HashMap<TaskId, FrameId>,
    /// Handler invocations queued during apply(), sent during flush_pending().
    pending_dispatches: Vec<Dispatch>,
    /// Channel for sending dispatches to the handler executor.
    tx: Sender<Dispatch>,
    /// Set when the root frame resolves.
    result: Option<Value>,
}
```

Three operations:

1. **`start(input: Value)`** — Enter the workflow action with the given input. Creates the initial frame tree and queues any immediately-dispatchable invocations.

2. **`apply(event: &Event)`** — Process a single event. Pure state transition: modifies frames, may queue new dispatches, but performs no I/O.

3. **`flush_pending()`** — Send all queued dispatches through `tx`. Returns whether the workflow is complete.

The event loop calls these in sequence:

```rust
engine.start(Value::Null);
engine.flush_pending();

while let Some(event) = receiver.recv().await {
    for applier in appliers.iter_mut() {
        applier.apply(&event);
    }
    engine.flush_pending();
    if engine.is_done() {
        break;
    }
}
```

(The engine is also in the appliers vec as a `Box<dyn Applier>`, so `apply` is called via the trait. `flush_pending` and `is_done` are called on the concrete type. This means the event loop knows about the engine specifically, which is fine — it's the driver.)

## Dispatch

A dispatch is a request to invoke a handler:

```rust
struct Dispatch {
    task_id: TaskId,
    handler: HandlerKind,
    value: Value,
}
```

The handler executor (outside the engine) receives dispatches, invokes the handler, and sends `TaskCompleted` events back through the event loop's channel. The engine never calls handlers directly.

## Frames

A frame represents the engine's state at one node in the AST. The set of active frames is the set of live execution points — one for sequential positions, many for parallel fanout.

```rust
struct Frame {
    id: FrameId,
    /// None for the root frame.
    parent: Option<ParentRef>,
    kind: FrameKind,
}

/// How a child frame relates to its parent.
struct ParentRef {
    frame_id: FrameId,
    /// Position within the parent (e.g., index in Parallel's results vec).
    index: usize,
}
```

Frame kinds track per-combinator state:

```rust
enum FrameKind {
    /// Leaf: waiting for handler completion.
    Invoke { task_id: TaskId },

    /// Sequential: executing action at `index`.
    Pipe { action: PipeAction, index: usize },

    /// Parallel fanout: collecting results from N branches.
    Parallel { results: Vec<Option<Value>>, remaining: usize },

    /// Parallel map: collecting results from N elements.
    ForEach { results: Vec<Option<Value>>, remaining: usize },

    /// Fixed-point iteration.
    Loop { action: LoopAction },

    /// Error materialization.
    Attempt,

    /// Delegating to a branch case or resolved step. No extra state.
    Passthrough,
}
```

Branch and Step don't need their own variants — once they select which child to enter, they just pass the result through. `Passthrough` covers both.

## Two operations

The engine has two core operations that drive all execution.

### enter(action, value, parent)

Start executing an action with the given input. Recursively descends until it reaches Invoke nodes (which queue dispatches) or structural nodes that must wait for children.

| Action | Behavior |
|--------|----------|
| **Invoke** | Create Invoke frame. Queue dispatch. |
| **Pipe** | Create Pipe frame (index=0). Enter actions[0]. |
| **Parallel** | Create Parallel frame (remaining=N). Enter each action. |
| **ForEach** | Assert value is array. Create ForEach frame (remaining=len). Enter body for each element. |
| **Branch** | Read `value.kind`. Look up case (panic if missing — config is validated). Create Passthrough frame. Enter the case action. |
| **Loop** | Create Loop frame. Enter body. |
| **Attempt** | Create Attempt frame. Enter inner action. |
| **Step(Named)** | Resolve `config.steps[name]` (panic if missing). Create Passthrough frame. Enter resolved action. |
| **Step(Root)** | Create Passthrough frame. Enter `config.workflow`. |

### complete(frame_id, result)

A child has resolved. Advance the parent based on its kind.

| Parent kind | Behavior |
|-------------|----------|
| **Pipe** | Increment index. If more actions remain, enter the next with the result value. Otherwise, complete the Pipe's parent. |
| **Parallel** | Store result at child's index. Decrement remaining. If 0, collect all results into an array and complete the Parallel's parent. |
| **ForEach** | Same as Parallel. |
| **Loop** | If result.kind == "Continue", re-enter the body with result.value. If "Break", complete the Loop's parent with result.value. |
| **Attempt** | Wrap as `{kind: "Ok", value: result}`. Complete parent. |
| **Passthrough** | Forward result to parent unchanged. |

When the root frame (parent=None) completes, set `self.result = Some(value)`.

### error(frame_id, error)

A child has failed. Propagate the error upward.

| Parent kind | Behavior |
|-------------|----------|
| **Attempt** | Catch the error. Complete parent with `{kind: "Err", error}`. |
| **Parallel / ForEach** | Cancel all sibling children (remove their frames, cancel their in-flight tasks). Propagate error to parent. |
| **Everything else** | Propagate error to parent. |

When an error reaches the root frame, set `self.result` to an error outcome.

## apply(event)

```rust
fn apply(&mut self, event: &Event) {
    match event {
        Event::TaskCompleted(tc) => {
            let frame_id = self.task_to_frame.remove(&tc.task_id)
                .expect("unknown task_id");
            self.frames.remove(&frame_id);
            let parent = /* parent of the removed frame */;
            match &tc.result {
                TaskResult::Success { value } => {
                    if let Some(parent) = parent {
                        self.complete(parent.frame_id, value.clone());
                    } else {
                        self.result = Some(value.clone());
                    }
                }
                TaskResult::Failure { error } => {
                    if let Some(parent) = parent {
                        self.error(parent.frame_id, error.clone());
                    } else {
                        // root invoke failed
                        self.result = Some(/* error value */);
                    }
                }
            }
        }
        Event::TaskStarted(_) => {
            // The engine emits these itself; ignore when received.
        }
    }
}
```

## flush_pending()

```rust
fn flush_pending(&mut self) {
    for dispatch in self.pending_dispatches.drain(..) {
        let _ = self.tx.send(dispatch);
    }
}
```

## Worked example

Config: `pipe(constant({project: "test"}), setup(), build())`

**start(null):**

```
enter(Pipe, null)
  → create F1 (Pipe, index=0, action=the pipe)
  → enter(Invoke[constant], null)
    → create F2 (Invoke, task_id=t1, parent=F1/0)
    → queue dispatch(t1, constant, null)

Active: F1(Pipe, idx=0), F2(Invoke, t1)
Pending: [dispatch(t1)]
```

**flush_pending():** sends dispatch t1.

**apply(TaskCompleted(t1, {project: "test"})):**

```
F2 resolves. Remove F2. Notify parent F1(Pipe).
F1: index 0→1. Enter actions[1] = Invoke[setup] with {project: "test"}.
  → create F3 (Invoke, task_id=t2, parent=F1/0)
  → queue dispatch(t2, setup, {project: "test"})

Active: F1(Pipe, idx=1), F3(Invoke, t2)
Pending: [dispatch(t2)]
```

**flush_pending():** sends dispatch t2.

**apply(TaskCompleted(t2, {initialized: true, project: "test"})):**

```
F3 resolves. Remove F3. Notify F1(Pipe).
F1: index 1→2. Enter actions[2] = Invoke[build] with {initialized: true, project: "test"}.
  → create F4 (Invoke, task_id=t3, parent=F1/0)
  → queue dispatch(t3, build, {initialized: true, project: "test"})

Active: F1(Pipe, idx=2), F4(Invoke, t3)
Pending: [dispatch(t3)]
```

**flush_pending():** sends dispatch t3.

**apply(TaskCompleted(t3, {artifact: "test.build"})):**

```
F4 resolves. Remove F4. Notify F1(Pipe).
F1: index 2→3. 3 == actions.len(). Pipe complete.
F1 is root → self.result = Some({artifact: "test.build"}).

Active: (empty)
engine.is_done() → true
```

## Parallel example

Config: `pipe(constant({artifact: "a"}), parallel(verify(), verify()))`

After constant completes with `{artifact: "a"}`:

```
enter(Parallel, {artifact: "a"})
  → create F2 (Parallel, results=[None, None], remaining=2)
  → enter(Invoke[verify], {artifact: "a"})
    → create F3 (Invoke, t2, parent=F2/0)
    → queue dispatch(t2, verify, {artifact: "a"})
  → enter(Invoke[verify], {artifact: "a"})
    → create F4 (Invoke, t3, parent=F2/1)
    → queue dispatch(t3, verify, {artifact: "a"})

Pending: [dispatch(t2), dispatch(t3)]
```

Both dispatches sent. Results arrive in any order:

```
apply(TaskCompleted(t2, {verified: true})):
  F3 resolves. Notify F2. results[0] = {verified: true}. remaining=1.

apply(TaskCompleted(t3, {verified: true})):
  F4 resolves. Notify F2. results[1] = {verified: true}. remaining=0.
  All done. F2 resolves with [{verified: true}, {verified: true}].
```

## Loop example

Config: `loop(healthCheck())`

```
enter(Loop, {deployed: false})
  → create F1 (Loop)
  → enter(Invoke[healthCheck], {deployed: false})
    → create F2 (Invoke, t1, parent=F1/0)

apply(TaskCompleted(t1, {kind: "Continue", value: {deployed: false}})):
  F2 resolves. Notify F1(Loop).
  Result kind = Continue. Re-enter body with {deployed: false}.
  → create F3 (Invoke, t2, parent=F1/0)

apply(TaskCompleted(t2, {kind: "Break", value: {stable: true}})):
  F3 resolves. Notify F1(Loop).
  Result kind = Break. F1 resolves with {stable: true}.
```

## How frames reference the AST

Frames need access to the AST to enter child actions (e.g., Pipe needs `actions[index+1]`, Loop needs its body). Two options:

**Option A: Frames store cloned actions.** Pipe frames clone the PipeAction, Parallel clones the ParallelAction, etc. The AST is small (it's config, not data), so cloning is cheap. Simple ownership, no lifetime issues.

**Option B: Pre-index the AST.** Walk the config on construction, assign each node an `ActionId`, store a flat `Vec<Action>` or `Vec<&Action>`. Frames store ActionIds. More efficient for deep trees, but adds complexity.

For the initial implementation, Option A (clone) is fine. We can optimize later if the AST grows large enough to matter.

## Resumability

The apply/flush split enables replay. To resume from a log:

1. Construct engine with config.
2. Call `start(input)`.
3. Replay each logged event through `apply()` (skip `flush_pending` for events already logged).
4. After replay, call `flush_pending()` to dispatch any work that was pending when the previous run stopped.

This works because `apply` is a deterministic state transition. Replaying the same events reconstructs the same frame tree.

## Open questions

1. **What carries dispatches to the handler executor?** The engine holds a `tx: Sender<Dispatch>`. What receives it? A handler executor task running alongside the event loop? A separate applier? The event loop itself?

2. **Concurrency limits.** ForEach over 10000 elements would create 10000 in-flight tasks. Max concurrency should throttle dispatches in `flush_pending`. Deferred for now — start with unlimited.

3. **Cancellation in Parallel/ForEach.** When one branch errors, sibling in-flight tasks should be cancelled. How? Remove their frames and ignore their TaskCompleted events when they arrive (the task_to_frame lookup will miss). The handler executor should also be told to stop, but that's a handler-executor concern.

4. **Builtin handlers (constant, identity, etc.).** These don't need external invocation. The engine could short-circuit them: instead of dispatching and waiting for TaskCompleted, immediately produce the result inline during `enter`. This keeps the frame tree pure (no Invoke frame for builtins) and avoids unnecessary round-trips. Deferred for now — all builtins go through handlers.

5. **Event granularity.** Should the engine emit events beyond TaskStarted/TaskCompleted? Candidates: FrameEntered, FrameCompleted, WorkflowCompleted, WorkflowFailed. More events = richer logs = easier debugging. But adds noise. Start minimal, add as needed.
