# Event Loop Restructure

**Blocks:** DEFERRED_RESTART_PERFORM
**Blocked by:** none

## Motivation

The event loop in `barnum_event_loop` processes dispatches in bulk with no per-event structure and no liveness checks. `run_workflow` calls `take_pending_dispatches()`, sends all dispatches to the scheduler at once, then blocks for one completion and calls `complete()` on it unconditionally. This creates a panic when a task is torn down by a restart before its completion arrives (Bug 1 in DEFERRED_RESTART_PERFORM).

The panic happens because the event loop violates an invariant: `complete()` expects every task_id it receives to exist in `task_to_frame`. When `RestartPerform` fires during `complete()` (via Chain trampoline → advance → `bubble_restart_effect`), `teardown_body` removes sibling tasks' `task_to_frame` entries. When those siblings' results arrive from the scheduler, the event loop passes them to `complete()` unconditionally, and `complete()` panics at `expect("unknown task")` (complete.rs:30).

The `expect("unknown task")` is a correct invariant. Callers of `complete()` must only pass live task_ids. The bug is in the event loop: it doesn't check whether a task is still part of a live tree before calling `complete()`. Documented by the `completing_torn_down_task_is_noop` test with `#[should_panic(expected = "unknown task")]` in effects.rs:370.

The fix: restructure the event loop to process events one at a time via an `Event` enum. Every event is checked for liveness before processing. Stale events are silently dropped. `complete()` is never called with a dead task.

## Design

### Liveness check

Add a method to check whether a task's Invoke frame is still part of the live tree:

```rust
// lib.rs — WorkflowState impl (new method)

/// Returns true if this task's Invoke frame still exists in the tree.
/// Used by the event loop to drop stale events before processing.
pub fn is_task_live(&self, task_id: TaskId) -> bool {
    self.task_to_frame.contains_key(&task_id)
}
```

### One-at-a-time dispatch processing

Change `pending_dispatches` from `Vec<Dispatch>` to `VecDeque<Dispatch>`. Add `pop_pending_dispatch()` for one-at-a-time consumption. Keep `take_pending_dispatches()` (reimplemented as drain + collect) for engine tests that assert on the full batch.

```rust
// lib.rs — WorkflowState (before, lines 125-131)
pub struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,
    task_to_frame: BTreeMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    next_task_id: u32,
}

// lib.rs — WorkflowState (after)
pub struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,
    task_to_frame: BTreeMap<TaskId, FrameId>,
    pending_dispatches: VecDeque<Dispatch>,
    next_task_id: u32,
}
```

```rust
// lib.rs — WorkflowState::new (before, lines 137-145)
pub fn new(flat_config: FlatConfig) -> Self {
    Self {
        flat_config,
        frames: Arena::new(),
        task_to_frame: BTreeMap::new(),
        pending_dispatches: Vec::new(),
        next_task_id: 0,
    }
}

// lib.rs — WorkflowState::new (after)
pub fn new(flat_config: FlatConfig) -> Self {
    Self {
        flat_config,
        frames: Arena::new(),
        task_to_frame: BTreeMap::new(),
        pending_dispatches: VecDeque::new(),
        next_task_id: 0,
    }
}
```

```rust
// lib.rs — methods (before, lines 155-157)
pub fn take_pending_dispatches(&mut self) -> Vec<Dispatch> {
    std::mem::take(&mut self.pending_dispatches)
}

// lib.rs — methods (after)

/// Pop the next pending dispatch, or `None` if the queue is empty.
pub fn pop_pending_dispatch(&mut self) -> Option<Dispatch> {
    self.pending_dispatches.pop_front()
}

/// Drain all pending dispatches into a `Vec`. Used by engine tests
/// that assert on the full batch.
pub fn take_pending_dispatches(&mut self) -> Vec<Dispatch> {
    self.pending_dispatches.drain(..).collect()
}
```

```rust
// advance.rs — Invoke arm (before, line 42)
workflow_state.pending_dispatches.push(Dispatch {
    task_id,
    handler_id: handler,
    value,
});

// advance.rs — Invoke arm (after)
workflow_state.pending_dispatches.push_back(Dispatch {
    task_id,
    handler_id: handler,
    value,
});
```

### Event enum

A local `Event` enum in `barnum_event_loop` with two variants. The deferred restart refactor adds the `Restart` variant.

```rust
// barnum_event_loop/src/lib.rs

/// A completed handler result from the scheduler.
struct CompletionEvent {
    task_id: TaskId,
    value: Value,
}

/// Events processed by the workflow event loop.
enum Event {
    /// A handler invocation ready to dispatch to a worker.
    Dispatch(Dispatch),
    /// A worker completed a task.
    Completion(CompletionEvent),
}
```

