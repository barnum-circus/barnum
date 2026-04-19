use barnum_ast::flat::{ActionId, FlatAction};
use intern::Lookup;
use serde_json::Value;

use super::frame::{
    Frame, FrameKind, ParentRef, RestartHandleFrame, RestartHandleSide, ResumeHandleFrame,
};
use super::{AdvanceError, DispatchEvent, PendingEffectKind, WorkflowState};

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
            workflow_state.pending_effects.push_back((
                frame_id,
                PendingEffectKind::Dispatch(DispatchEvent {
                    task_id,
                    handler_id: handler,
                    value,
                }),
            ));
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
                workflow_state.terminal_value =
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
                workflow_state.terminal_value =
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
            // Strip namespaced enum prefix: "Result.Ok" → "Ok"
            let match_str = kind_str
                .rsplit_once('.')
                .map_or(kind_str, |(_, suffix)| suffix);
            let (_, case_action_id) = workflow_state
                .flat_config
                .branch_cases(action_id)
                .find(|(key, _)| key.lookup() == match_str)
                .ok_or_else(|| AdvanceError::BranchNoMatch {
                    kind: kind_str.to_owned(),
                })?;
            advance(workflow_state, case_action_id, value, parent)?;
        }

        FlatAction::ResumeHandle { resume_handler_id } => {
            let body = workflow_state.flat_config.resume_handle_body(action_id);
            let handler = workflow_state.flat_config.resume_handle_handler(action_id);
            let frame_id = workflow_state.insert_frame(Frame {
                parent,
                kind: FrameKind::ResumeHandle(ResumeHandleFrame {
                    resume_handler_id,
                    body,
                    handler,
                    state: value.clone(),
                }),
            });
            advance(
                workflow_state,
                body,
                value,
                Some(ParentRef::ResumeHandle { frame_id }),
            )?;
        }

        FlatAction::ResumePerform { resume_handler_id } => {
            let parent = parent.ok_or(AdvanceError::UnhandledResumeEffect { resume_handler_id })?;
            super::effects::bubble_resume_effect(workflow_state, parent, resume_handler_id, value)?;
        }

        FlatAction::RestartHandle { restart_handler_id } => {
            let body = workflow_state.flat_config.restart_handle_body(action_id);
            let handler = workflow_state.flat_config.restart_handle_handler(action_id);
            let frame_id = workflow_state.insert_frame(Frame {
                parent,
                kind: FrameKind::RestartHandle(RestartHandleFrame {
                    restart_handler_id,
                    body,
                    handler,
                    state: value.clone(),
                }),
            });
            advance(
                workflow_state,
                body,
                value,
                Some(ParentRef::RestartHandle {
                    frame_id,
                    side: RestartHandleSide::Body,
                }),
            )?;
        }

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

            // Marker frame for liveness tracking. Lives in the body subtree,
            // so teardown_body removes it.
            let marker_frame_id = workflow_state.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::RestartPerformMarker,
            });

            workflow_state.pending_effects.push_back((
                marker_frame_id,
                PendingEffectKind::Restart(super::RestartEvent {
                    restart_handle_frame_id,
                    payload: value,
                }),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::doc_markdown, clippy::unwrap_used)]
mod tests {
    use crate::test_helpers::*;
    use barnum_ast::*;
    use serde_json::json;

    /// Single invoke: advance -> 1 dispatch.
    #[test]
    fn single_invoke() {
        let mut engine = engine_from(invoke("./handler.ts", "run"));
        let root = engine.workflow_root();
        super::advance(&mut engine, root, json!({"x": 1}), None).unwrap();

        let dispatch = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(dispatch.value, json!({"x": 1}));
        assert_eq!(
            engine.handler(dispatch.handler_id),
            &ts_handler("./handler.ts", "run"),
        );
    }

    /// Chain(A, B): only A is dispatched on advance.
    #[test]
    fn chain_dispatches_first_only() {
        let mut engine = engine_from(chain(invoke("./a.ts", "a"), invoke("./b.ts", "b")));
        let root = engine.workflow_root();
        super::advance(&mut engine, root, json!(null), None).unwrap();

        let dispatch = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(
            engine.handler(dispatch.handler_id),
            &ts_handler("./a.ts", "a"),
        );
    }

    /// All(A, B, C): all 3 dispatched on advance, all receive the same
    /// input.
    #[test]
    fn parallel_dispatches_all() {
        let mut engine = engine_from(parallel(vec![
            invoke("./a.ts", "a"),
            invoke("./b.ts", "b"),
            invoke("./c.ts", "c"),
        ]));
        let root = engine.workflow_root();
        super::advance(&mut engine, root, json!({"shared": true}), None).unwrap();

        let a_dispatch = pop_dispatch(&mut engine).unwrap();
        let b_dispatch = pop_dispatch(&mut engine).unwrap();
        let c_dispatch = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(a_dispatch.value, json!({"shared": true}));
        assert_eq!(b_dispatch.value, json!({"shared": true}));
        assert_eq!(c_dispatch.value, json!({"shared": true}));
    }

