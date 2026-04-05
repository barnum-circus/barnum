use barnum_ast::flat::{ActionId, FlatAction};
use intern::Lookup;
use serde_json::Value;

use super::frame::{Frame, FrameKind, HandleFrame, HandleSide, HandleStatus, ParentRef};
use super::{AdvanceError, Dispatch, StashOutcome, StashedItem, WorkflowState};

/// Expand an `ActionId` into frames. Creates frames for structural
/// combinators and Invoke leaves. Invoke frames hold the parent pointer
/// and handler ID; they're removed when the task completes.
///
/// Pass `parent: None` for the top-level action (i.e., starting a
/// workflow). Internal recursion provides `Some(parent_ref)` to attach
/// child frames to their parent.
///
/// # Errors
///
/// Returns [`AdvanceError`] if the workflow encounters a structural error
/// during expansion (e.g., `ForEach` on a non-array, `Branch` with no
/// matching case).
#[allow(
    clippy::too_many_lines,
    clippy::missing_panics_doc,
    clippy::expect_used
)]
pub fn advance(
    workflow_state: &mut WorkflowState,
    action_id: ActionId,
    value: Value,
    parent: Option<ParentRef>,
) -> Result<(), AdvanceError> {
    match workflow_state.flat_config.action(action_id) {
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

        FlatAction::Chain { rest } => {
            let first = workflow_state.flat_config.chain_first(action_id);
            let frame_id = workflow_state.insert_frame(Frame {
                parent,
                kind: FrameKind::Chain { rest },
            });
            advance(
                workflow_state,
                first,
                value,
                Some(ParentRef::Chain { frame_id }),
            )?;
        }

        FlatAction::All { count } => {
            if count.0 == 0 {
                // No children — vacuously complete with empty array.
                super::complete::deliver(workflow_state, parent, Value::Array(vec![]))
                    .expect("vacuous empty-parallel completion should not fail");
                return Ok(());
            }
            // Collect to a Vec to release the immutable borrow on
            // flat_config before the mutable advance() calls.
            #[allow(clippy::needless_collect)]
            let children: Vec<ActionId> = workflow_state
                .flat_config
                .parallel_children(action_id)
                .collect();
            let frame_id = workflow_state.insert_frame(Frame {
                parent,
                kind: FrameKind::All {
                    results: vec![None; count.0 as usize],
                },
            });
            for (i, child) in children.into_iter().enumerate() {
                advance(
                    workflow_state,
                    child,
                    value.clone(),
                    Some(ParentRef::All {
                        frame_id,
                        child_index: i,
                    }),
                )?;
            }
        }

        FlatAction::ForEach { body } => {
            let elements = match value {
                Value::Array(elements) => elements,
                other => {
                    return Err(AdvanceError::ForEachExpectedArray { value: other });
                }
            };
            if elements.is_empty() {
                // No elements — vacuously complete with empty array.
                super::complete::deliver(workflow_state, parent, Value::Array(vec![]))
                    .expect("vacuous empty-foreach completion should not fail");
                return Ok(());
            }
            let frame_id = workflow_state.insert_frame(Frame {
                parent,
                kind: FrameKind::ForEach {
                    results: vec![None; elements.len()],
                },
            });
            for (i, element) in elements.into_iter().enumerate() {
                advance(
                    workflow_state,
                    body,
                    element,
                    Some(ParentRef::ForEach {
                        frame_id,
                        child_index: i,
                    }),
                )?;
            }
        }

        FlatAction::Branch { .. } => {
            let kind_str =
                value["kind"]
                    .as_str()
                    .ok_or_else(|| AdvanceError::BranchMissingKind {
                        value: value.clone(),
                    })?;
            let (_, case_action_id) = workflow_state
                .flat_config
                .branch_cases(action_id)
                .find(|(key, _)| key.lookup() == kind_str)
                .ok_or_else(|| AdvanceError::BranchNoMatch {
                    kind: kind_str.to_owned(),
                })?;
            advance(workflow_state, case_action_id, value, parent)?;
        }

        FlatAction::Step { target } => {
            advance(workflow_state, target, value, parent)?;
        }

        FlatAction::Handle { effect_id } => {
            let body = workflow_state.flat_config.handle_body(action_id);
            let handler = workflow_state.flat_config.handle_handler(action_id);
            let frame_id = workflow_state.insert_frame(Frame {
                parent,
                kind: FrameKind::Handle(HandleFrame {
                    effect_id,
                    body,
                    handler,
                    state: value.clone(),
                    status: HandleStatus::Free,
                }),
            });
            advance(
                workflow_state,
                body,
                value,
                Some(ParentRef::Handle {
                    frame_id,
                    side: HandleSide::Body,
                }),
            )?;
        }

        FlatAction::Perform { effect_id } => {
            let parent = parent.ok_or(AdvanceError::UnhandledEffect { effect_id })?;
            match super::effects::bubble_effect(workflow_state, parent, effect_id, value)? {
                StashOutcome::Consumed => {}
                StashOutcome::Blocked(payload) => {
                    workflow_state.stashed_items.push_back(StashedItem::Effect {
                        starting_parent: parent,
                        effect_id,
                        payload,
                    });
                }
            }
        }
    }
    Ok(())
}
