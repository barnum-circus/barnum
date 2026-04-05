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

`advance` both creates frames (additive) and tears them down (destructive). When a RestartPerform fires inside an All child that contains a RestartHandle, the All loop must reason about which sibling frames survived the teardown and which didn't. A contained throw (targeting a handler inside the same All child) leaves the All frame alive and siblings should continue advancing. An escaping throw (targeting a handler above the All) destroys the All frame and siblings should stop. Getting this reasoning right for all cases is error-prone.

The fix: make `advance` purely additive. `RestartPerform` enqueues a pending effect instead of executing it. Teardown and handler execution happen after `advance` returns.

## Design

### Invariant: every advance completes entirely

After this change, every call to `advance()` runs to completion. Every child of an All advances. Every element of a ForEach advances. No early breaks, no frame-existence checks, no partial iteration.

### One effect queue

Advance produces two kinds of effects: dispatches (send work to a worker) and restarts (tear down a body and advance a handler). Both go into a single queue.

```rust
// lib.rs

/// An effect produced during advance. Processed after advance returns.
#[derive(Debug)]
pub enum PendingEffect {
    /// A handler invocation ready to be dispatched to a worker.
    Dispatch(Dispatch),
    /// A deferred restart. The body will be torn down and the handler advanced.
    Restart(PendingRestart),
}

/// A deferred restart effect.
#[derive(Debug)]
pub struct PendingRestart {
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
    pending_effects: Vec<PendingEffect>,
    next_task_id: u32,
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
    workflow_state.pending_effects.push(PendingEffect::Dispatch(Dispatch {
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

    workflow_state.pending_effects.push(PendingEffect::Restart(PendingRestart {
        restart_handle_frame_id,
        payload: value,
    }));
}
```

### Draining the effect queue

A single public method on `WorkflowState` processes all pending effects and returns the dispatches that survived:

```rust
// lib.rs — WorkflowState impl

/// Drain all pending effects. Restarts are processed internally (teardown +
/// handler advance, which may produce more effects). Dispatches are collected
/// and returned, skipping any whose task was torn down by a restart.
///
/// Call this after `advance` or `complete` to get the dispatches to send
/// to workers.
pub fn drain_pending_effects(&mut self) -> Result<Vec<Dispatch>, AdvanceError> {
    effects::drain_pending_effects(self)
}
```

```rust
// effects.rs

pub fn drain_pending_effects(
    workflow_state: &mut WorkflowState,
) -> Result<Vec<Dispatch>, AdvanceError> {
    let mut dispatches = Vec::new();

    while !workflow_state.pending_effects.is_empty() {
        let effects = std::mem::take(&mut workflow_state.pending_effects);
        for effect in effects {
            match effect {
                PendingEffect::Dispatch(dispatch) => {
                    // Skip dispatches whose frame was torn down by a restart
                    // earlier in this batch.
                    if workflow_state.task_to_frame.contains_key(&dispatch.task_id) {
                        dispatches.push(dispatch);
                    }
                }
                PendingEffect::Restart(pending_restart) => {
                    process_restart(workflow_state, pending_restart)?;
                    // process_restart may have pushed more effects via advance.
                    // The outer while loop will pick them up.
                }
            }
        }
    }

    Ok(dispatches)
}

fn process_restart(
    workflow_state: &mut WorkflowState,
    pending_restart: PendingRestart,
) -> Result<(), AdvanceError> {
    let PendingRestart {
        restart_handle_frame_id,
        payload,
    } = pending_restart;

    // The RestartHandle may have been torn down by a previous restart
    // in this batch. Skip it.
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

    // Advance handler. This may push more effects to pending_effects.
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

The drain processes effects in FIFO order. Each restart tears down frames and advances a handler (which pushes more effects to the queue). Each dispatch is checked against `task_to_frame` — if the task was torn down by an earlier restart, it's skipped. The outer `while` loop repeats until no more effects remain.

### Graceful stale task completion

Already-dispatched tasks (sent to workers in a previous event loop iteration) can complete after their frame was torn down. `complete()` handles this:

```rust
// complete.rs (before)
pub fn complete(
    workflow_state: &mut WorkflowState,
    task_id: super::TaskId,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let frame_id = workflow_state
        .task_to_frame
        .remove(&task_id)
        .expect("unknown task");
    // ...
}

