# Deferred RestartPerform

**Blocked by:** EVENT_LOOP_RESTRUCTURE (done), FRAME_BASED_LIVENESS

## Motivation

`RestartPerform` currently executes synchronously during `advance`: it tears down the body, advances the handler, and (if the handler is a builtin) restarts the body, all within the same call stack. This creates two bugs and a structural problem.

### Bug 1: Stale task completion panic (fixed by EVENT_LOOP_RESTRUCTURE)

When `RestartPerform` fires (during `complete` → deliver → Chain trampoline → advance), `teardown_body` removes frames and `task_to_frame` entries for in-flight sibling tasks. When those tasks complete later, `complete()` panics on `expect("unknown task")`. EVENT_LOOP_RESTRUCTURE and FRAME_BASED_LIVENESS fix this: the event loop checks `is_frame_live()` before processing any event, so `complete()` is never called with unknown task_ids. The `expect("unknown task")` invariant is preserved.

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

### New types

FRAME_BASED_LIVENESS established `PendingEffect = (FrameId, PendingEffectKind)` with a single `Dispatch(DispatchEvent)` variant. This refactor adds `Restart(RestartEvent)`.

The `FrameId` in the tuple is the liveness key. For dispatches, it's the Invoke frame. For restarts, it's a lightweight marker frame at the perform site.

```rust
// lib.rs (after FRAME_BASED_LIVENESS)
pub type PendingEffect = (FrameId, PendingEffectKind);

#[derive(Debug)]
pub enum PendingEffectKind {
    Dispatch(DispatchEvent),
}

// lib.rs (after this refactor)
pub type PendingEffect = (FrameId, PendingEffectKind);

#[derive(Debug)]
pub enum PendingEffectKind {
    /// A handler invocation ready to be dispatched to a worker.
    Dispatch(DispatchEvent),
    /// A deferred restart. The body will be torn down and the handler advanced.
    Restart(RestartEvent),
}

/// A deferred restart effect. The `FrameId` in the `PendingEffect` tuple
/// is the marker frame (liveness key). This struct carries the handle
/// target and payload.
#[derive(Debug)]
pub struct RestartEvent {
    /// The `RestartHandle` frame that will process this restart.
    pub restart_handle_frame_id: FrameId,
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

### Engine API

FRAME_BASED_LIVENESS already provides `is_frame_live`, `task_frame_id`, `pop_pending_effect`, and the `pending_effects` queue. This refactor adds one new free function:

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
    restart_event: RestartEvent,
) -> Result<(), AdvanceError> {
    // ... (see "Processing restarts" section below)
}
```

### Advance: RestartPerform arm

The Invoke arm is unchanged from FRAME_BASED_LIVENESS. Only the RestartPerform arm changes:

```rust
// advance.rs — RestartPerform arm (before, i.e. after FRAME_BASED_LIVENESS)
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

    workflow_state.pending_effects.push_back((
        marker_frame_id,
        PendingEffectKind::Restart(RestartEvent {
            restart_handle_frame_id,
            payload: value,
        }),
    ));
}
```

### Processing restarts (called by event loop, not engine)

The caller has verified `is_frame_live(marker_frame_id)`, so we know the body subtree is intact and the RestartHandle frame exists.

```rust
// effects.rs

#[allow(clippy::expect_used)]
pub fn process_restart(
    workflow_state: &mut WorkflowState,
    restart_event: RestartEvent,
) -> Result<(), AdvanceError> {
    let RestartEvent {
        restart_handle_frame_id,
        payload,
    } = restart_event;

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

FRAME_BASED_LIVENESS established `Event = (FrameId, EventKind)` with `From<PendingEffectKind> for EventKind`. This refactor adds `EventKind::Restart` and a new `From` arm.

```rust
// barnum_event_loop/src/lib.rs (after FRAME_BASED_LIVENESS)
enum EventKind {
    Dispatch(DispatchEvent),
    Completion(CompletionEvent),
}

impl From<PendingEffectKind> for EventKind {
    fn from(kind: PendingEffectKind) -> Self {
        match kind {
            PendingEffectKind::Dispatch(dispatch_event) => EventKind::Dispatch(dispatch_event),
        }
    }
}

// barnum_event_loop/src/lib.rs (after this refactor)
enum EventKind {
    Dispatch(DispatchEvent),
    Restart(RestartEvent),
    Completion(CompletionEvent),
}

