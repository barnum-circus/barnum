# Deferred RestartPerform

**Blocked by:** EVENT_LOOP_RESTRUCTURE (done), FRAME_BASED_LIVENESS

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

---

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
    Dispatch {
        /// The Invoke frame's ID. Used as the liveness key.
        frame_id: FrameId,
        /// The dispatch payload for the scheduler.
        dispatch_event: DispatchEvent,
    },
    /// A deferred restart. The body will be torn down and the handler advanced.
    Restart(PendingRestartEvent),
}

impl PendingEffect {
    /// The frame targeted by this effect. If the frame has been removed
    /// from the arena, this effect is stale and should be skipped.
    pub fn frame_id(&self) -> FrameId {
        match self {
            Self::Dispatch { frame_id, .. } => *frame_id,
            Self::Restart(r) => r.marker_frame_id,
        }
    }
}

/// A deferred restart effect.
#[derive(Debug)]
pub struct PendingRestartEvent {
    /// The `RestartHandle` frame that will process this restart.
    pub restart_handle_frame_id: FrameId,
    /// Marker frame created at the RestartPerform site during advance.
    /// Lives in the RestartHandle's body subtree, so teardown_body removes
    /// it. The event loop checks this frame for liveness.
    pub marker_frame_id: FrameId,
    /// The payload value passed to the handler.
    pub payload: Value,
}
```

### Marker frame for RestartPerform

RestartPerform creates a lightweight marker frame during advance. This frame has no semantic role in the engine — it exists solely for liveness tracking. It's a child of the RestartPerform's parent in the frame tree, which means it lives in the RestartHandle's body subtree. When `teardown_body` removes body descendants, the marker frame is removed with them.

This solves the double-restart problem: if two RestartPerforms for the same RestartHandle are enqueued, the first one's `process_restart` tears down the body (removing both marker frames). The second one's liveness check fails because its marker frame was removed.

```rust
// frame.rs — new variant
pub enum FrameKind {
    // ... existing variants ...

    /// Marker for a deferred RestartPerform. No data — exists only so
    /// that teardown_body can remove it, causing the liveness check to
    /// fail for stale restart effects.
    RestartPerformMarker,
}
```

### WorkflowState

```rust
// lib.rs — WorkflowState (before, i.e. after FRAME_BASED_LIVENESS)
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

`is_frame_live` and `task_frame_id` already exist from FRAME_BASED_LIVENESS. This refactor replaces `pop_pending_dispatch` with `pop_pending_effect` and adds `process_restart`.

```rust
// lib.rs — WorkflowState impl (before)
pub fn pop_pending_dispatch(&mut self) -> Option<DispatchEvent> {
    self.pending_dispatches.pop_front()
}

// lib.rs — WorkflowState impl (after)
pub fn pop_pending_effect(&mut self) -> Option<PendingEffect> {
    self.pending_effects.pop_front()
}
```

`process_restart` is a new pub free function in `effects.rs`, following the same pattern as `advance::advance` and `complete::complete`. The caller has already verified liveness, so this function can `expect` the frame exists:

```rust
// effects.rs

/// Process a single restart: tear down the body, advance the handler.
/// The handler advance may push more effects to `pending_effects`.
///
/// # Panics
///
/// Panics if the RestartHandle frame does not exist. The caller must
/// verify liveness via `is_frame_live` before calling.
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
    workflow_state.pending_effects.push_back(PendingEffect::Dispatch {
        frame_id,
        dispatch_event: DispatchEvent {
            task_id,
            handler_id: handler,
            value,
        },
    });
}
```

RestartPerform creates a marker frame and pushes a `PendingEffect::Restart`:

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

    // Marker frame for liveness tracking. Lives in the body subtree,
    // so teardown_body removes it.
    let marker_frame_id = workflow_state.insert_frame(Frame {
        parent: Some(parent),
        kind: FrameKind::RestartPerformMarker,
    });

    workflow_state.pending_effects.push_back(PendingEffect::Restart(PendingRestartEvent {
        restart_handle_frame_id,
        marker_frame_id,
        payload: value,
    }));
}
```

### Processing restarts (called by event loop, not engine)

The caller has verified `is_frame_live(pending_restart.marker_frame_id)`, so we know the body subtree is intact and the RestartHandle frame exists.

```rust
// effects.rs

