# Event Loop Restructure

**Blocks:** DEFERRED_RESTART_PERFORM
**Blocked by:** none

## Motivation

The event loop in `barnum_event_loop` processes dispatches in bulk with no per-event structure and no liveness checks. `run_workflow` calls `take_pending_dispatches()`, sends all dispatches to the scheduler at once, then blocks for one completion and calls `complete()` on it unconditionally. This creates a panic when a task is torn down by a restart before its completion arrives (Bug 1 in DEFERRED_RESTART_PERFORM).

The panic happens because the event loop violates an invariant: `complete()` expects every task_id it receives to exist in `task_to_frame`. When `RestartPerform` fires during `complete()` (via Chain trampoline → advance → `bubble_restart_effect`), `teardown_body` removes sibling tasks' `task_to_frame` entries. When those siblings' results arrive from the scheduler, the event loop passes them to `complete()` unconditionally, and `complete()` panics at `expect("unknown task")` (complete.rs:30).

The `expect("unknown task")` is a correct invariant. Callers of `complete()` must only pass live task_ids. The bug is in the event loop: it doesn't check whether a task is still part of a live tree before calling `complete()`. Documented by the `completing_torn_down_task_is_noop` test with `#[should_panic(expected = "unknown task")]` in effects.rs:370.

The fix: restructure the event loop to process events one at a time. Every event is checked for liveness before processing. Stale events are silently dropped. `complete()` is never called with a dead task.

## Design

### Rename `Dispatch` to `DispatchEvent`

The struct is an event consumed by the event loop, not a command. Rename it to match the naming convention of `CompletionEvent` (and the future `PendingRestartEvent` from DEFERRED_RESTART_PERFORM).

```rust
// barnum_engine/src/lib.rs — Dispatch (before, lines 42-50)
pub struct Dispatch {
    pub task_id: TaskId,
    pub handler_id: HandlerId,
    pub value: Value,
}

// barnum_engine/src/lib.rs — DispatchEvent (after)
#[derive(Debug)]
pub struct DispatchEvent {
    pub task_id: TaskId,
    pub handler_id: HandlerId,
    pub value: Value,
}
```

### `CompletionEvent`

Define `CompletionEvent` in `barnum_engine` alongside `DispatchEvent`. This is the event type consumed by `complete()` — a handler result keyed by task.

```rust
// barnum_engine/src/lib.rs — new struct, after DispatchEvent

/// A completed handler result, ready to be delivered to the workflow engine.
#[derive(Debug)]
pub struct CompletionEvent {
    /// The task that completed.
    pub task_id: TaskId,
    /// The handler's return value.
    pub value: Value,
}
```

### `complete()` takes `CompletionEvent`

`complete()` takes the entire `CompletionEvent` instead of separate `task_id` and `value` arguments. The body destructures at the top and is otherwise unchanged. (Delegation methods were already removed — callers use `complete::complete()` directly.)

```rust
// barnum_engine/src/complete.rs — complete (before)
pub fn complete(
    workflow_state: &mut WorkflowState,
    task_id: super::TaskId,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let frame_id = workflow_state
        .task_to_frame
        .remove(&task_id)
        .expect("unknown task");
    // ... rest uses task_id and value
}

// barnum_engine/src/complete.rs — complete (after)
pub fn complete(
    workflow_state: &mut WorkflowState,
    completion_event: super::CompletionEvent,
) -> Result<Option<Value>, CompleteError> {
    let super::CompletionEvent { task_id, value } = completion_event;
    let frame_id = workflow_state
        .task_to_frame
        .remove(&task_id)
        .expect("unknown task");
    // ... rest unchanged
}
```

### Liveness check

Add a method to check whether a task's Invoke frame is still part of the live tree:

```rust
// barnum_engine/src/lib.rs — WorkflowState impl (new method)

/// Returns true if this task's Invoke frame still exists in the tree.
/// Used by the event loop to drop stale events before processing.
pub fn is_task_live(&self, task_id: TaskId) -> bool {
    self.task_to_frame.contains_key(&task_id)
}
```

### One-at-a-time dispatch processing

Change `pending_dispatches` from `Vec<Dispatch>` to `VecDeque<DispatchEvent>`. Replace `take_pending_dispatches()` with `pop_pending_dispatch()` for one-at-a-time consumption. `take_pending_dispatches` is removed entirely — tests pop one at a time.

```rust
// barnum_engine/src/lib.rs — WorkflowState (before)
pub struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,
    task_to_frame: BTreeMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    next_task_id: u32,
}

// barnum_engine/src/lib.rs — WorkflowState (after)
pub struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,
    task_to_frame: BTreeMap<TaskId, FrameId>,
    pending_dispatches: VecDeque<DispatchEvent>,
    next_task_id: u32,
}
```

```rust
// barnum_engine/src/lib.rs — WorkflowState::new (before)
pub fn new(flat_config: FlatConfig) -> Self {
    Self {
        flat_config,
        frames: Arena::new(),
        task_to_frame: BTreeMap::new(),
        pending_dispatches: Vec::new(),
        next_task_id: 0,
    }
}

// barnum_engine/src/lib.rs — WorkflowState::new (after)
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
// barnum_engine/src/lib.rs — methods (before)
pub fn take_pending_dispatches(&mut self) -> Vec<Dispatch> {
    std::mem::take(&mut self.pending_dispatches)
}

// barnum_engine/src/lib.rs — methods (after)

/// Pop the next pending dispatch, or `None` if the queue is empty.
pub fn pop_pending_dispatch(&mut self) -> Option<DispatchEvent> {
    self.pending_dispatches.pop_front()
}
```

```rust
// barnum_engine/src/advance.rs — Invoke arm (before, line 42)
workflow_state.pending_dispatches.push(Dispatch {
    task_id,
    handler_id: handler,
    value,
});

// barnum_engine/src/advance.rs — Invoke arm (after)
workflow_state.pending_dispatches.push_back(DispatchEvent {
    task_id,
    handler_id: handler,
    value,
});
```

### Event struct

A local `Event` struct in `barnum_event_loop` with `task_id` factored out as a top-level field. `EventKind` carries the variant-specific payload — no `task_id` in the variants. This gives uniform `event.task_id` access for the liveness check without a method or per-variant extraction. The deferred restart refactor adds a `Restart` variant to `EventKind`.

```rust
// barnum_event_loop/src/lib.rs

/// An event for the workflow event loop.
/// `task_id` is factored out for uniform liveness checking.
struct Event {
    task_id: TaskId,
    kind: EventKind,
}

/// The payload of a workflow event, without `task_id`.
enum EventKind {
    /// A handler invocation ready to dispatch to a worker.
    Dispatch { handler_id: HandlerId, value: Value },
    /// A worker completed a task.
    Completion { value: Value },
}
```

### Scheduler dispatch signature

```rust
// barnum_event_loop/src/lib.rs — Scheduler::dispatch (before, line 71)
pub fn dispatch(&self, dispatch: &Dispatch, handler: &HandlerKind) {
    let result_tx = self.result_tx.clone();
    let task_id = dispatch.task_id;
    // ... rest unchanged
}

// barnum_event_loop/src/lib.rs — Scheduler::dispatch (after)
pub fn dispatch(&self, dispatch_event: &DispatchEvent, handler: &HandlerKind) {
    let result_tx = self.result_tx.clone();
    let task_id = dispatch_event.task_id;
    // ... rest unchanged
}
```

### Event loop

Each iteration sources the next event (pending dispatch first, scheduler completion when the queue is empty). A single `is_task_live` call on `event.task_id` precedes the `match` — stale events are dropped without entering any branch. `complete()` keeps its `expect("unknown task")` because the liveness check guarantees it is never called with a stale task.

