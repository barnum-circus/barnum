# Deferred RestartPerform

**Blocked by:** EVENT_LOOP_RESTRUCTURE (done)

## Motivation

`RestartPerform` currently executes synchronously during `advance`: it tears down the body, advances the handler, and (if the handler is a builtin) restarts the body, all within the same call stack. This creates two bugs and a structural problem.

### Bug 1: Stale task completion panic (fixed by EVENT_LOOP_RESTRUCTURE)

When `RestartPerform` fires (during `complete` → deliver → Chain trampoline → advance), `teardown_body` removes frames and `task_to_frame` entries for in-flight sibling tasks. When those tasks complete later, `complete()` panics on `expect("unknown task")`. EVENT_LOOP_RESTRUCTURE fixes this by adding an `is_task_live()` check in the event loop and test helpers so `complete()` is never called with unknown task_ids. The `expect("unknown task")` invariant is preserved.

### Bug 2: Iterator invalidation in All/ForEach

If `RestartPerform` fires as a child of All during the All advance loop, `teardown_body` removes the All frame mid-iteration. Subsequent siblings create frames pointing to the removed All frame. Documented by the `restart_perform_non_terminal_in_all` test with `#[should_panic(expected = "parent frame exists")]`.

Current combinators happen to avoid this because `RestartPerform` is always behind `Chain(Tag("Break"), RestartPerform(...))`, and Tag is a builtin Invoke that goes through the dispatch cycle. But this is a combinator implementation detail, not a structural guarantee. Future combinators or inline builtin optimization would break it.

### Structural problem: advance has destructive side effects

`advance` both creates frames (additive) and tears them down (destructive). When a RestartPerform fires inside an All child that contains a RestartHandle, the All loop must reason about which sibling frames survived the teardown and which didn't. Getting this reasoning right for all cases is error-prone.

The fix: make `advance` purely additive. `RestartPerform` enqueues a pending effect instead of executing it. The event loop handles teardown and dispatch uniformly.

## Design

### Invariant: every advance completes entirely

After this change, every call to `advance()` runs to completion. Every child of an All advances. Every element of a ForEach advances. `advance` only creates frames and pushes effects to a queue. It never tears down frames or processes restarts.

### One effect queue

Advance produces effects: dispatches and restarts. Both go into a single FIFO queue. The engine exposes one-at-a-time access. The event loop processes them.

```rust
// lib.rs

/// An effect produced during advance.
#[derive(Debug)]
pub enum PendingEffect {
    /// A handler invocation ready to be dispatched to a worker.
    Dispatch(DispatchEvent),
    /// A deferred restart. The body will be torn down and the handler advanced.
    Restart(PendingRestartEvent),
}

/// A deferred restart effect.
#[derive(Debug)]
pub struct PendingRestartEvent {
    /// The `RestartHandle` frame that will process this restart.
    pub restart_handle_frame_id: FrameId,
    /// The payload value passed to the handler.
    pub payload: Value,
}
```

```rust
// lib.rs — WorkflowState (before)
pub struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,
    task_to_frame: BTreeMap<TaskId, FrameId>,
    pending_dispatches: VecDeque<DispatchEvent>,
    next_task_id: u32,
}

// lib.rs — WorkflowState (after)
pub struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,
    task_to_frame: BTreeMap<TaskId, FrameId>,
    pending_effects: VecDeque<PendingEffect>,
    next_task_id: u32,
}
```

### Engine API

The engine produces effects one at a time. This refactor replaces the dispatch-specific methods with effect-generic ones. `is_task_live` is unchanged — the event loop uses it for Dispatch and Completion liveness.

```rust
// lib.rs — WorkflowState impl (before)

pub fn pop_pending_dispatch(&mut self) -> Option<DispatchEvent> {
    self.pending_dispatches.pop_front()
}

pub fn is_task_live(&self, task_id: TaskId) -> bool {
    self.task_to_frame.contains_key(&task_id)
}

// lib.rs — WorkflowState impl (after)

/// Pop the next pending effect, or None if the queue is empty.
pub fn pop_pending_effect(&mut self) -> Option<PendingEffect> {
    self.pending_effects.pop_front()
}

/// Returns true if this task's Invoke frame still exists in the tree.
pub fn is_task_live(&self, task_id: TaskId) -> bool {
    self.task_to_frame.contains_key(&task_id)
}
```