#[allow(clippy::expect_used)]
pub fn process_restart(
    workflow_state: &mut WorkflowState,
    pending_restart: PendingRestartEvent,
) -> Result<(), AdvanceError> {
    let PendingRestartEvent {
        restart_handle_frame_id,
        marker_frame_id: _,
        payload,
    } = pending_restart;

    let restart_handle_frame = workflow_state
        .frames
        .get(restart_handle_frame_id)
        .expect("RestartHandle frame exists (liveness verified by caller)");
    let FrameKind::RestartHandle(ref restart_handle) = restart_handle_frame.kind else {
        unreachable!("restart_handle_frame_id points to non-RestartHandle frame");
    };

    let handler_action_id = restart_handle.handler;
    let state = restart_handle.state.clone();

    // Tear down body (removes marker frame and all other body descendants).
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

FRAME_BASED_LIVENESS established `Event { frame_id, kind }` with `is_frame_live(event.frame_id)`. This refactor adds `EventKind::Restart` and changes the event source from `pop_pending_dispatch` to `pop_pending_effect`.

```rust
// barnum_event_loop/src/lib.rs (after FRAME_BASED_LIVENESS)
enum EventKind {
    Dispatch(DispatchEvent),
    Completion(CompletionEvent),
}

// barnum_event_loop/src/lib.rs (after this refactor)
enum EventKind {
    Dispatch(DispatchEvent),
    Restart(PendingRestartEvent),
    Completion(CompletionEvent),
}

impl From<PendingEffect> for Event {
    fn from(pending_effect: PendingEffect) -> Self {
        let frame_id = pending_effect.frame_id();
        match pending_effect {
            PendingEffect::Dispatch { dispatch_event, .. } => Event {
                frame_id,
                kind: EventKind::Dispatch(dispatch_event),
            },
            PendingEffect::Restart(pending_restart_event) => Event {
                frame_id,
                kind: EventKind::Restart(pending_restart_event),
            },
        }
    }
}
```

```rust
// barnum_event_loop/src/lib.rs — run_workflow (after FRAME_BASED_LIVENESS)
loop {
    let event = if let Some(dispatch_event) = workflow_state.pop_pending_dispatch() {
        let Some(frame_id) = workflow_state.task_frame_id(dispatch_event.task_id) else {
            continue;
        };
        Event {
            frame_id,
            kind: EventKind::Dispatch(dispatch_event),
        }
    } else {
        let (task_id, result) = scheduler
            .recv()
            .await
            .expect("scheduler channel closed unexpectedly");
        let Some(frame_id) = workflow_state.task_frame_id(task_id) else {
            continue;
        };
        Event {
            frame_id,
            kind: EventKind::Completion(CompletionEvent { task_id, value: result? }),
        }
    };

    if !workflow_state.is_frame_live(event.frame_id) {
        continue;
    }

    match event.kind {
        EventKind::Dispatch(dispatch_event) => {
            let handler = workflow_state.handler(dispatch_event.handler_id);
            scheduler.dispatch(&dispatch_event, handler);
        }
        EventKind::Completion(completion_event) => {
            if let Some(terminal_value) = complete(workflow_state, completion_event)? {
                return Ok(terminal_value);
            }
        }
    }
}

// barnum_event_loop/src/lib.rs — run_workflow (after this refactor)
loop {
    let event = if let Some(pending_effect) = workflow_state.pop_pending_effect() {
        Event::from(pending_effect)
    } else {
        let (task_id, result) = scheduler
            .recv()
            .await
            .expect("scheduler channel closed unexpectedly");
        let Some(frame_id) = workflow_state.task_frame_id(task_id) else {
            continue; // stale completion — task was torn down
        };
        Event {
            frame_id,
            kind: EventKind::Completion(CompletionEvent { task_id, value: result? }),
        }
    };

    if !workflow_state.is_frame_live(event.frame_id) {
        continue;
    }

    match event.kind {
        EventKind::Dispatch(dispatch_event) => {
            let handler = workflow_state.handler(dispatch_event.handler_id);
            scheduler.dispatch(&dispatch_event, handler);
        }
        EventKind::Restart(pending_restart_event) => {
            process_restart(workflow_state, pending_restart_event)?;
        }
        EventKind::Completion(completion_event) => {
            if let Some(terminal_value) = complete(workflow_state, completion_event)? {
                return Ok(terminal_value);
            }
        }
    }
}
```

In the local-effect path (`pop_pending_effect`), the `task_frame_id` lookup is gone. The frame_id comes directly from the `PendingEffect`. The `is_frame_live` check is the sole liveness gate — no longer redundant.

### Walkthrough: `All(invoke_A, RestartPerform, invoke_B)`

1. `advance` processes All's three children in order:
   - invoke_A: creates Invoke frame (F1), pushes `Dispatch { frame_id: F1, ... }`
   - RestartPerform: creates marker frame (F2), pushes `Restart { marker_frame_id: F2, ... }`
   - invoke_B: creates Invoke frame (F3), pushes `Dispatch { frame_id: F3, ... }`
   - advance completes. Queue: `[Dispatch(F1), Restart(F2), Dispatch(F3)]`

2. Event loop pops one effect at a time:
   - `Dispatch(F1)`: `is_frame_live(F1)` → true → sent to worker
   - `Restart(F2)`: `is_frame_live(F2)` → true → `process_restart` → teardown removes F1, F2, F3 → handler advance pushes new effects
   - `Dispatch(F3)`: `is_frame_live(F3)` → false (torn down) → skipped

3. Eventually, A's worker completes → `task_frame_id(A)` → None (F1 removed) → skipped.

### Double-restart correctness

Two RestartPerforms for the same RestartHandle: the first creates marker F1, the second creates marker F2. Both are in the body subtree.

Queue: `[Restart(marker=F1, handle=RH), Restart(marker=F2, handle=RH)]`

1. `Restart(F1)`: `is_frame_live(F1)` → true → `process_restart` → teardown removes F1, F2 and all other body descendants.
2. `Restart(F2)`: `is_frame_live(F2)` → false → skipped. ✓

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

### Test helpers

```rust
// test_helpers.rs — drive_builtins (after FRAME_BASED_LIVENESS)
pub fn drive_builtins(
    engine: &mut WorkflowState,
) -> Result<(Option<Value>, Vec<DispatchEvent>), CompleteError> {
    let mut ts_dispatches: Vec<DispatchEvent> = Vec::new();
    loop {
        let Some(dispatch_event) = engine.pop_pending_dispatch() else {
            break;
        };
        if engine.task_frame_id(dispatch_event.task_id).is_none() {
            continue;
        }
        // ... builtin/TS dispatch ...
    }
    Ok((None, ts_dispatches))
}

// test_helpers.rs — drive_builtins (after this refactor)
pub fn drive_builtins(
    engine: &mut WorkflowState,
) -> Result<(Option<Value>, Vec<DispatchEvent>), CompleteError> {
    let mut ts_dispatches: Vec<DispatchEvent> = Vec::new();
    loop {
        let Some(pending_effect) = engine.pop_pending_effect() else {
            break;
        };
        if !engine.is_frame_live(pending_effect.frame_id()) {
            continue;
        }
        match pending_effect {
            PendingEffect::Restart(pending_restart_event) => {
                process_restart(engine, pending_restart_event)?;
            }
            PendingEffect::Dispatch { dispatch_event, .. } => {
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

`complete_and_drive` is unchanged from FRAME_BASED_LIVENESS.

### Tests

The remaining `#[should_panic]` test becomes a passing test:

- `completing_torn_down_task_is_noop`: Already fixed by EVENT_LOOP_RESTRUCTURE.
- `restart_perform_non_terminal_in_all`: All advance loop completes entirely (advance is purely additive). Both children advance. Event loop processes the restart (teardown), then the stale dispatch is dropped by liveness check.

All existing restart tests (`restart_branch_*`, `teardown_cleans_up_*`, `multi_step_restart_handler_chain`, etc.) should continue to pass with the updated helpers.

---

## What changes

| Component | Before (after FRAME_BASED_LIVENESS) | After |
|-----------|--------|-------|
| `advance` for `Invoke` | Pushes `DispatchEvent` to `pending_dispatches` | Pushes `PendingEffect::Dispatch { frame_id, dispatch_event }` to `pending_effects` |
| `advance` for `RestartPerform` | Calls `bubble_restart_effect` (teardown + handler advance) | Creates marker frame, walks ancestors, pushes `PendingEffect::Restart` to `pending_effects` |
| `FrameKind` | No `RestartPerformMarker` | New `RestartPerformMarker` variant for liveness tracking |
| `bubble_restart_effect` | Exists in effects.rs | Deleted |
| `process_restart` | Does not exist | New free function in effects.rs: teardown + handler advance, uses `expect` (caller verifies liveness) |
| `WorkflowState` fields | `pending_dispatches: VecDeque<DispatchEvent>` | `pending_effects: VecDeque<PendingEffect>` |
| `pop_pending_dispatch` | Returns `Option<DispatchEvent>` | Replaced by `pop_pending_effect` returning `Option<PendingEffect>` |
| Event loop local source | `pop_pending_dispatch` + `task_frame_id` lookup | `pop_pending_effect` + `Event::from(pending_effect)` |
| `EventKind` | Two variants: Dispatch, Completion | Three variants: Dispatch, Restart, Completion |

## What gets deleted

- `bubble_restart_effect` in effects.rs — replaced by the `RestartPerform` advance arm (marker frame + ancestor walk + enqueue) and `process_restart` (teardown + handler advance).
- `pending_dispatches` field on `WorkflowState` — replaced by `pending_effects`.
- `pop_pending_dispatch` — replaced by `pop_pending_effect`.

`bubble_resume_effect` is unchanged (ResumePerform is purely additive, no teardown).