The Dispatch arm reconstructs a `DispatchEvent` for `scheduler.dispatch()` because the scheduler takes `&DispatchEvent`. This is a minor boundary cost of factoring `task_id` out of `EventKind`.

```rust
// barnum_event_loop/src/lib.rs — run_workflow (before)
pub async fn run_workflow(
    workflow_state: &mut WorkflowState,
    scheduler: &mut Scheduler,
) -> Result<Value, RunWorkflowError> {
    let root = workflow_state.workflow_root();
    advance::advance(workflow_state, root, Value::Null, None)
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

        if let Some(terminal_value) = complete::complete(workflow_state, task_id, value)? {
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
    advance::advance(workflow_state, root, Value::Null, None)
        .expect("initial advance failed");

    loop {
        let event = match workflow_state.pop_pending_dispatch() {
            Some(dispatch_event) => Event {
                task_id: dispatch_event.task_id,
                kind: EventKind::Dispatch {
                    handler_id: dispatch_event.handler_id,
                    value: dispatch_event.value,
                },
            },
            None => {
                let (task_id, result) = scheduler
                    .recv()
                    .await
                    .expect("scheduler channel closed unexpectedly");
                Event {
                    task_id,
                    kind: EventKind::Completion { value: result? },
                }
            }
        };

        if !workflow_state.is_task_live(event.task_id) {
            continue;
        }

        match event.kind {
            EventKind::Dispatch { handler_id, value } => {
                let handler = workflow_state.handler(handler_id);
                let dispatch_event = DispatchEvent {
                    task_id: event.task_id,
                    handler_id,
                    value,
                };
                scheduler.dispatch(&dispatch_event, handler);
            }
            EventKind::Completion { value } => {
                let completion_event = CompletionEvent {
                    task_id: event.task_id,
                    value,
                };
                if let Some(terminal_value) =
                    complete::complete(workflow_state, completion_event)?
                {
                    return Ok(terminal_value);
                }
            }
        }
    }
}
```

### Test helpers

> **Known smell / follow-up:** `drive_builtins` is a test-only shortcut that processes builtins synchronously in barnum_engine tests (which have no scheduler). In the real event loop, builtins are dispatched through the scheduler like everything else. A future refactor should eliminate this special-case pattern — builtins should go through the scheduler in tests too.

`drive_builtins` switches to `pop_pending_dispatch` for one-at-a-time processing and checks liveness before each dispatch. When completing a builtin, it constructs a `CompletionEvent` to pass to `complete::complete()`. Stale dispatches (from restarts triggered by earlier builtins in the same queue) are dropped before reaching `complete()`.

```rust
// barnum_engine/src/test_helpers.rs — drive_builtins (before)
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
                    if let Some(value) = complete(engine, dispatch.task_id, result)? {
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

// barnum_engine/src/test_helpers.rs — drive_builtins (after)
pub fn drive_builtins(
    engine: &mut WorkflowState,
) -> Result<(Option<Value>, Vec<DispatchEvent>), CompleteError> {
    let mut ts_dispatches: Vec<DispatchEvent> = Vec::new();
    loop {
        let Some(dispatch_event) = engine.pop_pending_dispatch() else {
            break;
        };
        if !engine.is_task_live(dispatch_event.task_id) {
            continue;
        }
        match engine.handler(dispatch_event.handler_id).clone() {
            HandlerKind::Builtin(builtin_handler) => {
                let result =
                    barnum_builtins::execute_builtin(&builtin_handler.builtin, &dispatch_event.value)
                        .unwrap();
                let completion_event = CompletionEvent {
                    task_id: dispatch_event.task_id,
                    value: result,
                };
                if let Some(value) = complete::complete(engine, completion_event)? {
                    return Ok((Some(value), ts_dispatches));
                }
            }
            HandlerKind::TypeScript(_) => {
                ts_dispatches.push(dispatch_event);
            }
        }
    }
    Ok((None, ts_dispatches))
}
```