`process_restart` is a new pub free function in `effects.rs`, following the same pattern as `advance::advance` and `complete::complete`:

```rust
// effects.rs

/// Process a single restart: tear down the body, advance the handler.
/// The handler advance may push more effects to `pending_effects`.
///
/// If the `RestartHandle` frame was already torn down by a previous
/// restart, this is a no-op.
pub fn process_restart(
    workflow_state: &mut WorkflowState,
    pending_restart: PendingRestartEvent,
) -> Result<(), AdvanceError> {
    // ... (see "Processing restarts" section below)
}
```

### Advance pushes to one queue

Invoke pushes a `PendingEffect::Dispatch`:

```rust
// advance.rs — Invoke arm (before)
FlatAction::Invoke { handler } => {
    let task_id = workflow_state.next_task_id();
    let frame_id = workflow_state.insert_frame(Frame {
        parent,
        kind: FrameKind::Invoke { handler },
    });
    workflow_state.task_to_frame.insert(task_id, frame_id);
    workflow_state.pending_dispatches.push_back(DispatchEvent {
        task_id,
        handler_id: handler,
        value,
    });
}

// advance.rs — Invoke arm (after)
FlatAction::Invoke { handler } => {
    let task_id = workflow_state.next_task_id();
    let frame_id = workflow_state.insert_frame(Frame {
        parent,
        kind: FrameKind::Invoke { handler },
    });
    workflow_state.task_to_frame.insert(task_id, frame_id);
    workflow_state.pending_effects.push_back(PendingEffect::Dispatch(DispatchEvent {
        task_id,
        handler_id: handler,
        value,
    }));
}
```

RestartPerform pushes a `PendingEffect::Restart`:

```rust
// advance.rs — RestartPerform arm (before)
FlatAction::RestartPerform { restart_handler_id } => {
    let parent =
        parent.ok_or(AdvanceError::UnhandledRestartEffect { restart_handler_id })?;
    super::effects::bubble_restart_effect(
        workflow_state,
        parent,
        restart_handler_id,
        value,
    )?;
}

// advance.rs — RestartPerform arm (after)
FlatAction::RestartPerform { restart_handler_id } => {
    let parent =
        parent.ok_or(AdvanceError::UnhandledRestartEffect { restart_handler_id })?;

    // Walk ancestors to find the matching RestartHandle.
    let restart_handle_frame_id =
        super::ancestors::ancestors(&workflow_state.frames, parent)
            .find_map(|(edge, frame)| {
                if let FrameKind::RestartHandle(restart_handle) = &frame.kind
                    && restart_handle.restart_handler_id == restart_handler_id
                {
                    Some(edge.frame_id())
                } else {
                    None
                }
            })
            .ok_or(AdvanceError::UnhandledRestartEffect { restart_handler_id })?;

    workflow_state.pending_effects.push_back(PendingEffect::Restart(PendingRestartEvent {
        restart_handle_frame_id,
        payload: value,
    }));
}
```

### Processing restarts (called by event loop, not engine)

```rust
// effects.rs

pub fn process_restart(
    workflow_state: &mut WorkflowState,
    pending_restart: PendingRestartEvent,
) -> Result<(), AdvanceError> {
    let PendingRestartEvent {
        restart_handle_frame_id,
        payload,
    } = pending_restart;

    // The RestartHandle may have been torn down by a previous restart.
    let Some(restart_handle_frame) =
        workflow_state.frames.get(restart_handle_frame_id)
    else {
        return Ok(());
    };
    let FrameKind::RestartHandle(ref restart_handle) = restart_handle_frame.kind else {
        return Ok(());
    };

    let handler_action_id = restart_handle.handler;
    let state = restart_handle.state.clone();

    // Tear down body.
    teardown_body(
        &mut workflow_state.frames,
        &mut workflow_state.task_to_frame,
        restart_handle_frame_id,
    );

    // Advance handler. This pushes more effects to pending_effects.
    let handler_input = serde_json::json!([payload, state]);
    super::advance::advance(
        workflow_state,
        handler_action_id,
        handler_input,
        Some(ParentRef::RestartHandle {
            frame_id: restart_handle_frame_id,
            side: RestartHandleSide::Handler,
        }),
    )?;

    Ok(())
}
```

