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

The fix: make `advance` purely additive. `RestartPerform` enqueues a pending restart instead of executing it. Teardown and handler execution happen after `advance` returns, in a well-defined processing phase.

## Design

### Invariant: every advance completes entirely

After this change, every call to `advance()` runs to completion. Every child of an All advances. Every element of a ForEach advances. No early breaks, no frame-existence checks, no partial iteration. `advance` only creates frames, pushes dispatches, and enqueues pending restarts.

### New field: `pending_restarts`

```rust
// lib.rs — WorkflowState
pub struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,
    task_to_frame: BTreeMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    pending_restarts: Vec<PendingRestart>,  // NEW
    next_task_id: u32,
}
```

```rust
// lib.rs — new type
/// A deferred restart effect. Created during advance when a `RestartPerform`
/// fires. Processed after advance/complete returns.
#[derive(Debug)]
pub struct PendingRestart {
    /// The `RestartHandle` frame that will process this restart.
    pub restart_handle_frame_id: FrameId,
    /// The payload value passed to the handler as `[payload, state]`.
    pub payload: Value,
}
```

### RestartPerform advance arm

```rust
// advance.rs
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

    workflow_state.pending_restarts.push(PendingRestart {
        restart_handle_frame_id,
        payload: value,
    });
}
```

Ancestor walking still happens during advance (to validate the handler exists and find its frame ID). The difference: no teardown, no handler advance. Just enqueue and return.

### Processing pending restarts

A new public method on `WorkflowState`:

```rust
// lib.rs — WorkflowState impl
/// Process all pending restarts. For each restart:
/// 1. Verify the `RestartHandle` frame still exists (a previous restart
///    in this batch may have torn it down).
/// 2. Tear down the body.
/// 3. Advance the handler DAG.
///
/// Processing a restart may trigger further advances that enqueue more
/// restarts (e.g., the restarted body immediately throws again). This
/// method drains the queue until empty.
///
/// # Errors
///
/// Returns [`AdvanceError`] if handler or body advance fails.
pub fn process_pending_restarts(&mut self) -> Result<(), AdvanceError> {
    effects::process_pending_restarts(self)
}
```

```rust
// effects.rs
pub fn process_pending_restarts(
    workflow_state: &mut WorkflowState,
) -> Result<(), AdvanceError> {
    // Drain loop: processing a restart may enqueue more restarts.
    while let Some(pending_restart) = workflow_state.pending_restarts.pop() {
        let PendingRestart {
            restart_handle_frame_id,
            payload,
        } = pending_restart;

        // The RestartHandle may have been torn down by a previous restart
        // in this batch. Skip it.
        let Some(restart_handle_frame) =
            workflow_state.frames.get(restart_handle_frame_id)
        else {
            continue;
        };
        let FrameKind::RestartHandle(ref restart_handle) = restart_handle_frame.kind
        else {
            continue;
        };

        let handler_action_id = restart_handle.handler;
        let state = restart_handle.state.clone();

        // Tear down body.
        teardown_body(
            &mut workflow_state.frames,
            &mut workflow_state.task_to_frame,
            restart_handle_frame_id,
        );

        // Advance handler.
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
    }

    Ok(())
}
```

### Filtering stale dispatches

After processing restarts, `pending_dispatches` may contain entries for tasks whose frames were torn down. A new public method:

```rust
// lib.rs — WorkflowState impl
/// Remove pending dispatches whose task is no longer in `task_to_frame`.
/// Call this after `process_pending_restarts` and before dispatching.
pub fn filter_stale_dispatches(&mut self) {
    self.pending_dispatches
        .retain(|dispatch| self.task_to_frame.contains_key(&dispatch.task_id));
}
```

### Graceful stale task completion

Already-dispatched tasks (sent to workers in a previous event loop iteration) can complete after their frame was torn down. `complete()` handles this gracefully:

```rust
// complete.rs
pub fn complete(
    workflow_state: &mut WorkflowState,
    task_id: super::TaskId,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let Some(frame_id) = workflow_state.task_to_frame.remove(&task_id) else {
        // Task belonged to a torn-down subtree.
        return Ok(None);
    };
    // ... rest unchanged
}
```

