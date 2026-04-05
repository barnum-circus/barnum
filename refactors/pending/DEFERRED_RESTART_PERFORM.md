# Deferred RestartPerform

## Motivation

`RestartPerform` currently executes synchronously during `advance`: it tears down the body, advances the handler, and (if the handler is a builtin) restarts the body, all within the same call stack. This creates two bugs and a structural problem.

### Bug 1: Stale task completion panic

When `RestartPerform` fires (during `complete` → deliver → Chain trampoline → advance), `teardown_body` removes frames and `task_to_frame` entries for in-flight sibling tasks. When those tasks complete later, `complete()` panics:

```rust
// complete.rs:27-30
let frame_id = workflow_state
    .task_to_frame
    .remove(&task_id)
    .expect("unknown task"); // panics
```

Documented by the `completing_torn_down_task_is_noop` test with `#[should_panic(expected = "unknown task")]`.

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
// lib.rs — Dispatch rename (before)
pub struct Dispatch {
    pub task_id: TaskId,
    pub handler_id: HandlerId,
    pub value: Value,
}

// lib.rs — Dispatch rename (after)
pub struct DispatchEvent {
    pub task_id: TaskId,
    pub handler_id: HandlerId,
    pub value: Value,
}
```

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
    pending_dispatches: Vec<Dispatch>,
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

The engine produces effects one at a time and lets the event loop consume them:

```rust
// lib.rs — WorkflowState impl (before)

pub fn take_pending_dispatches(&mut self) -> Vec<Dispatch> {
    std::mem::take(&mut self.pending_dispatches)
}

// lib.rs — WorkflowState impl (after)

/// Pop the next pending effect, or None if the queue is empty.
pub fn pop_pending_effect(&mut self) -> Option<PendingEffect> {
    self.pending_effects.pop_front()
}

/// Check whether a task is still part of a live tree.
pub fn is_task_pending(&self, task_id: TaskId) -> bool {
    self.task_to_frame.contains_key(&task_id)
}

