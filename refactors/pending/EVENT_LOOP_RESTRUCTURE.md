# Event Loop Restructure

**Blocks:** DEFERRED_RESTART_PERFORM
**Blocked by:** none

## Motivation

Two changes are needed before the deferred restart refactor can land.

`complete()` panics on stale task_ids. When `RestartPerform` fires during `complete()` (via Chain trampoline → advance → `bubble_restart_effect`), `teardown_body` removes frames and `task_to_frame` entries for in-flight sibling tasks. When those siblings' results arrive from the scheduler, `complete()` panics at `expect("unknown task")` (complete.rs:30). Stale task completions are an inherent consequence of async task execution combined with synchronous teardown. The fix belongs inside `complete()`, not in a caller-side pre-check: `complete()` should return `Ok(None)` for unknown task_ids, the same way `process_restart` returns `Ok(())` when its `RestartHandle` frame no longer exists. Documented by the `completing_torn_down_task_is_noop` test with `#[should_panic(expected = "unknown task")]` in effects.rs:370.

The event loop processes dispatches in bulk with no per-event structure. `run_workflow` calls `take_pending_dispatches()`, sends all dispatches to the scheduler at once, then blocks for one completion. The deferred restart refactor introduces a unified effect queue (dispatches and restarts interleaved) with one-at-a-time processing via an `Event` enum. Restructuring the event loop now means the main refactor only needs to add the `Restart` variant and change the underlying queue type.

## Design

### Fix `complete()` for stale task_ids

When `task_to_frame` has no entry for the given `task_id`, the task was torn down by a restart. Return `Ok(None)`.

```rust
// complete.rs — complete() (before, lines 21-30)
#[allow(clippy::expect_used)]
pub fn complete(
    workflow_state: &mut WorkflowState,
    task_id: super::TaskId,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let frame_id = workflow_state
        .task_to_frame
        .remove(&task_id)
        .expect("unknown task");

// complete.rs — complete() (after)
pub fn complete(
    workflow_state: &mut WorkflowState,
    task_id: super::TaskId,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let Some(frame_id) = workflow_state.task_to_frame.remove(&task_id) else {
        return Ok(None);
    };
```

The `#[allow(clippy::expect_used)]` on `complete()` (line 21) is removed. The remaining `expect` calls inside `deliver` are invariant assertions for frames that must exist (Chain, All/ForEach, ResumeHandle, RestartHandle parents) and are genuine bugs if they fail.

### One-at-a-time dispatch processing

Change `pending_dispatches` from `Vec<Dispatch>` to `VecDeque<Dispatch>`. Add `pop_pending_dispatch()` for one-at-a-time consumption. Keep `take_pending_dispatches()` (reimplemented as drain + collect) for existing engine tests that assert on the full batch.

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

Each iteration sources the next event — pending dispatch first, blocking for a scheduler completion only when the queue is empty — then processes it in a two-branch match. `complete()` handles stale task_ids internally, so the Completion arm has no external liveness check.

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
                let handler = workflow_state.handler(dispatch.handler_id);
                scheduler.dispatch(&dispatch, handler);
            }
            Event::Completion(CompletionEvent { task_id, value }) => {
                if let Some(terminal_value) = workflow_state.complete(task_id, value)? {
                    return Ok(terminal_value);
                }
            }
        }
    }
}
```

### Test helpers

`drive_builtins` switches to `pop_pending_dispatch` for one-at-a-time processing. This eliminates the `had_builtin` tracking from the batch approach: the loop pops until the queue is empty, and builtin completions that produce new dispatches (via Chain trampoline) make them immediately available on the next pop.

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

`complete_and_drive` simplifies: when the workflow terminates, return an empty dispatch vector. No caller uses the dispatches when the result is `Some`.

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
    let result = engine.complete(task_id, value)?;
    if result.is_some() {
        return Ok((result, Vec::new()));
    }
    drive_builtins(engine)
}
```

## What changes

| Component | Before | After |
|-----------|--------|-------|
| `complete()` | Panics on unknown `task_id` | Returns `Ok(None)` for stale tasks |
| `WorkflowState::pending_dispatches` | `Vec<Dispatch>` | `VecDeque<Dispatch>` |
| `WorkflowState::new` | `Vec::new()` | `VecDeque::new()` |
| `pop_pending_dispatch` | Does not exist | Returns `Option<Dispatch>` from front of queue |
| `take_pending_dispatches` | `std::mem::take` | `drain(..).collect()` (retained for tests) |
| `advance` Invoke arm | `.push(...)` | `.push_back(...)` |
| Event loop | Batch dispatch all, recv one, complete | One-at-a-time via `Event` enum, two-branch match |
| `drive_builtins` | Batch with `had_builtin` tracking | One-at-a-time via `pop_pending_dispatch` |
| `complete_and_drive` | Drains remaining dispatches on termination | Returns empty vec on termination |

## Tests

`completing_torn_down_task_is_noop` (effects.rs:370): remove `#[should_panic(expected = "unknown task")]`. The test passes: `complete(b_task_id, ...)` returns `Ok(None)`.

`restart_perform_non_terminal_in_all` (effects.rs:401): unchanged, stays `#[should_panic(expected = "parent frame exists")]`. That bug requires the deferred restart refactor (advance must be purely additive).

All other existing tests pass unchanged. Tests in advance.rs and complete.rs continue using `take_pending_dispatches()`.
