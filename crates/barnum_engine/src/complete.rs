use serde_json::Value;

use super::frame::{FrameKind, HandleSide, ParentRef, ResumeHandleFrame};
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

        ParentRef::ResumeHandle { frame_id } => {
            // Body completed. Remove the ResumeHandle frame and deliver
            // the value to the ResumeHandle's parent.
            let frame = workflow_state
                .frames
                .remove(frame_id)
                .expect("parent frame exists");
            deliver(workflow_state, frame.parent, value)
        }

        ParentRef::ResumePerform { frame_id } => {
            // Handler completed. Destructure [value, new_state], write
            // state back to ResumeHandle, deliver value upward.
            let frame = workflow_state
                .frames
                .remove(frame_id)
                .expect("ResumePerform frame exists");
            let FrameKind::ResumePerform(resume_perform) = frame.kind else {
                unreachable!("ResumePerform ParentRef points to non-ResumePerform frame");
            };
            let (result_value, new_state): (Value, Value) = serde_json::from_value(value)
                .map_err(|source| CompleteError::InvalidHandlerOutput { source })?;

            // Write state back to the ResumeHandle frame.
            let resume_handle_frame = workflow_state
                .frames
                .get_mut(resume_perform.resume_handle_frame_id)
                .expect("ResumeHandle frame exists");
            let FrameKind::ResumeHandle(ResumeHandleFrame { ref mut state, .. }) =
                resume_handle_frame.kind
            else {
                unreachable!("resume_handle_frame_id points to non-ResumeHandle frame");
            };
            *state = new_state;

            // Deliver the value to the Perform site's parent.
            deliver(workflow_state, frame.parent, result_value)
        }

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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::test_helpers::*;
    use serde_json::json;

    /// Chain(A, B): complete A -> dispatches B. Complete B -> workflow done.
    #[test]
    fn chain_trampolines_on_completion() {
        let mut engine = engine_from(chain(invoke("./a.ts", "a"), invoke("./b.ts", "b")));
        let root = engine.workflow_root();
        engine.advance(root, json!(null), None).unwrap();

        let d1 = engine.take_pending_dispatches();
        assert_eq!(d1.len(), 1);

        let result = engine.complete(d1[0].task_id, json!("a_result")).unwrap();
        assert_eq!(result, None);

        let d2 = engine.take_pending_dispatches();
        assert_eq!(d2.len(), 1);
        assert_eq!(d2[0].value, json!("a_result"));

        let result = engine.complete(d2[0].task_id, json!("b_result")).unwrap();
        assert_eq!(result, Some(json!("b_result")));
    }

    /// Deep chain: Chain(A, Chain(B, C)) -> A -> B -> C -> done.
    #[test]
    fn nested_chain_completes() {
        let mut engine = engine_from(chain(
            invoke("./a.ts", "a"),
            chain(invoke("./b.ts", "b"), invoke("./c.ts", "c")),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // A
        let d = engine.take_pending_dispatches();
        assert_eq!(engine.complete(d[0].task_id, json!("a_out")).unwrap(), None);
        // B
        let d = engine.take_pending_dispatches();
        assert_eq!(d[0].value, json!("a_out"));
        assert_eq!(engine.complete(d[0].task_id, json!("b_out")).unwrap(), None);
        // C
        let d = engine.take_pending_dispatches();
        assert_eq!(d[0].value, json!("b_out"));
        assert_eq!(
            engine.complete(d[0].task_id, json!("c_out")).unwrap(),
            Some(json!("c_out")),
        );
    }

    /// `All(A, B)`: complete both -> workflow done with collected results.
    #[test]
    fn parallel_collects_results() {
        let mut engine = engine_from(parallel(vec![invoke("./a.ts", "a"), invoke("./b.ts", "b")]));
        let root = engine.workflow_root();
        engine.advance(root, json!(null), None).unwrap();

        let d = engine.take_pending_dispatches();
        assert_eq!(d.len(), 2);

        // Complete in reverse order to verify index-based collection.
        assert_eq!(
            engine.complete(d[1].task_id, json!("b_result")).unwrap(),
            None,
        );
        assert_eq!(
            engine.complete(d[0].task_id, json!("a_result")).unwrap(),
            Some(json!(["a_result", "b_result"])),
        );
    }

    /// `ForEach` over array: complete both -> collected results.
    #[test]
    fn foreach_collects_results() {
        let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
        let root = engine.workflow_root();
        engine.advance(root, json!([10, 20]), None).unwrap();

        let d = engine.take_pending_dispatches();
        assert_eq!(d.len(), 2);

        assert_eq!(engine.complete(d[0].task_id, json!("r10")).unwrap(), None);
        assert_eq!(
            engine.complete(d[1].task_id, json!("r20")).unwrap(),
            Some(json!(["r10", "r20"])),
        );
    }
}
