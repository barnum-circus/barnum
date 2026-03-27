# Engine Design

The engine interprets the Action AST. It has two responsibilities:

1. **Advance** — Walk cursors through the AST until they reach handler invocations. Pure state manipulation, no I/O.
2. **Schedule** — Dispatch pending handler invocations to the executor, subject to concurrency limits.

These two operations alternate: advance produces a queue of dispatches, schedule sends them, handler results arrive as events, advance runs again.

## Cursors and frames

A **cursor** is an active point of execution in the AST — a (position, value) pair where the engine is blocked waiting for a handler result. The engine starts with one cursor at the workflow root with value `null`. As execution proceeds, cursors multiply (Parallel, ForEach) and converge (when all branches complete).

A **frame** is the engine's bookkeeping for one AST node. Frames form a tree mirroring the part of the AST that's currently executing. Leaf frames are Invoke nodes (cursors waiting for handler results). Interior frames are structural combinators (Pipe, Parallel, Loop, etc.) tracking partial progress.

```rust
struct Frame {
    id: FrameId,
    parent: Option<ParentRef>,
    kind: FrameKind,
}

struct ParentRef {
    frame_id: FrameId,
    /// Which child slot this frame occupies (e.g., index in Parallel's results).
    index: usize,
}

enum FrameKind {
    /// Leaf: handler dispatched, waiting for result.
    Invoke { task_id: TaskId },

    /// Sequential composition: executing action at `index`.
    Pipe { action: PipeAction, index: usize },

    /// Parallel fanout: collecting results.
    Parallel { results: Vec<Option<Value>>, remaining: usize },

    /// Parallel map over array input: collecting results.
    ForEach { results: Vec<Option<Value>>, remaining: usize },

    /// Fixed-point iteration.
    Loop { action: LoopAction },

    /// Error materialization.
    Attempt,

    /// Branch or Step delegation. Forwards child result to parent.
    Passthrough,
}
```

## Engine state

```rust
struct Engine {
    config: Config,
    frames: HashMap<FrameId, Frame>,
    task_to_frame: HashMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    next_frame_id: u64,
    next_task_id: u64,
    result: Option<EngineResult>,
}

struct Dispatch {
    task_id: TaskId,
    handler: HandlerKind,
    value: Value,
}

enum EngineResult {
    Success(Value),
    Failure(String),
}
```

No `tx`, no channels. The engine is a pure state machine. Scheduling is a separate concern wired up at the integration layer.

## Operations

### advance(action, value, parent)

Walk the AST from an action with the given input. Recurse until reaching Invoke nodes (which queue dispatches) or combinators that wait for children.

| Action | Behavior |
|--------|----------|
| **Invoke** | Create Invoke frame. Queue dispatch(task_id, handler, value). |
| **Pipe** | Create Pipe frame (index=0). Advance into actions[0] with the value. |
| **Parallel** | Create Parallel frame (remaining=N). Advance into each action with the value. |
| **ForEach** | Value must be an array. Create ForEach frame (remaining=len). Advance body for each element. |
| **Branch** | Read value["kind"]. Look up the case. Create Passthrough frame. Advance into the case. |
| **Loop** | Create Loop frame. Advance into body with the value. |
| **Attempt** | Create Attempt frame. Advance into inner action. |
| **Step(Named)** | Resolve config.steps[name]. Create Passthrough frame. Advance into the resolved action. |
| **Step(Root)** | Create Passthrough frame. Advance into config.workflow. |

### complete(frame_id, value)

A child frame resolved with a value. Advance the parent.

| Parent kind | Behavior |
|-------------|----------|
| **Pipe** | Increment index. If more actions, advance into the next with the value. Otherwise complete the Pipe's parent. |
| **Parallel** | Store value at child index. Decrement remaining. If 0, collect results into an array. Complete parent. |
| **ForEach** | Same as Parallel. |
| **Loop** | If value.kind == "Continue": advance body again with value.value. If "Break": complete parent with value.value. |
| **Attempt** | Wrap as {kind: "Ok", value}. Complete parent. |
| **Passthrough** | Forward value to parent. |

When the root frame completes: `self.result = Some(EngineResult::Success(value))`.

### error(frame_id, error)

A child frame failed. Propagate upward.

| Parent kind | Behavior |
|-------------|----------|
| **Attempt** | Catch. Complete parent with {kind: "Err", error}. |
| **Parallel / ForEach** | Cancel siblings (remove frames, drop their pending dispatches). Propagate error to parent. |
| **Everything else** | Propagate to parent. |

When error reaches root: `self.result = Some(EngineResult::Failure(error))`.

### on_task_completed(task_id, result)

Entry point when a handler result arrives. Finds the Invoke frame, removes it, and calls complete or error on the parent.