`complete_and_drive` takes a `CompletionEvent` and checks liveness before calling `complete::complete()`. Stale task completions return `(None, [])` without touching the engine.

```rust
// barnum_engine/src/test_helpers.rs — complete_and_drive (before)
pub fn complete_and_drive(
    engine: &mut WorkflowState,
    task_id: TaskId,
    value: Value,
) -> Result<(Option<Value>, Vec<Dispatch>), CompleteError> {
    let result = complete(engine, task_id, value)?;
    if result.is_some() {
        let ts = engine.take_pending_dispatches();
        return Ok((result, ts));
    }
    drive_builtins(engine)
}

// barnum_engine/src/test_helpers.rs — complete_and_drive (after)
pub fn complete_and_drive(
    engine: &mut WorkflowState,
    completion_event: CompletionEvent,
) -> Result<(Option<Value>, Vec<DispatchEvent>), CompleteError> {
    if !engine.is_task_live(completion_event.task_id) {
        return Ok((None, Vec::new()));
    }
    let result = complete::complete(engine, completion_event)?;
    if result.is_some() {
        return Ok((result, Vec::new()));
    }
    drive_builtins(engine)
}
```

### `complete()` internals are unchanged

`complete()` keeps its `expect("unknown task")` (complete.rs:30). The only change is the signature: it takes `CompletionEvent` and destructures at the top. The liveness check guarantees the task_id exists in `task_to_frame`, so the `expect` is never hit with a stale task.

## What changes

| Component | Before | After |
|-----------|--------|-------|
| `Dispatch` struct | Named `Dispatch` | Renamed to `DispatchEvent` |
| `CompletionEvent` struct | Does not exist | Defined in `barnum_engine`, consumed by `complete()` |
| `complete()` signature | Takes `(TaskId, Value)` | Takes `CompletionEvent` |
| `complete()` internals | `expect("unknown task")` | Unchanged — invariant preserved by event loop |
| `is_task_live` | Does not exist | Checks `task_to_frame` for the task's Invoke frame |
| `WorkflowState::pending_dispatches` | `Vec<Dispatch>` | `VecDeque<DispatchEvent>` |
| `WorkflowState::new` | `Vec::new()` | `VecDeque::new()` |
| `pop_pending_dispatch` | Does not exist | Returns `Option<DispatchEvent>` from front of queue |
| `take_pending_dispatches` | `std::mem::take` returning `Vec<Dispatch>` | Removed — tests use `pop_pending_dispatch` one at a time |
| `advance` Invoke arm | `.push(Dispatch{...})` | `.push_back(DispatchEvent{...})` |
| `Scheduler::dispatch` | Takes `&Dispatch` | Takes `&DispatchEvent` |
| `Event` / `EventKind` | Does not exist | `Event` struct with `task_id` field + `EventKind` enum (no `task_id` in variants) |
| Event loop | Batch dispatch all, recv one, complete unconditionally | One-at-a-time via `Event` struct; single `is_task_live` check on `event.task_id` before match |
| `drive_builtins` | Batch with `had_builtin` tracking; returns `Vec<Dispatch>` | One-at-a-time; liveness check; constructs `CompletionEvent`; returns `Vec<DispatchEvent>` |
| `complete_and_drive` | Takes `(TaskId, Value)`, calls `complete()` unconditionally, returns `Vec<Dispatch>` | Takes `CompletionEvent`, liveness check before `complete()`, returns `Vec<DispatchEvent>` |

## Tests

### `completing_torn_down_task_is_noop` (effects.rs:370)

Remove `#[should_panic(expected = "unknown task")]`. Change the final assertion to use `complete_and_drive` instead of calling `complete::complete()` directly, so the liveness check drops the stale completion before it reaches `complete()`.

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

    let result = complete(&mut engine, b_task_id, json!("b_out")).unwrap();
    assert_eq!(result, None);
}