### Event loop

The event loop processes one event at a time. There are three kinds of events. After this refactor, the `Event { task_id, kind: EventKind }` struct from EVENT_LOOP_RESTRUCTURE changes to a flat enum, because Restart events don't carry a task_id:

```rust
// barnum_event_loop/src/lib.rs

// Before (from EVENT_LOOP_RESTRUCTURE):
struct Event {
    task_id: TaskId,
    kind: EventKind,
}

enum EventKind {
    Dispatch { handler_id: HandlerId, value: Value },
    Completion { value: Value },
}

// After:
enum Event {
    /// A handler invocation ready to be sent to a worker.
    Dispatch(DispatchEvent),
    /// A deferred restart to process.
    Restart(PendingRestartEvent),
    /// A worker completed a task.
    Completion(CompletionEvent),
}

impl From<PendingEffect> for Event {
    fn from(pending_effect: PendingEffect) -> Self {
        match pending_effect {
            PendingEffect::Dispatch(dispatch_event) => Event::Dispatch(dispatch_event),
            PendingEffect::Restart(pending_restart_event) => Event::Restart(pending_restart_event),
        }
    }
}
```

Each iteration sources the next event — pending effects first, blocking for a scheduler completion only when the effect queue is empty — then processes it in a three-branch match:

1. **Dispatch** — check `is_task_live`; if stale, skip. Otherwise send to worker.
2. **Restart** — call `process_restart`. Liveness check is internal (checks if RestartHandle frame still exists). If stale, no-op.
3. **Completion** — check `is_task_live`; if stale, skip. Otherwise call `complete()`. The `expect("unknown task")` invariant is preserved.

```rust
// barnum_event_loop/src/lib.rs — run_workflow (before)
pub async fn run_workflow(
    workflow_state: &mut WorkflowState,
    scheduler: &mut Scheduler,
) -> Result<Value, RunWorkflowError> {
    let root = workflow_state.workflow_root();
    advance(workflow_state, root, Value::Null, None).expect("initial advance failed");

    loop {
        let event = if let Some(dispatch_event) = workflow_state.pop_pending_dispatch() {
            Event {
                task_id: dispatch_event.task_id,
                kind: EventKind::Dispatch {
                    handler_id: dispatch_event.handler_id,
                    value: dispatch_event.value,
                },
            }
        } else {
            let (task_id, result) = scheduler
                .recv()
                .await
                .expect("scheduler channel closed unexpectedly");
            Event {
                task_id,
                kind: EventKind::Completion { value: result? },
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
                if let Some(terminal_value) = complete(workflow_state, completion_event)? {
                    return Ok(terminal_value);
                }
            }
        }
    }
}

// barnum_event_loop/src/lib.rs — run_workflow (after)
pub async fn run_workflow(
    workflow_state: &mut WorkflowState,
    scheduler: &mut Scheduler,
) -> Result<Value, RunWorkflowError> {
    let root = workflow_state.workflow_root();
    advance(workflow_state, root, Value::Null, None).expect("initial advance failed");

    loop {
        let event: Event = if let Some(pending_effect) = workflow_state.pop_pending_effect() {
            pending_effect.into()
        } else {
            let (task_id, result) = scheduler
                .recv()
                .await
                .expect("scheduler channel closed unexpectedly");
            Event::Completion(CompletionEvent { task_id, value: result? })
        };

        match event {
            Event::Dispatch(dispatch_event) => {
                if !workflow_state.is_task_live(dispatch_event.task_id) {
                    continue;
                }
                let handler = workflow_state.handler(dispatch_event.handler_id);
                scheduler.dispatch(&dispatch_event, handler);
            }
            Event::Restart(pending_restart_event) => {
                process_restart(workflow_state, pending_restart_event)?;
            }
            Event::Completion(ref completion_event) => {
                if !workflow_state.is_task_live(completion_event.task_id) {
                    continue;
                }
                if let Some(terminal_value) = complete(workflow_state, completion_event)? {
                    return Ok(terminal_value);
                }
            }
        }
    }
}
```