### Event loop

Each iteration sources the next event (pending dispatch first, scheduler completion when the queue is empty), then checks liveness, then processes it. Every branch checks liveness before doing work. `complete()` keeps its `expect("unknown task")` — the liveness check guarantees it is never called with a stale task.

```rust
// barnum_event_loop/src/lib.rs — run_workflow (before, lines 141-168)
pub async fn run_workflow(
    workflow_state: &mut WorkflowState,
    scheduler: &mut Scheduler,
) -> Result<Value, RunWorkflowError> {
    let root = workflow_state.workflow_root();
    workflow_state
        .advance(root, Value::Null, None)
        .expect("initial advance failed");

    loop {
        let dispatches = workflow_state.take_pending_dispatches();
        for dispatch in &dispatches {
            let handler = workflow_state.handler(dispatch.handler_id);
            scheduler.dispatch(dispatch, handler);
        }

        let (task_id, result) = scheduler
            .recv()
            .await
            .expect("scheduler channel closed unexpectedly");

        let value = result?;

        if let Some(terminal_value) = workflow_state.complete(task_id, value)? {
            return Ok(terminal_value);
        }
    }
}

// barnum_event_loop/src/lib.rs — run_workflow (after)
pub async fn run_workflow(
    workflow_state: &mut WorkflowState,
    scheduler: &mut Scheduler,
) -> Result<Value, RunWorkflowError> {
    let root = workflow_state.workflow_root();
    workflow_state
        .advance(root, Value::Null, None)
        .expect("initial advance failed");

    loop {
        let event = match workflow_state.pop_pending_dispatch() {
            Some(dispatch) => Event::Dispatch(dispatch),
            None => {
                let (task_id, result) = scheduler
                    .recv()
                    .await
                    .expect("scheduler channel closed unexpectedly");
                Event::Completion(CompletionEvent {
                    task_id,
                    value: result?,
                })
            }
        };

        match event {
            Event::Dispatch(dispatch) => {
                if !workflow_state.is_task_live(dispatch.task_id) {
                    continue;
                }
                let handler = workflow_state.handler(dispatch.handler_id);
                scheduler.dispatch(&dispatch, handler);
            }
            Event::Completion(CompletionEvent { task_id, value }) => {
                if !workflow_state.is_task_live(task_id) {
                    continue;
                }
                if let Some(terminal_value) = workflow_state.complete(task_id, value)? {
                    return Ok(terminal_value);
                }
            }
        }
    }
}
```

### Test helpers

`drive_builtins` switches to `pop_pending_dispatch` for one-at-a-time processing and checks liveness before each dispatch. This eliminates the `had_builtin` tracking from the batch approach and mirrors the event loop's liveness check pattern. Stale dispatches (from restarts triggered by earlier builtins in the same queue) are dropped before reaching `complete()`.

```rust
// test_helpers.rs — drive_builtins (before, lines 177-208)
pub fn drive_builtins(
    engine: &mut WorkflowState,
) -> Result<(Option<Value>, Vec<Dispatch>), CompleteError> {
    let mut ts_dispatches: Vec<Dispatch> = Vec::new();
    loop {
        let dispatches = engine.take_pending_dispatches();
        if dispatches.is_empty() {
            break;
        }
        let mut had_builtin = false;
        for dispatch in dispatches {
            match engine.handler(dispatch.handler_id).clone() {
                HandlerKind::Builtin(builtin_handler) => {
                    let result =
                        barnum_builtins::execute_builtin(&builtin_handler.builtin, &dispatch.value)
                            .unwrap();
                    if let Some(value) = engine.complete(dispatch.task_id, result)? {
                        return Ok((Some(value), ts_dispatches));
                    }
                    had_builtin = true;
                }
                HandlerKind::TypeScript(_) => {
                    ts_dispatches.push(dispatch);
                }
            }
        }
        if !had_builtin {
            break;
        }
    }
    Ok((None, ts_dispatches))
}

// test_helpers.rs — drive_builtins (after)
pub fn drive_builtins(
    engine: &mut WorkflowState,
) -> Result<(Option<Value>, Vec<Dispatch>), CompleteError> {
    let mut ts_dispatches: Vec<Dispatch> = Vec::new();
    loop {
        let Some(dispatch) = engine.pop_pending_dispatch() else {
            break;
        };
        if !engine.is_task_live(dispatch.task_id) {
            continue;
        }
        match engine.handler(dispatch.handler_id).clone() {
            HandlerKind::Builtin(builtin_handler) => {
                let result =
                    barnum_builtins::execute_builtin(&builtin_handler.builtin, &dispatch.value)
                        .unwrap();
                if let Some(value) = engine.complete(dispatch.task_id, result)? {
                    return Ok((Some(value), ts_dispatches));
                }
            }
            HandlerKind::TypeScript(_) => {
                ts_dispatches.push(dispatch);
            }
        }
    }
    Ok((None, ts_dispatches))
}
```

