# Completion and Error Handling

Implementation plan for the second engine milestone: task correlation, completion, error propagation, and terminal results.

**Depends on:** ENGINE.md (design), FRAME_STORAGE_AND_ADVANCE.md (first milestone — frame storage, advance, pending dispatches)

**Scope:** `TaskId`, `task_to_frame`, `on_task_completed`, `complete` (promoted to public-facing), `error`, `EngineResult`, `is_done()`, `result()`. This milestone takes the engine from "expand and dispatch" to "full advance/complete cycle."

## What the first milestone left out

The advance milestone produces dispatches but has no way to consume results. Dispatches go out to the runtime but nothing comes back. The engine is a one-shot expansion machine.

This milestone closes the loop:

```
Dispatch goes out → runtime executes handler → result comes back
  → on_task_completed(task_id, result)
    → finds the Invoke frame
    → calls complete(parent, value) or error(parent, error_string)
      → parent frame decides what to do
        → may call advance() again (Chain trampoline, Loop re-enter)
          → produces more dispatches
            → cycle continues until Root completes or errors
```

## New types

### TaskId

```rust
u32_newtype!(
    /// Identifies a pending handler invocation. Assigned by the engine,
    /// returned to the engine in `on_task_completed`.
    pub TaskId
);
```

Monotonic `u32` counter. Assigned during advance when an Invoke frame is created. Returned in `Dispatch` so the runtime can correlate results. Used as a `HashMap` key in `task_to_frame`.

### TaskResult

```rust
pub enum TaskResult {
    Success { value: Value },
    Failure { error: String },
}
```

What the runtime sends back. Success carries the handler's output value. Failure carries an error message (untyped string for now — typed errors are in DEFERRED_FEATURES.md).

### EngineResult

```rust
#[derive(Debug)]
pub enum EngineResult {
    Success(Value),
    Failure(String),
}
```

Terminal state. Set when Root is completed or when an unhandled error reaches Root.

### Updated Dispatch

```rust
#[derive(Debug)]
pub struct Dispatch {
    pub task_id: TaskId,
    pub handler_id: HandlerId,
    pub value: Value,
}
```

Now includes `task_id` so the runtime can send results back keyed by task.

### Updated FrameKind::Invoke

```rust
FrameKind::Invoke { task_id: TaskId },
```

The Invoke frame now stores its TaskId. When `on_task_completed` arrives, the engine looks up the frame by TaskId, extracts the parent, removes the frame, and calls complete or error.

### Updated Engine

```rust
pub struct Engine {
    flat_config: FlatConfig,
    frames: Slab<Frame>,
    task_to_frame: HashMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    next_task_id: u32,
    result: Option<EngineResult>,
}
```

Three new fields over the advance milestone:
- `task_to_frame`: maps pending TaskIds to Invoke FrameIds. Populated in advance (Invoke arm), consumed in on_task_completed.
- `next_task_id`: monotonic counter for TaskId allocation.
- `result`: terminal state, set by Root completion or unhandled error.

## Updated advance (Invoke arm only)

The only change to advance: the Invoke arm now allocates a TaskId, stores the mapping, and includes the TaskId in the dispatch.

```rust
FlatAction::Invoke { handler } => {
    let task_id = self.next_task_id();
    let frame_id = self.insert_frame(Frame {
        parent: Some(parent),
        kind: FrameKind::Invoke { task_id },
    });
    self.task_to_frame.insert(task_id, frame_id);
    self.pending_dispatches.push(Dispatch {
        task_id,
        handler_id: handler,
        value,
    });
}
```

## complete (promoted from private)

In the advance milestone, complete existed as a private dependency for empty ForEach/Parallel. Now it's the core of the engine's execution model.

No code changes to complete itself — the implementation from the advance milestone is already complete. The only change is that Root now stores the result:

```rust
FrameKind::Root => {
    self.frames.remove(frame_id.0);
    self.result = Some(EngineResult::Success(value));
}
```

