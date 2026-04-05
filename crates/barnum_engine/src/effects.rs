use std::collections::BTreeMap;

use barnum_ast::EffectId;
use serde_json::Value;
use thunderdome::Arena;

use super::frame::{Frame, FrameId, FrameKind, HandleSide, HandleStatus, ParentRef};
use super::{
    AdvanceError, AncestorCheck, CompleteError, HandlerOutput, StashOutcome, StashedItem,
    StateUpdate, TaskId, TryDeliverResult, WorkflowState,
};

/// Walk the parent chain from `starting_parent` upward looking for a
/// Handle that matches `effect_id`. If found, dispatch to it. Does NOT
/// mutate the stash.
#[allow(clippy::expect_used)]
pub fn bubble_effect(
    workflow_state: &mut WorkflowState,
    starting_parent: ParentRef,
    effect_id: EffectId,
    payload: Value,
) -> Result<StashOutcome, AdvanceError> {
    // First: can this effect proceed at all?
    match super::ancestors::find_blocking_ancestor(&workflow_state.frames, starting_parent) {
        AncestorCheck::FrameGone => return Ok(StashOutcome::Consumed),
        AncestorCheck::Blocked => return Ok(StashOutcome::Blocked(payload)),
        AncestorCheck::Clear => {}
    }

    // Not blocked — walk the parent chain to find the matching Handle.
    find_and_dispatch_handler(workflow_state, starting_parent, effect_id, payload)
}

/// Walk from `starting_parent` upward. All ancestors are guaranteed
/// present and unblocked (caller checked via `find_blocking_ancestor`).
fn find_and_dispatch_handler(
    workflow_state: &mut WorkflowState,
    starting_parent: ParentRef,
    effect_id: EffectId,
    payload: Value,
) -> Result<StashOutcome, AdvanceError> {
    let handle_frame_id = super::ancestors::ancestors(&workflow_state.frames, starting_parent)
        .find_map(|(edge, frame)| {
            if let FrameKind::Handle(handle_frame) = &frame.kind
                && handle_frame.effect_id == effect_id
            {
                Some(edge.frame_id())
            } else {
                None
            }
        })
        .ok_or(AdvanceError::UnhandledEffect { effect_id })?;

    dispatch_to_handler(workflow_state, handle_frame_id, starting_parent, payload)?;
    Ok(StashOutcome::Consumed)
}

/// Dispatch a handler for a matched effect. Suspends the Handle and
/// advances the handler DAG.
#[allow(clippy::expect_used, clippy::needless_pass_by_value)]
fn dispatch_to_handler(
    workflow_state: &mut WorkflowState,
    handle_frame_id: FrameId,
    perform_parent: ParentRef,
    payload: Value,
) -> Result<(), AdvanceError> {
    // Look up the handler ActionId while we have immutable access.
    let handle_frame = workflow_state
        .frames
        .get(handle_frame_id)
        .expect("Handle frame exists");
    let FrameKind::Handle(ref handle) = handle_frame.kind else {
        unreachable!("dispatch_to_handler called on non-Handle frame");
    };
    let handler_action_id = handle.handler;
    let state = handle.state.clone();

    // Assert the Handle is free before suspending.
    assert!(
        matches!(handle.status, HandleStatus::Free),
        "dispatch_to_handler: Handle must be Free, got {:?}",
        handle.status,
    );

    // Mark the Handle as suspended.
    let handle_frame = workflow_state
        .frames
        .get_mut(handle_frame_id)
        .expect("Handle frame exists");
    let FrameKind::Handle(ref mut handle) = handle_frame.kind else {
        unreachable!();
    };
    handle.status = HandleStatus::Suspended(perform_parent);

    // Build the handler input: [payload, state].
    let handler_input = serde_json::json!([payload, state]);

    // Advance the handler DAG.
    super::advance::advance(
        workflow_state,
        handler_action_id,
        handler_input,
        Some(ParentRef::Handle {
            frame_id: handle_frame_id,
            side: HandleSide::Handler,
        }),
    )?;

    Ok(())
}

/// Process the handler's completion value.
#[allow(clippy::expect_used)]
pub fn handle_handler_completion(
    workflow_state: &mut WorkflowState,
    handle_frame_id: FrameId,
    handler_value: Value,
) -> Result<Option<Value>, CompleteError> {
    let handler_output: HandlerOutput = serde_json::from_value(handler_value)?;

    match handler_output {
        HandlerOutput::Resume {
            value,
            state_update,
        } => {
            apply_state_update(&mut workflow_state.frames, handle_frame_id, state_update);
            resume_continuation(workflow_state, handle_frame_id, value)
        }
        HandlerOutput::RestartBody {
            value,
            state_update,
        } => {
            apply_state_update(&mut workflow_state.frames, handle_frame_id, state_update);
            restart_body(workflow_state, handle_frame_id, value)
        }
    }
}

