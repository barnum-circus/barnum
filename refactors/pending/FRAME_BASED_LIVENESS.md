# Frame-Based Liveness

**Blocked by:** EVENT_LOOP_RESTRUCTURE (done)

## Motivation

The event loop checks `is_task_live(event.task_id)` — which looks up `task_to_frame` to see if the Invoke frame still exists. This is an indirect check: it asks "does this task still have a frame?" when the real question is "does this frame still exist in the arena?"

This indirection also can't generalize. Restart effects don't have a task_id — they target a frame directly. To have a uniform liveness gate for all event types (dispatches, completions, and future restart effects), the check must be frame-based.

Two pre-factors, implemented together.

---

## Pre-factor 1: is_task_live → is_frame_live

### WorkflowState API

```rust
// lib.rs — WorkflowState impl (before)

/// Returns true if this task's Invoke frame still exists in the tree.
pub fn is_task_live(&self, task_id: TaskId) -> bool {
    self.task_to_frame.contains_key(&task_id)
}

// lib.rs — WorkflowState impl (after)

/// Returns true if `frame_id` still exists in the frame arena.
/// The single liveness check for all event types.
pub fn is_frame_live(&self, frame_id: FrameId) -> bool {
    self.frames.contains(frame_id)
}

/// Look up the Invoke frame ID for a task. Returns `None` if the task
/// was torn down (stale completion from the scheduler).
pub fn task_frame_id(&self, task_id: TaskId) -> Option<FrameId> {
    self.task_to_frame.get(&task_id).copied()
}
```

### Test helpers

```rust
// test_helpers.rs — drive_builtins (before)
if !engine.is_task_live(dispatch_event.task_id) {
    continue;
}

// test_helpers.rs — drive_builtins (after)
if engine.task_frame_id(dispatch_event.task_id).is_none() {
    continue;
}
```

```rust
// test_helpers.rs — complete_and_drive (before)
if !engine.is_task_live(completion_event.task_id) {
    return Ok((None, Vec::new()));
}

// test_helpers.rs — complete_and_drive (after)
if engine.task_frame_id(completion_event.task_id).is_none() {
    return Ok((None, Vec::new()));
}
```

---

## Pre-factor 2: Event carries frame_id

Every event carries a `frame_id` — the frame it originates from. The event loop checks `is_frame_live(event.frame_id)` once before the match. This is the single liveness gate for all event types.

For dispatches and completions, the `frame_id` is derived from `task_frame_id(task_id)` at event construction time. After DEFERRED_RESTART_PERFORM, restart effects will carry their own `frame_id` (a marker frame), so the frame-based check generalizes without any changes to the liveness gate.

### Event loop types

`Event` carries `frame_id` instead of `task_id`. `EventKind` variants carry full `DispatchEvent`/`CompletionEvent` structs (task_id lives inside them, not factored out).

```rust
// barnum_event_loop/src/lib.rs (before)
struct Event {
    task_id: TaskId,
    kind: EventKind,
}

enum EventKind {
    Dispatch { handler_id: HandlerId, value: Value },
    Completion { value: Value },
}

// barnum_event_loop/src/lib.rs (after)
struct Event {
    frame_id: FrameId,
    kind: EventKind,
}

enum EventKind {
    Dispatch(DispatchEvent),
    Completion(CompletionEvent),
}
```

### Event loop run_workflow

```rust
// barnum_event_loop/src/lib.rs — run_workflow (before)
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

// barnum_event_loop/src/lib.rs — run_workflow (after)
loop {
    let event = if let Some(dispatch_event) = workflow_state.pop_pending_dispatch() {
        let Some(frame_id) = workflow_state.task_frame_id(dispatch_event.task_id) else {
            continue; // stale dispatch — task was torn down
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
        EventKind::Completion(completion_event) => {
            if let Some(terminal_value) = complete(workflow_state, completion_event)? {
                return Ok(terminal_value);
            }
        }
    }
}
```

Note: `task_frame_id` returning `None` at construction time filters stale events. `is_frame_live` at processing time is redundant here (if `task_frame_id` returned `Some`, the frame exists). It becomes load-bearing after DEFERRED_RESTART_PERFORM, where restart effects carry a marker frame_id that isn't looked up via `task_frame_id`.

---

## What changes

| Component | Before | After |
|-----------|--------|-------|
| `is_task_live` | Checks `task_to_frame.contains_key(&task_id)` | Deleted |
| `is_frame_live` | Does not exist | Checks `frames.contains(frame_id)` |
| `task_frame_id` | Does not exist | Returns `task_to_frame.get(&task_id).copied()` |
| `Event` struct | `Event { task_id, kind }` | `Event { frame_id, kind }` |
| `EventKind::Dispatch` | `{ handler_id, value }` | `Dispatch(DispatchEvent)` |
| `EventKind::Completion` | `{ value }` | `Completion(CompletionEvent)` |
| Event loop liveness | `is_task_live(event.task_id)` | `is_frame_live(event.frame_id)` |