    /// `ForEach` over 3-element array: 3 dispatches, one per element.
    #[test]
    fn foreach_dispatches_per_element() {
        let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
        let root = engine.workflow_root();
        super::advance(&mut engine, root, json!([10, 20, 30]), None).unwrap();

        let d0 = pop_dispatch(&mut engine).unwrap();
        let d1 = pop_dispatch(&mut engine).unwrap();
        let d2 = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(d0.value, json!(10));
        assert_eq!(d1.value, json!(20));
        assert_eq!(d2.value, json!(30));
    }

    /// Branch: only the matching case is dispatched.
    #[test]
    fn branch_dispatches_matching_case() {
        let mut engine = engine_from(branch(vec![
            ("Ok", invoke("./ok.ts", "handle")),
            ("Err", invoke("./err.ts", "handle")),
        ]));
        let root = engine.workflow_root();
        super::advance(
            &mut engine,
            root,
            json!({"kind": "Result.Ok", "value": 42}),
            None,
        )
        .unwrap();

        let dispatch = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(
            engine.handler(dispatch.handler_id),
            &ts_handler("./ok.ts", "handle"),
        );
    }

    /// Nested: Chain inside All. All(Chain(A, B), C) -> dispatches A
    /// and C.
    #[test]
    fn nested_chain_in_parallel() {
        let mut engine = engine_from(parallel(vec![
            chain(invoke("./a.ts", "a"), invoke("./b.ts", "b")),
            invoke("./c.ts", "c"),
        ]));
        let root = engine.workflow_root();
        super::advance(&mut engine, root, json!(null), None).unwrap();

        let a_dispatch = pop_dispatch(&mut engine).unwrap();
        let c_dispatch = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());
        let handlers = [
            engine.handler(a_dispatch.handler_id).clone(),
            engine.handler(c_dispatch.handler_id).clone(),
        ];
        assert!(handlers.contains(&ts_handler("./a.ts", "a")));
        assert!(handlers.contains(&ts_handler("./c.ts", "c")));
        // B is not dispatched yet (behind Chain).
        assert!(!handlers.contains(&ts_handler("./b.ts", "b")));
    }

    /// Deep chain: Chain(A, Chain(B, C)) -> only A dispatched.
    #[test]
    fn deep_chain_dispatches_first_only() {
        let mut engine = engine_from(chain(
            invoke("./a.ts", "a"),
            chain(invoke("./b.ts", "b"), invoke("./c.ts", "c")),
        ));
        let root = engine.workflow_root();
        super::advance(&mut engine, root, json!(null), None).unwrap();

        let dispatch = pop_dispatch(&mut engine).unwrap();
        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(
            engine.handler(dispatch.handler_id),
            &ts_handler("./a.ts", "a"),
        );
    }

    /// `ForEach` with empty array: no dispatches, immediate completion.
    #[test]
    fn foreach_empty_array() {
        let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
        let root = engine.workflow_root();
        super::advance(&mut engine, root, json!([]), None).unwrap();

        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(engine.take_terminal_value(), Some(json!([])));
    }

    /// All with empty children: no dispatches, immediate completion.
    #[test]
    fn parallel_empty() {
        let mut engine = engine_from(parallel(vec![]));
        let root = engine.workflow_root();
        super::advance(&mut engine, root, json!(null), None).unwrap();

        assert!(pop_dispatch(&mut engine).is_none());
        assert_eq!(engine.take_terminal_value(), Some(json!([])));
    }

    // Bare RestartPerform with no enclosing RestartHandle → UnhandledRestartEffect.
    #[test]
    fn restart_perform_without_handle_errors() {
        let mut engine = engine_from(restart_perform(1));
        let root = engine.workflow_root();
        let err = super::advance(&mut engine, root, json!(null), None).unwrap_err();
        assert!(
            matches!(err, crate::AdvanceError::UnhandledRestartEffect { restart_handler_id } if restart_handler_id == RestartHandlerId(1)),
            "expected UnhandledRestartEffect, got: {err:?}",
        );
    }

    // Bare ResumePerform with no enclosing ResumeHandle → UnhandledResumeEffect.
    #[test]
    fn resume_perform_without_handle_errors() {
        let mut engine = engine_from(resume_perform(1));
        let root = engine.workflow_root();
        let err = super::advance(&mut engine, root, json!(null), None).unwrap_err();
        assert!(
            matches!(err, crate::AdvanceError::UnhandledResumeEffect { resume_handler_id } if resume_handler_id == ResumeHandlerId(1)),
            "expected UnhandledResumeEffect, got: {err:?}",
        );
    }
}