/// Apply a state update to a Handle frame.
#[allow(clippy::expect_used)]
fn apply_state_update(
    frames: &mut Arena<Frame>,
    handle_frame_id: FrameId,
    state_update: StateUpdate,
) {
    if let StateUpdate::Updated { value } = state_update {
        let frame = frames
            .get_mut(handle_frame_id)
            .expect("Handle frame exists");
        let FrameKind::Handle(ref mut handle) = frame.kind else {
            unreachable!("apply_state_update on non-Handle frame");
        };
        handle.state = value;
    }
}

/// Resume the body at the Perform site.
#[allow(clippy::expect_used)]
fn resume_continuation(
    workflow_state: &mut WorkflowState,
    handle_frame_id: FrameId,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    let frame = workflow_state
        .frames
        .get_mut(handle_frame_id)
        .expect("Handle frame exists");
    let FrameKind::Handle(ref mut handle) = frame.kind else {
        unreachable!("resume_continuation on non-Handle frame");
    };
    let HandleStatus::Suspended(perform_parent) = handle.status else {
        unreachable!("resume_continuation: Handle must be Suspended");
    };
    handle.status = HandleStatus::Free;

    match super::complete::try_deliver(workflow_state, perform_parent, value)? {
        TryDeliverResult::Delivered(result) => Ok(result),
        TryDeliverResult::Blocked(value) => {
            workflow_state
                .stashed_items
                .push_back(StashedItem::Delivery {
                    parent_ref: perform_parent,
                    value,
                });
            Ok(None)
        }
        TryDeliverResult::FrameGone => Ok(None),
    }
}

/// Restart the body. Tear down, re-advance from the body action.
#[allow(clippy::expect_used)]
fn restart_body(
    workflow_state: &mut WorkflowState,
    handle_frame_id: FrameId,
    value: Value,
) -> Result<Option<Value>, CompleteError> {
    teardown_body(
        &mut workflow_state.frames,
        &mut workflow_state.task_to_frame,
        handle_frame_id,
    );
    let frame = workflow_state
        .frames
        .get_mut(handle_frame_id)
        .expect("Handle frame exists");
    let FrameKind::Handle(ref mut handle) = frame.kind else {
        unreachable!("restart_body on non-Handle frame");
    };
    let body_action_id = handle.body;
    handle.status = HandleStatus::Free;
    super::advance::advance(
        workflow_state,
        body_action_id,
        value,
        Some(ParentRef::Handle {
            frame_id: handle_frame_id,
            side: HandleSide::Body,
        }),
    )?;
    Ok(None)
}

/// Remove all frames that are descendants of the given Handle's body.
fn teardown_body(
    frames: &mut Arena<Frame>,
    task_to_frame: &mut BTreeMap<TaskId, FrameId>,
    handle_frame_id: FrameId,
) {
    // Collect frame IDs to remove (can't mutate arena while iterating).
    let frames_ref = &*frames;
    let to_remove: Vec<FrameId> = frames_ref
        .iter()
        .filter_map(|(frame_id, _)| {
            if is_descendant_of_body(frames_ref, frame_id, handle_frame_id) {
                Some(frame_id)
            } else {
                None
            }
        })
        .collect();

    for frame_id in &to_remove {
        frames.remove(*frame_id);
    }

    // Remove task_to_frame entries whose Invoke frame was torn down.
    task_to_frame.retain(|_, frame_id| !to_remove.contains(frame_id));
}

/// Is `frame_id` a descendant of the given Handle's body side?
fn is_descendant_of_body(
    frames: &Arena<Frame>,
    frame_id: FrameId,
    handle_frame_id: FrameId,
) -> bool {
    let mut current_id = frame_id;
    loop {
        let Some(frame) = frames.get(current_id) else {
            return false;
        };
        let Some(parent_ref) = frame.parent else {
            return false;
        };
        if parent_ref.frame_id() == handle_frame_id {
            // This frame's parent is the Handle. Check if it's on the body side.
            return matches!(
                parent_ref,
                ParentRef::Handle {
                    side: HandleSide::Body,
                    ..
                }
            );
        }
        current_id = parent_ref.frame_id();
    }
}
