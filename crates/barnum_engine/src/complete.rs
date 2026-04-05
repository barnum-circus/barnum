use serde_json::Value;

use super::frame::{FrameKind, HandleSide, ParentRef};
use super::{
    AncestorCheck, CompleteError, StashOutcome, StashedItem, SweepResult, TaskId, TryDeliverResult,
    WorkflowState,
};

/// Deliver a task result. The caller invokes this when a dispatched
/// handler finishes.
///
/// Returns `Ok(Some(value))` when the workflow terminates, `Ok(None)`
/// when it's still running.
///
/// # Errors
///
/// Returns [`CompleteError`] if the result value has an invalid shape,
/// or if an advance error occurs during Chain trampoline or Handle re-entry.
///
/// # Panics
///
/// Panics if `task_id` is not a known pending task.
#[allow(clippy::expect_used)]
pub fn complete(
    workflow_state: &mut WorkflowState,
    task_id: TaskId,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let frame_id = workflow_state
        .task_to_frame
        .remove(&task_id)
        .expect("unknown task");
    let frame = workflow_state
        .frames
        .remove(frame_id)
        .expect("invoke frame exists");
    debug_assert!(
        matches!(frame.kind, FrameKind::Invoke { .. }),
        "task_to_frame pointed at non-Invoke frame: {:?}",
        frame.kind,
    );
    let result = match frame.parent {
        Some(parent_ref) => match try_deliver(workflow_state, parent_ref, value)? {
            TryDeliverResult::Delivered(result) => result,
            TryDeliverResult::Blocked(value) => {
                workflow_state
                    .stashed_items
                    .push_back(StashedItem::Delivery { parent_ref, value });
                None
            }
            TryDeliverResult::FrameGone => None,
        },
        None => Some(value),
    };
    if result.is_some() {
        return Ok(result);
    }
    sweep_stash(workflow_state)
}

/// Check if delivery is possible and deliver if so. Does NOT push
/// to the stash — the caller is responsible for stashing on `Blocked`.
pub fn try_deliver(
    workflow_state: &mut WorkflowState,
    parent_ref: ParentRef,
    value: Value,
) -> Result<TryDeliverResult, CompleteError> {
    match super::ancestors::find_blocking_ancestor(&workflow_state.frames, parent_ref) {
        AncestorCheck::FrameGone => Ok(TryDeliverResult::FrameGone),
        AncestorCheck::Blocked => Ok(TryDeliverResult::Blocked(value)),
        AncestorCheck::Clear => {
            let result = deliver(workflow_state, Some(parent_ref), value)?;
            Ok(TryDeliverResult::Delivered(result))
        }
    }
}

// -- Sweep stash --

/// Repeatedly sweep the stash until no progress is made.
fn sweep_stash(workflow_state: &mut WorkflowState) -> Result<Option<Value>, CompleteError> {
    loop {
        match sweep_stash_once(workflow_state)? {
            SweepResult::WorkflowDone(value) => return Ok(Some(value)),
            SweepResult::MadeProgress => {}
            SweepResult::NoProgress => return Ok(None),
        }
    }
}

/// Single pass over items that existed at the start.
#[allow(clippy::expect_used)]
fn sweep_stash_once(workflow_state: &mut WorkflowState) -> Result<SweepResult, CompleteError> {
    let n = workflow_state.stashed_items.len();
    for _ in 0..n {
        let item = workflow_state
            .stashed_items
            .pop_front()
            .expect("stash has n items");
        match item {
            StashedItem::Delivery { parent_ref, value } => {
                match try_deliver(workflow_state, parent_ref, value)? {
                    TryDeliverResult::Delivered(Some(value)) => {
                        return Ok(SweepResult::WorkflowDone(value));
                    }
                    TryDeliverResult::Delivered(None) => {
                        return Ok(SweepResult::MadeProgress);
                    }
                    TryDeliverResult::Blocked(value) => {
                        workflow_state
                            .stashed_items
                            .push_back(StashedItem::Delivery { parent_ref, value });
                    }
                    TryDeliverResult::FrameGone => {
                        // Frame torn down. Item silently dropped.
                    }
                }
            }
            StashedItem::Effect {
                starting_parent,
                effect_id,
                payload,
            } => {
                match super::effects::bubble_effect(
                    workflow_state,
                    starting_parent,
                    effect_id,
                    payload,
                )? {
                    StashOutcome::Consumed => {
                        return Ok(SweepResult::MadeProgress);
                    }
                    StashOutcome::Blocked(payload) => {
                        workflow_state.stashed_items.push_back(StashedItem::Effect {
                            starting_parent,
                            effect_id,
                            payload,
                        });
                    }
                }
            }
        }
    }
    Ok(SweepResult::NoProgress)
}

/// Deliver a value to the parent that was waiting for it.
///
/// - **No parent:** workflow done — return the terminal value.
/// - **Chain:** trampoline — advance the `rest` action with the value.
/// - **All/ForEach:** store in results slot; if all slots filled,
///   collect into array and deliver to parent.
#[allow(clippy::expect_used, clippy::unwrap_used)]
pub fn deliver(
    workflow_state: &mut WorkflowState,
    parent: Option<ParentRef>,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let Some(parent_ref) = parent else {
        return Ok(Some(value));
    };

    match parent_ref {
        ParentRef::Chain { frame_id } => {
            let frame = workflow_state
                .frames
                .remove(frame_id)
                .expect("parent frame exists");
            let FrameKind::Chain { rest } = frame.kind else {
                unreachable!(
                    "Chain ParentRef points to non-Chain frame: {:?}",
                    frame.kind
                )
            };
            super::advance::advance(workflow_state, rest, value, frame.parent)?;
            Ok(None)
        }

        ParentRef::Handle { frame_id, side } => match side {
            HandleSide::Body => {
                // Body delivered.
                // Deliver to the Handle's parent.
                let frame = workflow_state
                    .frames
                    .remove(frame_id)
                    .expect("parent frame exists");
                deliver(workflow_state, frame.parent, value)
            }
            HandleSide::Handler => {
                // Handler completed. Process the output.
                super::effects::handle_handler_completion(workflow_state, frame_id, value)
            }
        },

        ParentRef::All {
            frame_id,
            child_index,
        }
        | ParentRef::ForEach {
            frame_id,
            child_index,
        } => {
            let frame = workflow_state
                .frames
                .get_mut(frame_id)
                .expect("parent frame exists");
            let results = match &mut frame.kind {
                FrameKind::All { results } | FrameKind::ForEach { results } => results,
                other => {
                    unreachable!("All/ForEach ParentRef points to wrong frame: {:?}", other)
                }
            };
            results[child_index] = Some(value);
            if results.iter().all(Option::is_some) {
                let collected: Vec<Value> = results.iter_mut().map(|r| r.take().unwrap()).collect();
                let parent = frame.parent;
                workflow_state.frames.remove(frame_id);
                deliver(workflow_state, parent, Value::Array(collected))
            } else {
                Ok(None)
            }
        }
    }
}