### Walkthrough: `All(invoke_A, throw, invoke_B)`

1. `advance` processes All's three children in order:
   - invoke_A: pushes `Dispatch(A)` to `pending_effects`
   - throw (RestartPerform): walks ancestors, pushes `Restart(...)` to `pending_effects`
   - invoke_B: pushes `Dispatch(B)` to `pending_effects`
   - advance completes. Queue: `[Dispatch(A), Restart(...), Dispatch(B)]`

2. Event loop pops one effect at a time:
   - `Dispatch(A)`: `is_task_live(A)` → true → sent to worker
   - `Restart(...)`: `process_restart` → teardown removes A's and B's frames → handler advance pushes new effects to back of queue
   - `Dispatch(B)`: `is_task_live(B)` → false (torn down) → skipped
   - New effects from handler advance are processed next

3. Eventually, A's worker completes → event loop receives `(A, value)` → `is_task_live(A)` → false → skipped. `complete()` is never called.

Dispatch(A) was sent to a worker before the restart tore it down. That's wasted work, and that's fine. Dispatch(B) came after the restart in the queue, so it was skipped cheaply.

### WorkflowState construction

```rust
// lib.rs — WorkflowState::new (before)
pub fn new(flat_config: FlatConfig) -> Self {
    Self {
        flat_config,
        frames: Arena::new(),
        task_to_frame: BTreeMap::new(),
        pending_dispatches: VecDeque::new(),
        next_task_id: 0,
    }
}

// lib.rs — WorkflowState::new (after)
pub fn new(flat_config: FlatConfig) -> Self {
    Self {
        flat_config,
        frames: Arena::new(),
        task_to_frame: BTreeMap::new(),
        pending_effects: VecDeque::new(),
        next_task_id: 0,
    }
}
```

### What happens to `RestartHandleSide::Handler` in deliver

The handler-completion deliver path is unchanged. When a restart handler's TypeScript handler completes via `complete`, the handler result is delivered to `RestartHandleSide::Handler`, which re-advances the body. That body advance pushes more effects (dispatches and restarts), which the event loop processes on the next iteration.

```rust
// complete.rs — deliver (unchanged)
ParentRef::RestartHandle { frame_id, side } => match side {
    RestartHandleSide::Body => {
        let frame = workflow_state
            .frames
            .remove(frame_id)
            .expect("parent frame exists");
        deliver(workflow_state, frame.parent, value)
    }
    RestartHandleSide::Handler => {
        let frame = workflow_state
            .frames
            .get(frame_id)
            .expect("RestartHandle frame exists");
        let FrameKind::RestartHandle(ref restart_handle) = frame.kind else {
            unreachable!();
        };
        let body_action_id = restart_handle.body;
        super::advance::advance(
            workflow_state,
            body_action_id,
            value,
            Some(ParentRef::RestartHandle {
                frame_id,
                side: RestartHandleSide::Body,
            }),
        )?;
        Ok(None)
    }
},
```

## What changes

| Component | Before | After |
|-----------|--------|-------|
| `advance` for `Invoke` | Pushes `DispatchEvent` to `pending_dispatches` | Pushes `PendingEffect::Dispatch(DispatchEvent)` to `pending_effects` |
| `advance` for `RestartPerform` | Calls `bubble_restart_effect` (teardown + handler advance) | Walks ancestors, pushes `PendingEffect::Restart` to `pending_effects` |
| `bubble_restart_effect` | Exists in effects.rs | Deleted |
| `process_restart` | Does not exist | New free function in effects.rs: teardown + handler advance, called by event loop |
| `WorkflowState` fields | `pending_dispatches: VecDeque<DispatchEvent>` | `pending_effects: VecDeque<PendingEffect>` |
| `pop_pending_dispatch` | Returns `Option<DispatchEvent>` | Replaced by `pop_pending_effect` returning `Option<PendingEffect>` |
| `is_task_live` | Checks `task_to_frame` | Unchanged |
| `complete()` | `expect("unknown task")` — invariant enforced by event loop | Unchanged |
| Event loop types | `Event { task_id, kind: EventKind }` struct with two-variant `EventKind` | Flat `Event` enum with three variants (Dispatch, Restart, Completion) |
| Event loop match | Two-branch match with uniform `is_task_live` check before match | Three-branch match with per-branch liveness checks |