(The advance milestone's Root arm just removed the frame without storing anything.)

## error

A child failed. Walk up the frame tree until an Attempt catches the error or Root terminates.

```rust
fn error(&mut self, parent_ref: ParentRef, error: String) {
    let frame_id = parent_ref.frame_id();
    let frame = self.frames.remove(frame_id.0);

    match frame.kind {
        FrameKind::Root => {
            self.result = Some(EngineResult::Failure(error));
        }

        FrameKind::Attempt => {
            let parent = frame.parent.expect("non-root frame has parent");
            let wrapped = serde_json::json!({ "kind": "Err", "error": error });
            self.complete(parent, wrapped);
        }

        FrameKind::Parallel { .. } | FrameKind::ForEach { .. } => {
            let parent = frame.parent.expect("non-root frame has parent");
            self.cancel_descendants(frame_id);
            self.error(parent, error);
        }

        _ => {
            let parent = frame.parent.expect("non-root frame has parent");
            self.error(parent, error);
        }
    }
}
```

### Error propagation path

Error walks up frame-by-frame:
1. **Root**: terminal failure. Engine is done.
2. **Attempt**: catches the error. Wraps as `{ kind: "Err", error }` and completes the parent normally. Error stops propagating.
3. **Parallel/ForEach**: cancel all other in-flight children (they're now irrelevant), then propagate the error upward. Short-circuit — one failure fails the whole fan-out.
4. **Chain/Loop/Invoke**: transparent — just propagate upward.

### cancel_descendants

When a Parallel or ForEach frame errors, its surviving children (other Invoke frames still pending) must be cancelled. The engine walks the subtree rooted at the fan-out frame and:
- Removes all descendant frames from the slab
- Removes all descendant TaskIds from `task_to_frame`
- Does NOT remove dispatches already in `pending_dispatches` — those are already queued for the runtime. The runtime will send results for cancelled tasks; `on_task_completed` must handle "unknown task" gracefully (the frame was already removed).

```rust
fn cancel_descendants(&mut self, _frame_id: FrameId) {
    // Walk all frames in the slab, find those whose ancestor chain
    // includes frame_id, remove them and their task_to_frame entries.
    //
    // Simple approach: iterate slab, check parent chains.
    // O(n) where n = number of frames. Fine for now.
    //
    // Optimization: maintain a children list per frame for O(subtree) traversal.
    // Not needed until profiling shows it matters.
    let to_remove: Vec<usize> = self.frames.iter()
        .filter(|(_, frame)| self.is_descendant_of(frame, _frame_id))
        .map(|(key, _)| key)
        .collect();

    for key in to_remove {
        let frame = self.frames.remove(key);
        if let FrameKind::Invoke { task_id } = frame.kind {
            self.task_to_frame.remove(&task_id);
        }
    }
}
```

**Open question:** Should `on_task_completed` panic or silently ignore unknown TaskIds? If cancelled tasks' results arrive after cancellation, the TaskId won't be in `task_to_frame`. Panicking is correct for debugging (unknown TaskId = bug); ignoring is correct for production (cancelled tasks' results are expected noise). Start with panic, add a flag or match later.

## on_task_completed

The entry point from the runtime. A handler finished; deliver the result to the engine.

```rust
pub fn on_task_completed(&mut self, task_id: TaskId, task_result: TaskResult) {
    let frame_id = self.task_to_frame.remove(&task_id).expect("unknown task");
    let frame = self.frames.remove(frame_id.0);
    let parent = frame.parent.expect("Invoke frame has parent");
    match task_result {
        TaskResult::Success { value } => self.complete(parent, value),
        TaskResult::Failure { error } => self.error(parent, error),
    }
}
```

Remove the Invoke frame and its task mapping. Extract the parent. Dispatch to complete or error based on the result.

## Updated public API

```rust
impl Engine {
    pub fn new(flat_config: FlatConfig) -> Self;
    pub fn start(&mut self, input: Value);
    pub fn on_task_completed(&mut self, task_id: TaskId, task_result: TaskResult);
    pub fn take_pending_dispatches(&mut self) -> Vec<Dispatch>;
    pub fn handler(&self, id: HandlerId) -> &HandlerKind;
    pub fn is_done(&self) -> bool;
    pub fn result(&self) -> Option<&EngineResult>;
}
```

Three new methods over the advance milestone:
- `on_task_completed` — deliver a handler result
- `is_done` — check terminal state
- `result` — read terminal value

## Tests

Tests use the full cycle: build config → flatten → Engine::new → start → take dispatches → on_task_completed → take more dispatches → ... → assert result.

Helper:
```rust
fn success(value: Value) -> TaskResult {
    TaskResult::Success { value }
}

fn failure(error: &str) -> TaskResult {
    TaskResult::Failure { error: error.to_string() }
}
```

### Completion tests

```rust
/// Chain(A, B): complete A → dispatches B. Complete B → engine done.
#[test]
fn chain_trampolines_on_completion() {
    let mut engine = engine_from(chain(
        invoke("./a.ts", "a"),
        invoke("./b.ts", "b"),
    ));
    engine.start(json!(null));

    let d1 = engine.take_pending_dispatches();
    assert_eq!(d1.len(), 1); // A dispatched

    engine.on_task_completed(d1[0].task_id, success(json!("a_result")));

    let d2 = engine.take_pending_dispatches();
    assert_eq!(d2.len(), 1); // B dispatched
    assert_eq!(d2[0].value, json!("a_result")); // B receives A's output

    engine.on_task_completed(d2[0].task_id, success(json!("b_result")));
    assert!(engine.is_done());
    assert!(matches!(engine.result(), Some(EngineResult::Success(v)) if *v == json!("b_result")));
}

/// Deep chain: Chain(A, Chain(B, C)) → A → B → C → done.
#[test]
fn nested_chain_completes() {
    let mut engine = engine_from(chain(
        invoke("./a.ts", "a"),
        chain(invoke("./b.ts", "b"), invoke("./c.ts", "c")),
    ));
    engine.start(json!("input"));

    // A
    let d = engine.take_pending_dispatches();
    engine.on_task_completed(d[0].task_id, success(json!("a_out")));
    // B
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!("a_out"));
    engine.on_task_completed(d[0].task_id, success(json!("b_out")));
    // C
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!("b_out"));
    engine.on_task_completed(d[0].task_id, success(json!("c_out")));

    assert!(matches!(engine.result(), Some(EngineResult::Success(v)) if *v == json!("c_out")));
}

/// Parallel(A, B): complete both → engine done with [a_result, b_result].
#[test]
fn parallel_collects_results() {
    let mut engine = engine_from(parallel(vec![
        invoke("./a.ts", "a"),
        invoke("./b.ts", "b"),
    ]));
    engine.start(json!(null));

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 2);

    // Complete in reverse order to verify index-based collection.
    engine.on_task_completed(d[1].task_id, success(json!("b_result")));
    assert!(!engine.is_done()); // Still waiting for A

    engine.on_task_completed(d[0].task_id, success(json!("a_result")));
    assert!(engine.is_done());
    assert!(matches!(
        engine.result(),
        Some(EngineResult::Success(v)) if *v == json!(["a_result", "b_result"])
    ));
}

/// ForEach over [10, 20]: complete both → [handler(10), handler(20)].
#[test]
fn foreach_collects_results() {
    let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
    engine.start(json!([10, 20]));

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 2);

    engine.on_task_completed(d[0].task_id, success(json!("r10")));
    engine.on_task_completed(d[1].task_id, success(json!("r20")));

    assert!(matches!(
        engine.result(),
        Some(EngineResult::Success(v)) if *v == json!(["r10", "r20"])
    ));
}

/// Loop: Continue re-dispatches, Break completes.
#[test]
fn loop_continue_and_break() {
    let mut engine = engine_from(loop_action(invoke("./handler.ts", "run")));
    engine.start(json!(0));

    // Iteration 1: handler returns Continue
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!(0));
    engine.on_task_completed(d[0].task_id, success(json!({"kind": "Continue", "value": 1})));

    // Iteration 2: handler returns Continue again
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!(1));
    engine.on_task_completed(d[0].task_id, success(json!({"kind": "Continue", "value": 2})));

    // Iteration 3: handler returns Break
    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!(2));
    engine.on_task_completed(d[0].task_id, success(json!({"kind": "Break", "value": "done"})));

    assert!(matches!(
        engine.result(),
        Some(EngineResult::Success(v)) if *v == json!("done")
    ));
}

/// Attempt wraps success in Ok.
#[test]
fn attempt_wraps_success() {
    let mut engine = engine_from(attempt(invoke("./handler.ts", "run")));
    engine.start(json!("input"));

    let d = engine.take_pending_dispatches();
    engine.on_task_completed(d[0].task_id, success(json!("output")));

    assert!(matches!(
        engine.result(),
        Some(EngineResult::Success(v)) if *v == json!({"kind": "Ok", "value": "output"})
    ));
}

/// Attempt catches failure as Err.
#[test]
fn attempt_catches_failure() {
    let mut engine = engine_from(attempt(invoke("./handler.ts", "run")));
    engine.start(json!("input"));

    let d = engine.take_pending_dispatches();
    engine.on_task_completed(d[0].task_id, failure("handler crashed"));

    assert!(matches!(
        engine.result(),
        Some(EngineResult::Success(v)) if *v == json!({"kind": "Err", "error": "handler crashed"})
    ));
    // Note: Success, not Failure — Attempt caught the error.
}

/// Error propagates through Chain to Root.
#[test]
fn error_propagates_through_chain() {
    let mut engine = engine_from(chain(
        invoke("./a.ts", "a"),
        invoke("./b.ts", "b"),
    ));
    engine.start(json!(null));

    let d = engine.take_pending_dispatches();
    engine.on_task_completed(d[0].task_id, failure("a failed"));

    assert!(matches!(
        engine.result(),
        Some(EngineResult::Failure(e)) if e == "a failed"
    ));
}

/// Error in one Parallel child fails the whole Parallel.
#[test]
fn error_in_parallel_child() {
    let mut engine = engine_from(parallel(vec![
        invoke("./a.ts", "a"),
        invoke("./b.ts", "b"),
    ]));
    engine.start(json!(null));

    let d = engine.take_pending_dispatches();
    engine.on_task_completed(d[0].task_id, failure("a failed"));

    assert!(matches!(
        engine.result(),
        Some(EngineResult::Failure(e)) if e == "a failed"
    ));
}
```

### Not tested here

- cancel_descendants correctness (requires inspecting internal frame state)
- Cancelled task results arriving after cancellation (on_task_completed with unknown TaskId)
- Deeply nested error propagation (Attempt inside Parallel inside Chain)
- Step(Root) re-entry patterns