### Event loop changes

```rust
// barnum_event_loop/src/lib.rs — run_workflow
pub async fn run_workflow(
    workflow_state: &mut WorkflowState,
    scheduler: &mut Scheduler,
) -> Result<Value, RunWorkflowError> {
    let root = workflow_state.workflow_root();
    workflow_state
        .advance(root, Value::Null, None)
        .expect("initial advance failed");

    loop {
        // Process deferred restarts (may create new dispatches and more restarts).
        workflow_state.process_pending_restarts()?;

        // Filter dispatches for tasks torn down by restart processing.
        workflow_state.filter_stale_dispatches();

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
```

The `complete` → deliver → Chain trampoline → advance path can also enqueue restarts (when a task completion triggers a throw). These are processed on the next loop iteration, before dispatching.

### What happens to `bubble_restart_effect`

Deleted. Its responsibilities are split:
- **Ancestor walk + enqueue**: moved into the `RestartPerform` advance arm.
- **Teardown + handler advance**: moved into `process_pending_restarts`.

`bubble_resume_effect` is unchanged (ResumePerform is purely additive, no teardown).

### What happens to `restart_body` in deliver

The `RestartHandleSide::Handler` arm in `deliver` currently calls `restart_body`, which re-advances the body. This path is for when a restart handler's TypeScript handler completes (via `complete`). The handler result is delivered to the RestartHandle handler side, and the body needs to restart.

With deferred restarts, this path still needs to exist: the handler was advanced during `process_pending_restarts`, but if the handler is a TypeScript invoke, it completes later via `complete`. The deliver arm re-advances the body, which may trigger more RestartPerforms (enqueued as pending restarts, processed on the next event loop iteration).

```rust
// complete.rs — deliver
ParentRef::RestartHandle { frame_id, side } => match side {
    RestartHandleSide::Body => {
        // Body completed normally. Remove frame, deliver to parent.
        let frame = workflow_state
            .frames
            .remove(frame_id)
            .expect("parent frame exists");
        deliver(workflow_state, frame.parent, value)
    }
    RestartHandleSide::Handler => {
        // Handler completed. Re-advance body with handler output.
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

Unchanged from current code. The body re-advance may enqueue pending restarts, which are processed by the event loop on the next iteration.

## What changes

| Component | Before | After |
|-----------|--------|-------|
| `advance` for `RestartPerform` | Calls `bubble_restart_effect` (teardown + handler advance) | Walks ancestors, pushes `PendingRestart` |
| `bubble_restart_effect` | Exists in effects.rs | Deleted |
| `WorkflowState` | No `pending_restarts` field | `pending_restarts: Vec<PendingRestart>` |
| `process_pending_restarts` | Does not exist | New public method, drains pending restarts |
| `filter_stale_dispatches` | Does not exist | New public method, removes stale dispatches |
| `complete()` | Panics on unknown task_id | Returns `Ok(None)` |
| Event loop | `take_dispatches` → dispatch → recv → complete | `process_pending_restarts` → `filter_stale_dispatches` → `take_dispatches` → dispatch → recv → complete |
| `teardown_body` | Called from `bubble_restart_effect` during advance | Called from `process_pending_restarts` after advance |

## Tests

Both `#[should_panic]` tests become passing tests:

- `completing_torn_down_task_is_noop`: `complete()` returns `Ok(None)` instead of panicking.
- `restart_perform_non_terminal_in_all`: All advance loop completes entirely. Both children advance (RestartPerform enqueues, invoke creates dispatch). Restart processed after advance returns. Teardown removes the invoke's frame. Dispatch filtered. When the orphaned task completes, `Ok(None)`.

The `drive_builtins` test helper needs to call `process_pending_restarts` after processing builtins, since builtins may trigger Chain trampolines that advance RestartPerform nodes. The sequence becomes: take dispatches → execute builtins (calling `complete` for each) → `process_pending_restarts` → `filter_stale_dispatches` → take dispatches (for remaining TypeScript handlers).

All existing restart tests (`restart_branch_*`, `teardown_cleans_up_*`, `multi_step_restart_handler_chain`, etc.) should continue to pass with the updated `drive_builtins` helper.
