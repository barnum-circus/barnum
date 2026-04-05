# Frame-Based Liveness

**Blocked by:** EVENT_LOOP_RESTRUCTURE (done)

## Motivation

The event loop checks `is_task_live(event.task_id)` — which looks up `task_to_frame` to see if the Invoke frame still exists. This is an indirect check: it asks "does this task still have a frame?" when the real question is "does this frame still exist in the arena?"

This indirection also can't generalize. Restart effects don't have a task_id — they target a frame directly. To have a uniform liveness gate for all event types (dispatches, completions, and future restart effects), the check must be frame-based.

Both `PendingEffect` and `Event` are two-tuples of `(FrameId, Kind)`. The `FrameId` is the liveness key. The `Kind` is the payload. This structural pattern is established here so DEFERRED_RESTART_PERFORM just adds new variants.

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

---

## Pre-factor 2: PendingEffect and Event as (FrameId, Kind) tuples

`PendingEffect` and `Event` are both `(FrameId, Kind)`. The first element is the liveness key. The second is the payload.

`PendingEffectKind` starts with one variant (`Dispatch`). DEFERRED_RESTART_PERFORM adds `Restart`. `EventKind` has `Dispatch` and `Completion`; DEFERRED_RESTART_PERFORM adds `Restart`.

The payload types (`DispatchEvent`, `CompletionEvent`, and later `RestartEvent`) are shared between both enums.

### Engine types

```rust
// lib.rs (new types)

/// `(FrameId, PendingEffectKind)` — the liveness key and effect payload.
pub type PendingEffect = (FrameId, PendingEffectKind);

/// The payload of a pending effect.
#[derive(Debug)]
pub enum PendingEffectKind {
    /// A handler invocation ready to be dispatched to a worker.
    Dispatch(DispatchEvent),
}
```

### WorkflowState

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

### Advance

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
    workflow_state.pending_effects.push_back((
        frame_id,
        PendingEffectKind::Dispatch(DispatchEvent {
            task_id,
            handler_id: handler,
            value,
        }),
    ));
}
```

### Event loop types

`Event` is `(FrameId, EventKind)`, mirroring `PendingEffect`.

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
type Event = (FrameId, EventKind);

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
```

### Test helpers

```rust
// test_helpers.rs — drive_builtins (before)
let Some(dispatch_event) = engine.pop_pending_dispatch() else { break; };
if !engine.is_task_live(dispatch_event.task_id) {
    continue;
}
match engine.handler(dispatch_event.handler_id).clone() {
    // ...
}

// test_helpers.rs — drive_builtins (after)
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

## What changes

| Component | Before | After |
|-----------|--------|-------|
| `is_task_live` | Checks `task_to_frame.contains_key(&task_id)` | Deleted |
| `is_frame_live` | Does not exist | Checks `frames.contains(frame_id)` |
| `task_frame_id` | Does not exist | Returns `task_to_frame.get(&task_id).copied()` |
| `PendingEffect` | Does not exist | Type alias: `(FrameId, PendingEffectKind)` |
| `PendingEffectKind` | Does not exist | Enum: `Dispatch(DispatchEvent)` |
| `pending_dispatches` | `VecDeque<DispatchEvent>` | Replaced by `pending_effects: VecDeque<PendingEffect>` |
| `pop_pending_dispatch` | Returns `Option<DispatchEvent>` | Replaced by `pop_pending_effect` returning `Option<PendingEffect>` |
| `advance` Invoke arm | Pushes `DispatchEvent` to `pending_dispatches` | Pushes `(frame_id, PendingEffectKind::Dispatch(...))` to `pending_effects` |
| `Event` | Struct: `{ task_id, kind }` | Type alias: `(FrameId, EventKind)` |
| `EventKind::Dispatch` | `{ handler_id, value }` | `Dispatch(DispatchEvent)` |
| `EventKind::Completion` | `{ value }` | `Completion(CompletionEvent)` |
| Event loop source | `pop_pending_dispatch` + manual Event construction | `pop_pending_effect` + `EventKind::from(pending_kind)` |
| Event loop liveness | `is_task_live(event.task_id)` | `is_frame_live(frame_id)` |