// effects.rs — completing_torn_down_task_is_noop (after)
#[test]
fn completing_torn_down_task_is_noop() {
    // ... setup unchanged ...
    let (_, ts) = drive_builtins(&mut engine).unwrap();
    assert_eq!(ts.len(), 2);
    let b_task_id = ts[1].task_id;

    let (result, _) = complete_and_drive(
        &mut engine,
        CompletionEvent { task_id: ts[0].task_id, value: json!("a_out") },
    ).unwrap();
    assert_eq!(result, Some(json!("a_out")));

    // B's task was torn down. Liveness check drops the stale completion.
    let (result, _) = complete_and_drive(
        &mut engine,
        CompletionEvent { task_id: b_task_id, value: json!("b_out") },
    ).unwrap();
    assert_eq!(result, None);
}
```

### `restart_perform_non_terminal_in_all` (effects.rs:401)

Unchanged behavior — stays `#[should_panic(expected = "parent frame exists")]`. That bug requires the deferred restart refactor (advance must be purely additive). The `complete()` call updates to use `CompletionEvent`:

```rust
// effects.rs — restart_perform_non_terminal_in_all (before, line 419)
complete(&mut engine, b_task_id, json!("b_out")).unwrap();

// effects.rs — restart_perform_non_terminal_in_all (after)
complete::complete(
    &mut engine,
    CompletionEvent { task_id: b_task_id, value: json!("b_out") },
).unwrap();
```

### All `complete()` and `complete_and_drive` call sites

Every direct `complete::complete(&mut engine, task_id, value)` call becomes `complete::complete(&mut engine, CompletionEvent { task_id, value })`. Every `complete_and_drive(&mut engine, task_id, value)` call becomes `complete_and_drive(&mut engine, CompletionEvent { task_id, value })`. Mechanical transformation. Affected tests:

**complete.rs:** `chain_trampolines_on_completion`, `nested_chain_completes`, `parallel_collects_results`, `foreach_collects_results`

**effects.rs:** `restart_handle_body_no_perform_exits_normally`, `multi_step_restart_handler_chain`, `resume_handler_does_not_block_sibling_completion`, `bind_single_binding_single_read`, `bind_single_binding_body_ignores_varref`, `bind_inside_foreach`, `bind_read_var_produces_correct_resume`, and all tests using `complete_and_drive`

Pattern:

```rust
// Before
complete::complete(&mut engine, d[0].task_id, json!("a_result"))

// After
complete::complete(&mut engine, CompletionEvent { task_id: d[0].task_id, value: json!("a_result") })
```

```rust
// Before
complete_and_drive(&mut engine, ts[0].task_id, json!("a_out"))

// After
complete_and_drive(&mut engine, CompletionEvent { task_id: ts[0].task_id, value: json!("a_out") })
```

### All `take_pending_dispatches` call sites

`take_pending_dispatches` is removed. Every test that called it switches to `pop_pending_dispatch`. Tests that asserted on the batch size pop each dispatch into a named binding and assert the queue is empty afterward.

Pattern:

```rust
// Before
let d = engine.take_pending_dispatches();
assert_eq!(d.len(), 2);
// ... use d[0], d[1]

// After
let a_dispatch = engine.pop_pending_dispatch().unwrap();
let b_dispatch = engine.pop_pending_dispatch().unwrap();
assert!(engine.pop_pending_dispatch().is_none());
// ... use a_dispatch, b_dispatch
```

Affected tests:

**complete.rs:** `chain_trampolines_on_completion`, `nested_chain_completes`, `parallel_collects_results`, `foreach_collects_results`

**effects.rs:** `multi_step_restart_handler_chain`, `concurrent_resume_performs_not_serialized`

Tests in advance.rs also use `take_pending_dispatches()` and switch to the same pattern. No advance.rs tests call `complete()`.
