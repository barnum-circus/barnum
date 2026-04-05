use serde_json::Value;

use super::frame::{FrameKind, ParentRef, RestartHandleSide, ResumeHandleFrame};
use super::{CompleteError, WorkflowState};

/// Deliver a task result. The caller invokes this when a dispatched
/// handler finishes.
///
/// Returns `Ok(Some(value))` when the workflow terminates, `Ok(None)`
/// when it's still running.
///
/// # Errors
///
/// Returns [`CompleteError`] if the result value has an invalid shape,
/// or if an advance error occurs during Chain trampoline or `RestartHandle`
/// re-entry.
///
/// # Panics
///
/// Panics if `task_id` is not a known pending task.
#[allow(clippy::expect_used)]
pub fn complete(
    workflow_state: &mut WorkflowState,
    completion_event: super::CompletionEvent,
) -> Result<Option<Value>, CompleteError> {
    let super::CompletionEvent { task_id, value } = completion_event;
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
    match frame.parent {
        Some(parent_ref) => deliver(workflow_state, Some(parent_ref), value),
        None => Ok(Some(value)),
    }
}

/// Deliver a value to the parent that was waiting for it.
///
/// - **No parent:** workflow done — return the terminal value.
/// - **Chain:** trampoline — advance the `rest` action with the value.
/// - **All/ForEach:** store in results slot; if all slots filled,
///   collect into array and deliver to parent.
/// - **`ResumeHandle`:** body completed, deliver to parent.
/// - **`ResumePerform`:** destructure `[value, new_state]`, write state back.
/// - **`RestartHandle`:** body completed or handler completed (re-advance body).
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]
pub(crate) fn deliver(
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
                    unreachable!("RestartHandle ParentRef points to non-RestartHandle frame");
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
    use crate::CompletionEvent;
    use crate::test_helpers::*;
    use serde_json::json;

    /// Chain(A, B): complete A -> dispatches B. Complete B -> workflow done.
    #[test]
    fn chain_trampolines_on_completion() {
        let mut engine = engine_from(chain(invoke("./a.ts", "a"), invoke("./b.ts", "b")));
        let root = engine.workflow_root();
        crate::advance::advance(&mut engine, root, json!(null), None).unwrap();

        let d1 = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());

        let result = super::complete(
            &mut engine,
            CompletionEvent {
                task_id: d1.task_id,
                value: json!("a_result"),
            },
        )
        .unwrap();
        assert_eq!(result, None);

        let d2 = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(d2.value, json!("a_result"));

        let result = super::complete(
            &mut engine,
            CompletionEvent {
                task_id: d2.task_id,
                value: json!("b_result"),
            },
        )
        .unwrap();
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
        crate::advance::advance(&mut engine, root, json!("input"), None).unwrap();

        // A
        let d = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(
            super::complete(
                &mut engine,
                CompletionEvent {
                    task_id: d.task_id,
                    value: json!("a_out")
                }
            )
            .unwrap(),
            None
        );
        // B
        let d = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(d.value, json!("a_out"));
        assert_eq!(
            super::complete(
                &mut engine,
                CompletionEvent {
                    task_id: d.task_id,
                    value: json!("b_out")
                }
            )
            .unwrap(),
            None
        );
        // C
        let d = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(d.value, json!("b_out"));
        assert_eq!(
            super::complete(
                &mut engine,
                CompletionEvent {
                    task_id: d.task_id,
                    value: json!("c_out")
                }
            )
            .unwrap(),
            Some(json!("c_out")),
        );
    }

    /// `All(A, B)`: complete both -> workflow done with collected results.
    #[test]
    fn parallel_collects_results() {
        let mut engine = engine_from(parallel(vec![invoke("./a.ts", "a"), invoke("./b.ts", "b")]));
        let root = engine.workflow_root();
        crate::advance::advance(&mut engine, root, json!(null), None).unwrap();

        let a_dispatch = pop_dispatch(&mut engine).unwrap();
        let b_dispatch = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());

        // Complete in reverse order to verify index-based collection.
        assert_eq!(
            super::complete(
                &mut engine,
                CompletionEvent {
                    task_id: b_dispatch.task_id,
                    value: json!("b_result")
                }
            )
            .unwrap(),
            None,
        );
        assert_eq!(
            super::complete(
                &mut engine,
                CompletionEvent {
                    task_id: a_dispatch.task_id,
                    value: json!("a_result")
                }
            )
            .unwrap(),
            Some(json!(["a_result", "b_result"])),
        );
    }

    /// `ForEach` over array: complete both -> collected results.
    #[test]
    fn foreach_collects_results() {
        let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
        let root = engine.workflow_root();
        crate::advance::advance(&mut engine, root, json!([10, 20]), None).unwrap();

        let d0 = pop_dispatch(&mut engine).unwrap();
        let d1 = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());

        assert_eq!(
            super::complete(
                &mut engine,
                CompletionEvent {
                    task_id: d0.task_id,
                    value: json!("r10")
                }
            )
            .unwrap(),
            None
        );
        assert_eq!(
            super::complete(
                &mut engine,
                CompletionEvent {
                    task_id: d1.task_id,
                    value: json!("r20")
                }
            )
            .unwrap(),
            Some(json!(["r10", "r20"])),
        );
    }
}