```rust
fn on_task_completed(&mut self, task_id: &TaskId, result: &TaskResult) {
    let frame_id = self.task_to_frame.remove(task_id)
        .expect("unknown task_id");
    let frame = self.frames.remove(&frame_id).unwrap();
    match result {
        TaskResult::Success { value } => match frame.parent {
            Some(parent) => self.complete(parent, value.clone()),
            None => self.result = Some(EngineResult::Success(value.clone())),
        },
        TaskResult::Failure { error } => match frame.parent {
            Some(parent) => self.error(parent, error.clone()),
            None => self.result = Some(EngineResult::Failure(error.clone())),
        },
    }
}
```

### start()

```rust
fn start(&mut self) {
    self.advance(&self.config.workflow, Value::Null, None);
}
```

### take_pending_dispatches()

```rust
fn take_pending_dispatches(&mut self) -> Vec<Dispatch> {
    std::mem::take(&mut self.pending_dispatches)
}
```

Returns the queued dispatches. The caller (scheduler/executor) decides when and how to send them.

## Worked examples

### Pipe: `pipe(constant({project: "test"}), setup(), build())`

**start():**
```
advance(Pipe, null, None)
  create F1 (Pipe, index=0)
  advance(Invoke[constant], null, F1/0)
    create F2 (Invoke, t1)
    queue dispatch(t1, constant, null)

pending: [t1]
```

**on_task_completed(t1, {project: "test"}):**
```
remove F2. complete(F1, {project: "test"})
  F1 Pipe: index 0 -> 1
  advance(Invoke[setup], {project: "test"}, F1/0)
    create F3 (Invoke, t2)
    queue dispatch(t2, setup, {project: "test"})

pending: [t2]
```

**on_task_completed(t2, {initialized: true, project: "test"}):**
```
remove F3. complete(F1, {initialized: true, project: "test"})
  F1 Pipe: index 1 -> 2
  advance(Invoke[build], {initialized: true, project: "test"}, F1/0)
    create F4 (Invoke, t3)
    queue dispatch(t3, build, {initialized: true, project: "test"})

pending: [t3]
```

**on_task_completed(t3, {artifact: "test.build"}):**
```
remove F4. complete(F1, {artifact: "test.build"})
  F1 Pipe: index 2 -> 3. 3 == len. Pipe complete.
  F1 is root -> result = Success({artifact: "test.build"})

pending: []
done: true
```

### Parallel: `parallel(verify(), verify())`

Entered with value `{artifact: "a"}`:
```
advance(Parallel, {artifact: "a"}, parent)
  create F1 (Parallel, results=[None, None], remaining=2)
  advance(Invoke[verify], {artifact: "a"}, F1/0)
    create F2 (Invoke, t1)
    queue dispatch(t1, verify, {artifact: "a"})
  advance(Invoke[verify], {artifact: "a"}, F1/1)
    create F3 (Invoke, t2)
    queue dispatch(t2, verify, {artifact: "a"})

pending: [t1, t2]
```

Results arrive in any order:
```
on_task_completed(t1, {verified: true}):
  remove F2. complete(F1/0, {verified: true})
  results[0] = Some. remaining = 1.

on_task_completed(t2, {verified: true}):
  remove F3. complete(F1/1, {verified: true})
  results[1] = Some. remaining = 0.
  F1 resolves: [{verified: true}, {verified: true}]
```

### Loop: `loop(healthCheck())`

Entered with `{deployed: false}`:
```
advance(Loop, {deployed: false}, parent)
  create F1 (Loop)
  advance(Invoke[healthCheck], {deployed: false}, F1/0)
    create F2 (Invoke, t1)

on_task_completed(t1, {kind: "Continue", value: {deployed: false}}):
  remove F2. complete(F1, ...)
  kind = Continue. Re-advance body with {deployed: false}.
  create F3 (Invoke, t2)

on_task_completed(t2, {kind: "Break", value: {stable: true}}):
  remove F3. complete(F1, ...)
  kind = Break. F1 resolves with {stable: true}.
```

### Branch: `branch({HasErrors: fixAll(), Clean: done()})`

Entered with `{kind: "HasErrors", errors: [...]}`:
```
advance(Branch, {kind: "HasErrors", errors: [...]}, parent)
  read kind = "HasErrors"
  create F1 (Passthrough)
  advance(Invoke[fixAll], {kind: "HasErrors", errors: [...]}, F1/0)
    create F2 (Invoke, t1)
```

### Step: `steps.FixCycle`

Entered with some value:
```
advance(Step(Named("FixCycle")), value, parent)
  resolve config.steps["FixCycle"] -> Loop(...)
  create F1 (Passthrough)
  advance(Loop(...), value, F1/0)
    ...
```