## Tests

The remaining `#[should_panic]` test becomes a passing test:

- `completing_torn_down_task_is_noop`: Already fixed by EVENT_LOOP_RESTRUCTURE.
- `restart_perform_non_terminal_in_all`: All advance loop completes entirely (advance is purely additive). Both children advance. Event loop processes the restart (teardown), then the stale dispatch is dropped by liveness check. Its completion (if any) is also dropped by liveness check.

The `drive_builtins` and `complete_and_drive` test helpers mirror the event loop. They pop effects one at a time, check liveness, process restarts, and execute builtins inline. Stale events are dropped before reaching `complete()`.

```rust
// test_helpers.rs — drive_builtins (before)
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
                let result = barnum_builtins::execute_builtin(
                    &builtin_handler.builtin,
                    &dispatch_event.value,
                )
                .unwrap();
                let completion_event = CompletionEvent {
                    task_id: dispatch_event.task_id,
                    value: result,
                };
                if let Some(value) = complete(engine, completion_event)? {
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

// test_helpers.rs — drive_builtins (after)
pub fn drive_builtins(
    engine: &mut WorkflowState,
) -> Result<(Option<Value>, Vec<DispatchEvent>), CompleteError> {
    let mut ts_dispatches: Vec<DispatchEvent> = Vec::new();
    loop {
        let Some(pending_effect) = engine.pop_pending_effect() else {
            break;
        };
        match pending_effect {
            PendingEffect::Restart(pending_restart_event) => {
                process_restart(engine, pending_restart_event)?;
            }
            PendingEffect::Dispatch(dispatch_event) => {
                if !engine.is_task_live(dispatch_event.task_id) {
                    continue;
                }
                match engine.handler(dispatch_event.handler_id).clone() {
                    HandlerKind::Builtin(builtin_handler) => {
                        let result = barnum_builtins::execute_builtin(
                            &builtin_handler.builtin,
                            &dispatch_event.value,
                        )
                        .unwrap();
                        let completion_event = CompletionEvent {
                            task_id: dispatch_event.task_id,
                            value: result,
                        };
                        if let Some(value) = complete(engine, completion_event)? {
                            return Ok((Some(value), ts_dispatches));
                        }
                    }
                    HandlerKind::TypeScript(_) => {
                        ts_dispatches.push(dispatch_event);
                    }
                }
            }
        }
    }
    Ok((None, ts_dispatches))
}
```

```rust
// test_helpers.rs — complete_and_drive (unchanged)
pub fn complete_and_drive(
    engine: &mut WorkflowState,
    completion_event: CompletionEvent,
) -> Result<(Option<Value>, Vec<DispatchEvent>), CompleteError> {
    if !engine.is_task_live(completion_event.task_id) {
        return Ok((None, Vec::new()));
    }
    let result = complete(engine, completion_event)?;
    if result.is_some() {
        return Ok((result, Vec::new()));
    }
    drive_builtins(engine)
}
```

All existing restart tests (`restart_branch_*`, `teardown_cleans_up_*`, `multi_step_restart_handler_chain`, etc.) should continue to pass with the updated helper.

### What gets deleted

- `bubble_restart_effect` in effects.rs — replaced by the `RestartPerform` advance arm (ancestor walk + enqueue) and `process_restart` (teardown + handler advance).
- `pending_dispatches` field on `WorkflowState` — replaced by `pending_effects`.
- `pop_pending_dispatch` — replaced by `pop_pending_effect`.
- `Event { task_id, kind: EventKind }` struct + `EventKind` enum in event loop — replaced by flat `Event` enum.

`bubble_resume_effect` is unchanged (ResumePerform is purely additive, no teardown).