// complete.rs (after)
pub fn complete(
    workflow_state: &mut WorkflowState,
    task_id: super::TaskId,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let Some(frame_id) = workflow_state.task_to_frame.remove(&task_id) else {
        return Ok(None);
    };
    // ... rest unchanged
}
```

### Event loop

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
        let dispatches = workflow_state.drain_pending_effects()?;
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
```

The event loop calls `drain_pending_effects` instead of `take_pending_dispatches`. The drain handles restarts internally and returns only the live dispatches. The `complete` → deliver → Chain trampoline → advance path can also push effects (when a task completion triggers a throw). These are drained on the next loop iteration.

### What gets deleted

- `bubble_restart_effect` in effects.rs — replaced by the `RestartPerform` advance arm (ancestor walk + enqueue) and `process_restart` (teardown + handler advance).
- `pending_dispatches` field on `WorkflowState` — replaced by `pending_effects`.
- `take_pending_dispatches` method — replaced by `drain_pending_effects`.
- `filter_stale_dispatches` — folded into `drain_pending_effects`.
- `process_pending_restarts` — folded into `drain_pending_effects`.

`bubble_resume_effect` is unchanged (ResumePerform is purely additive, no teardown).

### What happens to `RestartHandleSide::Handler` in deliver

The handler-completion deliver path is unchanged. When a restart handler's TypeScript handler completes via `complete`, the handler result is delivered to `RestartHandleSide::Handler`, which re-advances the body. That body advance may push more effects (dispatches and restarts), which are drained on the next event loop iteration.

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
| `advance` for `Invoke` | Pushes to `pending_dispatches` | Pushes `PendingEffect::Dispatch` to `pending_effects` |
| `advance` for `RestartPerform` | Calls `bubble_restart_effect` (teardown + handler advance) | Walks ancestors, pushes `PendingEffect::Restart` to `pending_effects` |
| `bubble_restart_effect` | Exists in effects.rs | Deleted |
| `WorkflowState` fields | `pending_dispatches: Vec<Dispatch>` | `pending_effects: Vec<PendingEffect>` |
| `take_pending_dispatches` | Returns `Vec<Dispatch>` | Deleted, replaced by `drain_pending_effects` |
| `drain_pending_effects` | Does not exist | New: drains effects, processes restarts, returns live dispatches |
| `complete()` | Panics on unknown task_id | Returns `Ok(None)` |
| Event loop | `take_pending_dispatches` → dispatch → recv → complete | `drain_pending_effects` → dispatch → recv → complete |
| `teardown_body` | Called from `bubble_restart_effect` during advance | Called from `process_restart` inside `drain_pending_effects` |

## Tests

Both `#[should_panic]` tests become passing tests:

- `completing_torn_down_task_is_noop`: `complete()` returns `Ok(None)` instead of panicking.
- `restart_perform_non_terminal_in_all`: All advance loop completes entirely. Both children advance (RestartPerform pushes Restart effect, invoke pushes Dispatch effect). `drain_pending_effects` processes the Restart (teardown removes invoke's frame), then skips the stale Dispatch.

The `drive_builtins` test helper needs updating: instead of calling `take_pending_dispatches`, it calls `drain_pending_effects`, which handles restarts and filtering internally. This simplifies the helper — it no longer needs separate restart processing.

All existing restart tests (`restart_branch_*`, `teardown_cleans_up_*`, `multi_step_restart_handler_chain`, etc.) should continue to pass with the updated helper.