### ForEach: `forEach(migrate())`

Entered with `[{file: "a.ts"}, {file: "b.ts"}]`:
```
advance(ForEach, [{file: "a.ts"}, {file: "b.ts"}], parent)
  create F1 (ForEach, results=[None, None], remaining=2)
  advance(Invoke[migrate], {file: "a.ts"}, F1/0)
    create F2 (Invoke, t1)
  advance(Invoke[migrate], {file: "b.ts"}, F1/1)
    create F3 (Invoke, t2)

pending: [t1, t2]
```

## How frames reference the AST

Frames that need to re-enter child actions (Pipe advancing to the next action, Loop re-entering the body) store a cloned copy of the relevant AST node. The AST is config-sized (small), so cloning is cheap and avoids lifetime complexity.

## Testing strategy

The engine is a pure state machine: start() + on_task_completed() produce pending dispatches and eventually a result. Tests construct configs programmatically, call start, assert dispatches, feed fake completions, assert more dispatches, assert the final result.

```rust
#[test]
fn pipe_advances_through_actions() {
    let config = Config {
        workflow: Action::Pipe(PipeAction {
            actions: vec![invoke("setup"), invoke("build")],
        }),
        steps: HashMap::new(),
    };
    let mut engine = Engine::new(config);
    engine.start();

    // After start: first invoke is dispatched
    let dispatches = engine.take_pending_dispatches();
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].handler_name(), "setup");

    // Feed result for setup
    engine.on_task_completed(&dispatches[0].task_id, &success(json!({"ok": true})));

    // Now build is dispatched
    let dispatches = engine.take_pending_dispatches();
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].handler_name(), "build");

    // Feed result for build
    engine.on_task_completed(&dispatches[0].task_id, &success(json!({"artifact": "x"})));

    // Engine is done
    assert!(engine.is_done());
    assert_eq!(engine.result(), Some(&json!({"artifact": "x"})));
}

#[test]
fn parallel_dispatches_all_branches() {
    let config = Config {
        workflow: Action::Parallel(ParallelAction {
            actions: vec![invoke("verify"), invoke("verify")],
        }),
        steps: HashMap::new(),
    };
    let mut engine = Engine::new(config);
    engine.start();

    // Both dispatched at once
    let dispatches = engine.take_pending_dispatches();
    assert_eq!(dispatches.len(), 2);
}

#[test]
fn loop_re_enters_on_continue() {
    let config = Config {
        workflow: Action::Loop(LoopAction {
            body: Box::new(invoke("check")),
        }),
        steps: HashMap::new(),
    };
    let mut engine = Engine::new(config);
    engine.start();

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 1);

    // Continue: re-dispatches
    engine.on_task_completed(&d[0].task_id, &success(json!({
        "kind": "Continue", "value": {"retry": true}
    })));
    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 1);

    // Break: done
    engine.on_task_completed(&d[0].task_id, &success(json!({
        "kind": "Break", "value": {"stable": true}
    })));
    assert!(engine.is_done());
    assert_eq!(engine.result(), Some(&json!({"stable": true})));
}
```

Test helpers: `invoke(name)` builds an `Action::Invoke` with a recognizable handler name. `success(value)` builds a `TaskResult::Success`. These keep tests readable.

Additional test cases:
- `branch_selects_correct_case` — dispatches only the matching case's handler
- `foreach_dispatches_per_element` — N elements = N dispatches
- `step_resolves_named_step` — step reference enters the registered step's action
- `step_resolves_root` — Step(Root) re-enters the workflow
- `attempt_wraps_success` — success becomes {kind: "Ok", value}
- `attempt_catches_failure` — failure becomes {kind: "Err", error}
- `nested_pipe_in_parallel` — pipe inside parallel, both work correctly
- `error_propagates_through_pipe` — failure in pipe step skips remaining steps
- `error_cancels_parallel_siblings` — failure in one parallel branch cancels others

## Integration with the event loop

Deferred. The engine is testable as a standalone state machine. Integration wires it into the Applier trait and connects a handler executor, but the engine's logic is independent of that.

## Open questions

1. **Concurrency limits.** ForEach over N elements dispatches N handlers. Max concurrency should throttle `take_pending_dispatches` or buffer dispatches internally. Deferred — start with unlimited.

2. **Builtin short-circuiting.** constant(), identity(), etc. don't need handler invocation. The engine could resolve them inline during advance instead of dispatching. Deferred — all builtins go through handlers for now.

3. **Task ID generation.** Monotonic counter? UUID? Counter is simpler and sufficient for a single engine instance. UUID is needed if task IDs must be globally unique across runs.