impl From<PendingEffectKind> for EventKind {
    fn from(kind: PendingEffectKind) -> Self {
        match kind {
            PendingEffectKind::Dispatch(dispatch_event) => EventKind::Dispatch(dispatch_event),
            PendingEffectKind::Restart(restart_event) => EventKind::Restart(restart_event),
        }
    }
}
```

The event loop structure is unchanged from FRAME_BASED_LIVENESS — just a new match arm:

```rust
// barnum_event_loop/src/lib.rs — run_workflow (after FRAME_BASED_LIVENESS)
loop {
    let (frame_id, event_kind) = if let Some((frame_id, pending_kind)) =
        workflow_state.pop_pending_effect()
    {
        (frame_id, EventKind::from(pending_kind))
    } else {
        let (task_id, result) = scheduler
            .recv()
            .await
            .expect("scheduler channel closed unexpectedly");
        let Some(frame_id) = workflow_state.task_frame_id(task_id) else {
            continue; // stale completion — task was torn down
        };
        (frame_id, EventKind::Completion(CompletionEvent { task_id, value: result? }))
    };

    if !workflow_state.is_frame_live(frame_id) {
        continue;
    }

    match event_kind {
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
    let (frame_id, event_kind) = if let Some((frame_id, pending_kind)) =
        workflow_state.pop_pending_effect()
    {
        (frame_id, EventKind::from(pending_kind))
    } else {
        let (task_id, result) = scheduler
            .recv()
            .await
            .expect("scheduler channel closed unexpectedly");
        let Some(frame_id) = workflow_state.task_frame_id(task_id) else {
            continue; // stale completion — task was torn down
        };
        (frame_id, EventKind::Completion(CompletionEvent { task_id, value: result? }))
    };

    if !workflow_state.is_frame_live(frame_id) {
        continue;
    }

    match event_kind {
        EventKind::Dispatch(dispatch_event) => {
            let handler = workflow_state.handler(dispatch_event.handler_id);
            scheduler.dispatch(&dispatch_event, handler);
        }
        EventKind::Restart(restart_event) => {
            process_restart(workflow_state, restart_event)?;
        }
        EventKind::Completion(completion_event) => {
            if let Some(terminal_value) = complete(workflow_state, completion_event)? {
                return Ok(terminal_value);
            }
        }
    }
}
```

### Walkthrough: `All(invoke_A, RestartPerform, invoke_B)`

1. `advance` processes All's three children in order:
   - invoke_A: creates Invoke frame (F1), pushes `(F1, Dispatch(...))`
   - RestartPerform: creates marker frame (F2), pushes `(F2, Restart(...))`
   - invoke_B: creates Invoke frame (F3), pushes `(F3, Dispatch(...))`
   - advance completes. Queue: `[(F1, Dispatch), (F2, Restart), (F3, Dispatch)]`

2. Event loop pops one effect at a time:
   - `(F1, Dispatch)`: `is_frame_live(F1)` → true → sent to worker
   - `(F2, Restart)`: `is_frame_live(F2)` → true → `process_restart` → teardown removes F1, F2, F3 → handler advance pushes new effects
   - `(F3, Dispatch)`: `is_frame_live(F3)` → false (torn down) → skipped

3. Eventually, A's worker completes → `task_frame_id(A)` → None (F1 removed) → skipped.

### Double-restart correctness

Two RestartPerforms for the same RestartHandle: the first creates marker F1, the second creates marker F2. Both are in the body subtree.

Queue: `[(F1, Restart(..., handle=RH)), (F2, Restart(..., handle=RH))]`

1. `(F1, Restart)`: `is_frame_live(F1)` → true → `process_restart` → teardown removes F1, F2 and all other body descendants.
2. `(F2, Restart)`: `is_frame_live(F2)` → false → skipped. ✓

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
let Some((frame_id, pending_effect_kind)) = engine.pop_pending_effect() else {
    break;
};
if !engine.is_frame_live(frame_id) {
    continue;
}
match pending_effect_kind {
    PendingEffectKind::Dispatch(dispatch_event) => {
        match engine.handler(dispatch_event.handler_id).clone() {
            // ...
        }
    }
}

// test_helpers.rs — drive_builtins (after this refactor)
let Some((frame_id, pending_effect_kind)) = engine.pop_pending_effect() else {
    break;
};
if !engine.is_frame_live(frame_id) {
    continue;
}
match pending_effect_kind {
    PendingEffectKind::Restart(restart_event) => {
        process_restart(engine, restart_event)?;
    }
    PendingEffectKind::Dispatch(dispatch_event) => {
        match engine.handler(dispatch_event.handler_id).clone() {
            // ...
        }
    }
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
| `PendingEffectKind` | `Dispatch(DispatchEvent)` | Adds `Restart(RestartEvent)` |
| `RestartEvent` | Does not exist | `{ restart_handle_frame_id, payload }` |
| `FrameKind` | No `RestartPerformMarker` | New `RestartPerformMarker` variant for liveness tracking |
| `advance` for `RestartPerform` | Calls `bubble_restart_effect` (teardown + handler advance) | Creates marker frame, walks ancestors, pushes `(marker_frame_id, PendingEffectKind::Restart(...))` |
| `bubble_restart_effect` | Exists in effects.rs | Deleted |
| `process_restart` | Does not exist | New free function in effects.rs: teardown + handler advance, uses `expect` (caller verifies liveness) |
| `EventKind` | Two variants: Dispatch, Completion | Adds `Restart(RestartEvent)` |
| `From<PendingEffectKind> for EventKind` | One arm: Dispatch | Adds arm: Restart |
| Event loop match | Dispatch + Completion | Adds Restart arm calling `process_restart` |

## What gets deleted

- `bubble_restart_effect` in effects.rs — replaced by the `RestartPerform` advance arm (marker frame + ancestor walk + enqueue) and `process_restart` (teardown + handler advance).

`bubble_resume_effect` is unchanged (ResumePerform is purely additive, no teardown).