/// Process a single restart: tear down the body, advance the handler.
/// The handler advance may push more effects to `pending_effects`.
///
/// If the `RestartHandle` frame was already torn down by a previous
/// restart, this is a no-op.
pub fn process_restart(
    &mut self,
    pending_restart: PendingRestartEvent,
) -> Result<(), AdvanceError> {
    effects::process_restart(self, pending_restart)
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
    workflow_state.pending_dispatches.push(Dispatch {
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

The event loop processes one event at a time. There are three kinds of events, represented by a local enum in the event loop crate:

```rust
// barnum_event_loop/src/lib.rs

/// A completed task result from the scheduler.
struct CompletionEvent {
    task_id: TaskId,
    value: Value,
}

enum Event {
    /// A handler invocation ready to be sent to a worker.
    Dispatch(DispatchEvent),
    /// A deferred restart to process.
    Restart(PendingRestartEvent),
    /// A worker completed a task.
    Completion(CompletionEvent),
}
```

Each iteration sources the next event — pending effects first, blocking for a scheduler completion only when the effect queue is empty — then processes it in a three-branch match. Every branch checks liveness before doing work:

1. **Dispatch** — check `is_task_pending`; if stale, skip. Otherwise send to worker.
2. **Restart** — liveness check is inside `process_restart` (checks if RestartHandle frame still exists). If stale, no-op.
3. **Completion** — check `is_task_pending`; if stale, skip. Otherwise call `complete()`.

`complete()` keeps its `expect("unknown task")` — if the event loop calls it with a stale task, that's a bug in the event loop.

```rust
// barnum_event_loop/src/lib.rs — run_workflow (before)
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
        // Source the next event: pending effects first, then block for completion.
        let event = match workflow_state.pop_pending_effect() {
            Some(PendingEffect::Dispatch(dispatch_event)) => Event::Dispatch(dispatch_event),
            Some(PendingEffect::Restart(pending_restart_event)) => Event::Restart(pending_restart_event),
            None => {
                let (task_id, result) = scheduler
                    .recv()
                    .await
                    .expect("scheduler channel closed unexpectedly");
                Event::Completion(CompletionEvent { task_id, value: result? })
            }
        };

        // Process the event.
        match event {
            Event::Dispatch(dispatch_event) => {
                if workflow_state.is_task_pending(dispatch_event.task_id) {
                    let handler = workflow_state.handler(dispatch_event.handler_id);
                    scheduler.dispatch(&dispatch_event, handler);
                }
            }
            Event::Restart(pending_restart_event) => {
                workflow_state.process_restart(pending_restart_event)?;
            }
            Event::Completion(CompletionEvent { task_id, value }) => {
                if !workflow_state.is_task_pending(task_id) {
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

### Walkthrough: `All(invoke_A, throw, invoke_B)`

1. `advance` processes All's three children in order:
   - invoke_A: pushes `Dispatch(A)` to `pending_effects`
   - throw (RestartPerform): walks ancestors, pushes `Restart(...)` to `pending_effects`
   - invoke_B: pushes `Dispatch(B)` to `pending_effects`
   - advance completes. Queue: `[Dispatch(A), Restart(...), Dispatch(B)]`

2. Event loop pops one effect at a time:
   - `Dispatch(A)`: `is_task_pending(A)` → true → sent to worker
   - `Restart(...)`: `process_restart` → teardown removes A's and B's frames → handler advance pushes new effects to back of queue
   - `Dispatch(B)`: `is_task_pending(B)` → false (torn down) → skipped
   - New effects from handler advance are processed next

3. Eventually, A's worker completes → event loop receives `(A, value)` → `is_task_pending(A)` → false → skipped. `complete()` is never called.

Dispatch(A) was sent to a worker before the restart tore it down. That's wasted work, and that's fine. Dispatch(B) came after the restart in the queue, so it was skipped cheaply.

### What gets deleted

- `bubble_restart_effect` in effects.rs — replaced by the `RestartPerform` advance arm (ancestor walk + enqueue) and `process_restart` (teardown + handler advance).
- `pending_dispatches` field on `WorkflowState` — replaced by `pending_effects`.
- `take_pending_dispatches` method — replaced by `pop_pending_effect`.

`bubble_resume_effect` is unchanged (ResumePerform is purely additive, no teardown).

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
| `Dispatch` struct | Named `Dispatch` | Renamed to `DispatchEvent` |
| `advance` for `Invoke` | Pushes to `pending_dispatches` | Pushes `PendingEffect::Dispatch` to `pending_effects` |
| `advance` for `RestartPerform` | Calls `bubble_restart_effect` (teardown + handler advance) | Walks ancestors, pushes `PendingEffect::Restart` to `pending_effects` |
| `bubble_restart_effect` | Exists in effects.rs | Deleted |
| `WorkflowState` fields | `pending_dispatches: Vec<Dispatch>` | `pending_effects: VecDeque<PendingEffect>` |
| `take_pending_dispatches` | Returns `Vec<Dispatch>` | Deleted, replaced by `pop_pending_effect` returning `Option<PendingEffect>` |
| `is_task_pending` | Does not exist | New: checks `task_to_frame` for liveness |
| `process_restart` | Does not exist | New: teardown + handler advance, called by event loop |
| `complete()` | Unchanged | Unchanged — `expect("unknown task")` stays; event loop checks liveness before calling |
| Event loop | `take_pending_dispatches` → dispatch all → recv → complete | `pop_pending_effect` one at a time → liveness check on each event → recv when empty |
| `teardown_body` | Called from `bubble_restart_effect` during advance | Called from `process_restart`, invoked by event loop |

## Tests

Both `#[should_panic]` tests become passing tests:

- `completing_torn_down_task_is_noop`: The event loop checks `is_task_pending` before calling `complete()`. Stale completions are skipped. `complete()` is never called with an unknown task.
- `restart_perform_non_terminal_in_all`: All advance loop completes entirely. Both children advance. Event loop processes the restart (teardown), then skips the stale dispatch.

The `drive_builtins` test helper needs to process effects instead of dispatches. It takes effects one at a time, processes restarts, checks dispatch liveness, and executes builtins. This mirrors the event loop's behavior in a synchronous test context.

All existing restart tests (`restart_branch_*`, `teardown_cleans_up_*`, `multi_step_restart_handler_chain`, etc.) should continue to pass with the updated helper.