`complete_and_drive` checks liveness before calling `complete()`. Stale task completions return `(None, [])` without touching the engine. When the workflow terminates, return an empty dispatch vector (no caller uses dispatches after termination).

```rust
// test_helpers.rs — complete_and_drive (before, lines 212-223)
pub fn complete_and_drive(
    engine: &mut WorkflowState,
    task_id: TaskId,
    value: Value,
) -> Result<(Option<Value>, Vec<Dispatch>), CompleteError> {
    let result = engine.complete(task_id, value)?;
    if result.is_some() {
        let ts = engine.take_pending_dispatches();
        return Ok((result, ts));
    }
    drive_builtins(engine)
}

// test_helpers.rs — complete_and_drive (after)
pub fn complete_and_drive(
    engine: &mut WorkflowState,
    task_id: TaskId,
    value: Value,
) -> Result<(Option<Value>, Vec<Dispatch>), CompleteError> {
    if !engine.is_task_live(task_id) {
        return Ok((None, Vec::new()));
    }
    let result = engine.complete(task_id, value)?;
    if result.is_some() {
        return Ok((result, Vec::new()));
    }
    drive_builtins(engine)
}
```

### `complete()` is unchanged

`complete()` keeps its `expect("unknown task")` (complete.rs:30). This is a correct invariant: every task_id passed to `complete()` must exist in `task_to_frame`. The event loop and test helpers enforce this by checking liveness before calling `complete()`.

## What changes

| Component | Before | After |
|-----------|--------|-------|
| `is_task_live` | Does not exist | Checks `task_to_frame` for the task's Invoke frame |
| `complete()` | `expect("unknown task")` | Unchanged — invariant preserved by event loop |
| `WorkflowState::pending_dispatches` | `Vec<Dispatch>` | `VecDeque<Dispatch>` |
| `WorkflowState::new` | `Vec::new()` | `VecDeque::new()` |
| `pop_pending_dispatch` | Does not exist | Returns `Option<Dispatch>` from front of queue |
| `take_pending_dispatches` | `std::mem::take` | `drain(..).collect()` (retained for engine tests) |
| `advance` Invoke arm | `.push(...)` | `.push_back(...)` |
| Event loop | Batch dispatch all, recv one, complete unconditionally | One-at-a-time via `Event` enum; liveness check before every event |
| `drive_builtins` | Batch with `had_builtin` tracking | One-at-a-time; liveness check before each dispatch |
| `complete_and_drive` | Calls `complete()` unconditionally | Liveness check before `complete()`; returns `(None, [])` for stale tasks |

## Tests

`completing_torn_down_task_is_noop` (effects.rs:370): remove `#[should_panic(expected = "unknown task")]`. Change the final assertion to use `complete_and_drive` instead of calling `engine.complete()` directly, so the liveness check drops the stale completion before it reaches `complete()`.

```rust
// effects.rs — completing_torn_down_task_is_noop (before)
#[test]
#[should_panic(expected = "unknown task")]
fn completing_torn_down_task_is_noop() {
    // ... setup: restart_branch with All(Chain(A, break_restart_perform), B) ...
    let (_, ts) = drive_builtins(&mut engine).unwrap();
    assert_eq!(ts.len(), 2);
    let b_task_id = ts[1].task_id;

    let (result, _) = complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
    assert_eq!(result, Some(json!("a_out")));

    let result = engine.complete(b_task_id, json!("b_out")).unwrap();
    assert_eq!(result, None);
}

// effects.rs — completing_torn_down_task_is_noop (after)
#[test]
fn completing_torn_down_task_is_noop() {
    // ... setup unchanged ...
    let (_, ts) = drive_builtins(&mut engine).unwrap();
    assert_eq!(ts.len(), 2);
    let b_task_id = ts[1].task_id;

    let (result, _) = complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
    assert_eq!(result, Some(json!("a_out")));

    // B's task was torn down. Liveness check drops the stale completion.
    let (result, _) = complete_and_drive(&mut engine, b_task_id, json!("b_out")).unwrap();
    assert_eq!(result, None);
}
```

`restart_perform_non_terminal_in_all` (effects.rs:401): unchanged, stays `#[should_panic(expected = "parent frame exists")]`. That bug requires the deferred restart refactor (advance must be purely additive).

All other existing tests pass unchanged. Tests in advance.rs and complete.rs continue using `take_pending_dispatches()`.
