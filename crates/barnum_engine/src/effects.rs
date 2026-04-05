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

#[cfg(test)]
#[allow(clippy::doc_markdown, clippy::unwrap_used)]
mod tests {
    use crate::test_helpers::*;
    use barnum_ast::*;
    use serde_json::json;

    // -- Handle / Perform tests --

    // Test 2: Handle(e, always_resume(42), Chain(Perform(e), Invoke(echo))).
    /// Perform fires, handler resumes with 42, Chain trampolines to echo with 42.
    #[test]
    fn handle_resume_chains_to_next() {
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!(42)),
            chain(perform(1), invoke("./echo.ts", "echo")),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Perform fires synchronously during advance. The handler DAG
        // (Constant) produces a dispatch. Drive builtins to process it.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        // Chain trampolines to echo with the resumed value.
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!(42));

        // Complete echo.
        let result = engine
            .complete(ts[0].task_id, json!("echo_result"))
            .unwrap();
        assert_eq!(result, Some(json!("echo_result")));
    }

    /// Test 3: restart+Branch where Break+Perform fires before Invoke(unreachable).
    /// Handler restarts body, Branch takes Break arm, Invoke never runs.
    #[test]
    fn restart_branch_break_skips_rest_of_chain() {
        let mut engine = engine_from(restart_branch(
            1,
            chain(break_perform(1), invoke("./unreachable.ts", "nope")),
            identity_action(),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, Some(json!("input")));
        // No TypeScript dispatches — unreachable invoke never ran.
        assert!(ts.is_empty());
    }

    /// Test 5: Perform(outer) skips inner Handle, caught by outer.
    /// Handle(outer, h_outer, Handle(inner, h_inner, Perform(outer))).
    #[test]
    fn perform_skips_non_matching_handle() {
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!("outer_handled")),
            handle(
                2,
                always_resume_handler(json!("inner_handled")),
                perform(1), // effect_id=1, skips inner (effect_id=2)
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        // Outer handler resumed, inner Handle's body gets the value.
        // But Perform(1) is the entire inner body, so delivery goes to
        // Handle(inner)'s parent = Handle(outer)'s body.
        // Inner Handle exits with "outer_handled", then outer Handle exits.
        assert_eq!(result, Some(json!("outer_handled")));
        assert!(ts.is_empty());
    }

    /// Test 6: Perform is first half of Chain. After Resume(V), Chain rest receives V.
    #[test]
    fn perform_in_chain_first_resumes_to_rest() {
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!("resumed_value")),
            chain(perform(1), invoke("./next.ts", "next")),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("resumed_value"));

        let result = engine.complete(ts[0].task_id, json!("final")).unwrap();
        assert_eq!(result, Some(json!("final")));
    }

    /// Test 7: One branch of All Performs, the other completes normally.
    /// After Resume, All joins both.
    #[test]
    fn handle_all_one_performs_one_normal() {
        // Handle(e, echo_resume, All(Chain(Invoke(A), Perform(e)), Invoke(B)))
        let mut engine = engine_from(handle(
            1,
            echo_resume_handler(),
            parallel(vec![
                chain(invoke("./a.ts", "a"), perform(1)),
                invoke("./b.ts", "b"),
            ]),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 2); // A and B dispatched

        // Complete A -> Chain trampolines to Perform -> handler dispatched.
        let (result, ts2) = complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, None);
        // echo_resume echoes "a_out" back. Resume delivers to All slot 0.
        assert!(ts2.is_empty());

        // Complete B -> All slot 1.
        let result = engine.complete(ts[1].task_id, json!("b_out")).unwrap();
        // All joins with [resumed_value, "b_out"].
        assert_eq!(result, Some(json!(["a_out", "b_out"])));
    }

    /// Test 8: After restart+Branch Break, body frames removed from arena, task_to_parent
    /// entries removed. Chain(Invoke(A), break_perform) in Continue arm.
    #[test]
    fn restart_branch_break_cleans_up_frames_and_tasks() {
        let mut engine = engine_from(restart_branch(
            1,
            chain(invoke("./a.ts", "a"), break_perform(1)),
            identity_action(),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // A dispatched.
        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 1);

        // Complete A → break_perform → handler restarts → Branch(Break) → identity → exits.
        let (result, _) = complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, Some(json!("a_out")));

        // Verify frames are empty.
        assert_eq!(engine.frames.len(), 0);
        assert!(engine.task_to_frame.is_empty());
    }

    /// Test 9: Body Performs twice in a Chain. First resumed, then second fires.
    #[test]
    fn two_performs_in_chain() {
        // Handle(e, echo_resume, Chain(Perform(e), Chain(Invoke(mid), Perform(e))))
        let mut engine = engine_from(handle(
            1,
            echo_resume_handler(),
            chain(perform(1), chain(invoke("./mid.ts", "mid"), perform(1))),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // First Perform fires during advance. Drive builtins for echo_resume handler.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        // Handler echoed "input" back. Chain trampolines to Chain(mid, Perform).
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("input"));

        // Complete mid -> inner Chain trampolines to second Perform -> handler ->
        // Resume with "mid_out" -> deliver to Handle{Body} -> body done -> Handle exits.
        let (result, ts2) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("mid_out")).unwrap();
        assert_eq!(result, Some(json!("mid_out")));
        assert!(ts2.is_empty());
    }

    /// Test 10: RestartBody multiple times via Continue, then exit via Break.
    /// Uses restart+Branch pattern. Body step returns pre-tagged Continue or Break values.
    #[test]
    fn restart_branch_multiple_then_break() {
        // Continue arm: Invoke(step) → Perform(1).
        // Step returns { kind: "Continue", value: ... } or { kind: "Break", value: ... }.
        // The Perform payload is the pre-tagged value; handler extracts and restarts.
        // Branch dispatches on the tag.
        let mut engine = engine_from(restart_branch(
            1,
            chain(invoke("./body.ts", "step"), perform(1)),
            identity_action(),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("init"), None).unwrap();

        // drive_builtins processes the Tag("Continue"), Branch dispatch builtins,
        // and returns the TypeScript step Invoke dispatch.
        let (_, mut body_dispatches) = drive_builtins(&mut engine).unwrap();
        assert_eq!(body_dispatches.len(), 1);

        let frame_count_before = engine.frames.len();

        for _ in 0..3 {
            // Complete step with Continue-tagged value → Perform → handler restarts →
            // Branch(Continue) → re-enter body. complete_and_drive drives all builtins
            // and returns the new TypeScript step dispatch.
            let (result, new_dispatches) = complete_and_drive(
                &mut engine,
                body_dispatches[0].task_id,
                json!({"kind": "Continue", "value": "restarted"}),
            )
            .unwrap();
            assert_eq!(result, None);

            body_dispatches = new_dispatches;
            assert_eq!(body_dispatches.len(), 1);
            assert_eq!(body_dispatches[0].value, json!("restarted"));
        }

        // Frames shouldn't grow across restarts.
        assert!(
            engine.frames.len() <= frame_count_before,
            "arena grew: {} -> {}",
            frame_count_before,
            engine.frames.len(),
        );

        // Final iteration: Break instead of Continue.
        let (result, _) = complete_and_drive(
            &mut engine,
            body_dispatches[0].task_id,
            json!({"kind": "Break", "value": "gave_up"}),
        )
        .unwrap();
        assert_eq!(result, Some(json!("gave_up")));
        assert_eq!(engine.frames.len(), 0);
    }

    /// Test 14: Body runs without Performing. Handle exits normally.
    #[test]
    fn handle_body_no_perform_exits_normally() {
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!("unused")),
            invoke("./body.ts", "run"),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 1); // body invoke dispatched

        let result = engine
            .complete(ts[0].task_id, json!("body_result"))
            .unwrap();
        // Body done, no Perform, Handle exits with body result.
        assert_eq!(result, Some(json!("body_result")));
    }

    /// Test 15: Stash scenario — completion during suspension.
    /// Handle(e, echo_resume, All(Chain(Invoke(A), Perform(e)), Invoke(B)))
    /// Complete A -> Perform -> handler. Complete B -> stashed. Handler resumes -> sweep -> B delivered -> All joins.
    #[test]
    fn stash_delivery_during_suspension() {
        let mut engine = engine_from(handle(
            1,
            invoke("./handler.ts", "handler"),
            parallel(vec![
                chain(invoke("./a.ts", "a"), perform(1)),
                invoke("./b.ts", "b"),
            ]),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 2); // A and B

        // Complete A -> Chain trampolines to Perform -> handler dispatched.
        let (result, handler_ts) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, None);
        assert_eq!(handler_ts.len(), 1); // handler TS dispatch

        // Complete B while handler is running -> stashed.
        let result = engine.complete(ts[1].task_id, json!("b_out")).unwrap();
        assert_eq!(result, None);
        assert_eq!(engine.stashed_items.len(), 1);

        // Complete handler with Resume.
        let (result, _) = complete_and_drive(
            &mut engine,
            handler_ts[0].task_id,
            json!({"kind": "Resume", "value": "a_resumed"}),
        )
        .unwrap();
        // Resume delivers to All slot 0. sweep_stash delivers B to All slot 1. All joins.
        assert_eq!(result, Some(json!(["a_resumed", "b_out"])));
        assert!(engine.stashed_items.is_empty());
    }

    /// Test 16: restart+Branch with concurrent tasks. A completes → break_perform → restart →
    /// Branch(Break) → exits. B's in-flight task is torn down with the body.
    #[test]
    fn teardown_cleans_up_concurrent_tasks() {
        let mut engine = engine_from(restart_branch(
            1,
            parallel(vec![
                chain(invoke("./a.ts", "a"), break_perform(1)),
                invoke("./b.ts", "b"),
            ]),
            identity_action(),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 2);

        // Complete A → break_perform → handler (builtin) → RestartBody → Branch(Break) → exits.
        // B's task_to_frame entry is cleaned up during body teardown.
        let (result, _) = complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, Some(json!("a_out")));
        assert_eq!(engine.frames.len(), 0);
        assert!(engine.task_to_frame.is_empty());
    }

    /// Completing a task that was torn down during body teardown should
    /// return Ok(None), not panic.
    #[test]
    #[should_panic(expected = "unknown task")]
    fn completing_torn_down_task_is_noop() {
        let mut engine = engine_from(restart_branch(
            1,
            parallel(vec![
                chain(invoke("./a.ts", "a"), break_perform(1)),
                invoke("./b.ts", "b"),
            ]),
            identity_action(),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 2);
        let b_task_id = ts[1].task_id;

        // Complete A → teardown tears down B's task.
        let (result, _) = complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, Some(json!("a_out")));

        // B's handler completes after teardown. Should be Ok(None).
        let result = engine.complete(b_task_id, json!("b_out")).unwrap();
        assert_eq!(result, None);
    }

    /// Test 22: All(Perform(e), Perform(e)) — concurrent Performs.
    /// First dispatches handler, second stashed. After resume, sweep retries.
    #[test]
    fn concurrent_performs_serialized() {
        let mut engine = engine_from(handle(
            1,
            invoke("./handler.ts", "handler"),
            parallel(vec![perform(1), perform(1)]),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // During advance: All advances both Performs.
        // First Perform: bubble_effect -> dispatch handler (Handle becomes Suspended).
        // Second Perform: find_blocking_ancestor -> Handle busy -> stashed as Effect.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1); // handler dispatch for first Perform
        assert_eq!(engine.stashed_items.len(), 1); // second Perform stashed

        // Complete first handler with Resume.
        let (result, handler2_ts) = complete_and_drive(
            &mut engine,
            ts[0].task_id,
            json!({"kind": "Resume", "value": "first_resumed"}),
        )
        .unwrap();
        // Resume delivers to All slot 0. sweep_stash retries second effect ->
        // Handle free -> dispatches second handler.
        assert_eq!(result, None);
        assert_eq!(handler2_ts.len(), 1);
        assert!(engine.stashed_items.is_empty());

        // Complete second handler.
        let (result, _) = complete_and_drive(
            &mut engine,
            handler2_ts[0].task_id,
            json!({"kind": "Resume", "value": "second_resumed"}),
        )
        .unwrap();
        assert_eq!(result, Some(json!(["first_resumed", "second_resumed"])),);
    }

    /// Test 24: Handler returns garbage output → InvalidHandlerOutput error.
    #[test]
    fn garbage_handler_output_errors() {
        let mut engine = engine_from(handle(1, garbage_output_handler(), perform(1)));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Handler Constant returns { kind: "Unknown" }.
        let err = drive_builtins(&mut engine).unwrap_err();
        assert!(
            matches!(err, crate::CompleteError::InvalidHandlerOutput { .. }),
            "expected InvalidHandlerOutput, got: {err:?}",
        );
    }

    /// Test 25: Handler returns { kind: "Resume" } with missing value → InvalidHandlerOutput.
    #[test]
    fn missing_fields_handler_output_errors() {
        let mut engine = engine_from(handle(1, missing_fields_handler(), perform(1)));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let err = drive_builtins(&mut engine).unwrap_err();
        assert!(
            matches!(err, crate::CompleteError::InvalidHandlerOutput { .. }),
            "expected InvalidHandlerOutput, got: {err:?}",
        );
    }

    /// Test 27: Effect shadowing — inner Handle intercepts same effect_id.
    #[test]
    fn effect_shadowing_inner_catches() {
        // Handle(e, outer_handler, Handle(e, inner_handler, Perform(e)))
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!("outer")),
            handle(1, always_resume_handler(json!("inner")), perform(1)),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Inner Handle catches the Perform. Handler resumes with "inner".
        // Inner body done -> inner Handle exits -> outer body done -> outer Handle exits.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, Some(json!("inner")));
        assert!(ts.is_empty());
    }

    /// Test 30: Handle(e, handler, Perform(e)) — Perform is direct child of Handle body.
    #[test]
    fn perform_direct_child_of_handle_body() {
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!("resumed")),
            perform(1),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Perform's parent is Handle{Body}. bubble_effect checks is_blocked_by_handle.
        // Handle is Free -> not blocked -> handler dispatched.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        // Handler resumes with "resumed". Deliver to Handle{Body} -> body done -> exit.
        assert_eq!(result, Some(json!("resumed")));
        assert!(ts.is_empty());
    }

    /// Test 12: resume_with_state_handler — handler extracts state, resumes with it.
    #[test]
    fn resume_with_state() {
        // Handler DAG: ExtractIndex(1) -> Tag("Resume")
        // This means the handler echoes the Handle's state (index 1) back as the resume value.
        let resume_with_state = chain(
            invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
            invoke_builtin(BuiltinKind::Tag {
                value: json!("Resume"),
            }),
        );
        // Handle's initial state = pipeline value ("input"). Body: Chain(Invoke(step), Perform(e))
        let mut engine = engine_from(handle(
            1,
            resume_with_state,
            chain(invoke("./step.ts", "step"), perform(1)),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 1); // step dispatched

        // Complete step -> Perform fires -> handler gets ["step_out", "input"]
        // Handler extracts state (index 1 = "input") and resumes with it.
        let (result, _) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("step_out")).unwrap();
        // Resume with "input" -> deliver to Chain -> body done -> Handle exits.
        assert_eq!(result, Some(json!("input")));
    }

    /// Test 13: State update persists across handler invocations.
    #[test]
    fn state_update_persists() {
        // Handler: TS handler we manually complete (so we can control state_update).
        // Body: Chain(Perform(e), Chain(Invoke(mid), Perform(e)))
        let mut engine = engine_from(handle(
            1,
            invoke("./handler.ts", "handler"),
            chain(perform(1), chain(invoke("./mid.ts", "mid"), perform(1))),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // First Perform fires during advance -> handler dispatched.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1);

        // First handler: Resume with state_update.
        let result = engine
            .complete(
                ts[0].task_id,
                json!({
                    "kind": "Resume",
                    "value": "v1",
                    "state_update": { "kind": "Updated", "value": "new_state" }
                }),
            )
            .unwrap();
        assert_eq!(result, None);

        // Chain trampolines to mid.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("v1"));

        // Complete mid -> Chain trampolines to second Perform -> handler dispatched.
        let (_, handler_ts) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("mid_out")).unwrap();
        assert_eq!(handler_ts.len(), 1);

        // Second handler receives state = "new_state" (persisted from first handler).
        // We verify by checking the dispatch value: [payload, state].
        assert_eq!(handler_ts[0].value, json!(["mid_out", "new_state"]),);

        // Complete second handler with Resume.
        let (result, _) = complete_and_drive(
            &mut engine,
            handler_ts[0].task_id,
            json!({"kind": "Resume", "value": "v2"}),
        )
        .unwrap();
        // Body done -> Handle exits.
        assert_eq!(result, Some(json!("v2")));
    }

    /// Test 21: Multi-step handler Chain. Handler side is never blocked.
    #[test]
    fn multi_step_handler_chain() {
        // Handler: Chain(Invoke(step1), Invoke(step2))
        // Body: Perform(e)
        let mut engine = engine_from(handle(
            1,
            chain(invoke("./step1.ts", "s1"), invoke("./step2.ts", "s2")),
            perform(1),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Perform fires -> handler Chain starts -> step1 dispatched.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1);

        // Complete step1 -> Chain trampolines to step2.
        let result = engine.complete(ts[0].task_id, json!("s1_out")).unwrap();
        assert_eq!(result, None);

        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("s1_out"));

        // Complete step2 -> handler done -> handle_handler_completion called.
        // step2 returns the HandlerOutput JSON.
        let result = engine
            .complete(ts[0].task_id, json!({"kind": "Resume", "value": "final"}))
            .unwrap();
        assert_eq!(result, Some(json!("final")));
    }

    /// Test 23: Two effects targeting different Handles.
    /// Handle(e1, h1, Handle(e2, h2, All(Perform(e1), Perform(e2))))
    #[test]
    fn two_effects_different_handles() {
        let mut engine = engine_from(handle(
            1,
            invoke("./h1.ts", "h1"),
            handle(
                2,
                invoke("./h2.ts", "h2"),
                parallel(vec![perform(1), perform(2)]),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // During advance: All advances both Performs.
        // Perform(e1): bubble_effect walks up from All -> Handle(e2) (wrong effect) ->
        //   Handle(e1) (matches, free) -> dispatches h1. Handle(e1) suspends.
        // Perform(e2): find_blocking_ancestor from All -> Handle(e2) (free, body, not blocked) ->
        //   Handle(e1) (body, suspended) -> Blocked! Stashed as Effect.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1); // h1 dispatch
        assert_eq!(engine.stashed_items.len(), 1);

        // Complete h1 with Resume.
        let (result, h2_ts) = complete_and_drive(
            &mut engine,
            ts[0].task_id,
            json!({"kind": "Resume", "value": "e1_resumed"}),
        )
        .unwrap();
        // Resume delivers to All slot 0. sweep_stash retries Perform(e2) ->
        // Handle(e1) free -> walk to Handle(e2) (matches, free) -> dispatches h2.
        assert_eq!(result, None);
        assert_eq!(h2_ts.len(), 1);

        // Complete h2 with Resume.
        let (result, _) = complete_and_drive(
            &mut engine,
            h2_ts[0].task_id,
            json!({"kind": "Resume", "value": "e2_resumed"}),
        )
        .unwrap();
        assert_eq!(result, Some(json!(["e1_resumed", "e2_resumed"])),);
    }

    // -- Bind-shaped AST tests --
    //
    // These verify the runtime behavior of the Handle/Perform substrate
    // when driven by bind-shaped ASTs. The ASTs are constructed directly
    // in Rust, matching what the TS `bind()` macro produces.

    /// Bind test 1: Single binding, single read.
    /// Chain(All(Constant(42), Identity), Handle(e0, readVar(0), Chain(Perform(e0), Invoke(echo))))
    /// Input: "input". All → [42, "input"]. Handle state = [42, "input"].
    /// Perform fires, readVar(0) extracts 42, resumes. echo receives 42.
    #[test]
    fn bind_single_binding_single_read() {
        let e0 = 10;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant { value: json!(42) }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(0),
                chain(perform(e0), invoke("./echo.ts", "echo")),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!(42)); // echo receives 42

        let result = engine.complete(ts[0].task_id, json!("echo_done")).unwrap();
        assert_eq!(result, Some(json!("echo_done")));
    }

    /// Bind test 2: Single binding, body ignores VarRef.
    /// Chain(All(Constant(42), Identity), Handle(e0, readVar(0), Chain(ExtractIndex(1), Invoke(echo))))
    /// Body extracts pipeline_input (index 1) and passes to echo. No Perform fires.
    #[test]
    fn bind_single_binding_body_ignores_varref() {
        let e0 = 10;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant { value: json!(42) }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(0),
                chain(
                    invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
                    invoke("./echo.ts", "echo"),
                ),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("input")); // echo receives pipeline_input

        let result = engine.complete(ts[0].task_id, json!("done")).unwrap();
        assert_eq!(result, Some(json!("done")));
    }

    /// Bind test 3: Two bindings, two reads.
    /// `All(Constant("alice"), Constant(99), Identity)` → `["alice", 99, "input"]`
    /// Nested Handle(e0, readVar(0), Handle(e1, readVar(1), Chain(Perform(e0), Chain(Invoke(mid), Perform(e1)))))
    /// First Perform → "alice", mid gets "alice", second Perform → 99.
    #[test]
    fn bind_two_bindings_two_reads() {
        let e0 = 10;
        let e1 = 11;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant {
                    value: json!("alice"),
                }),
                invoke_builtin(BuiltinKind::Constant { value: json!(99) }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(0),
                handle(
                    e1,
                    read_var(1),
                    chain(perform(e0), chain(invoke("./mid.ts", "mid"), perform(e1))),
                ),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // First Perform(e0): bubbles past inner Handle(e1), caught by outer Handle(e0).
        // readVar(0) extracts state[0] = "alice" from [alice, 99, input]. Resumes with "alice".
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        // Chain trampolines to mid with "alice".
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("alice"));

        // Complete mid → second Perform(e1). Inner Handle catches.
        // readVar(1) extracts state[1] = 99. Resumes with 99.
        let (result, ts2) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("mid_out")).unwrap();
        assert_eq!(result, Some(json!(99)));
        assert!(ts2.is_empty());
    }

    /// Bind test 4: Two bindings, reads in reverse order.
    /// Body: Chain(Perform(e1), Perform(e0)).
    /// Perform(e1) caught by inner Handle. Perform(e0) bubbles to outer Handle.
    #[test]
    fn bind_two_bindings_reverse_order() {
        let e0 = 10;
        let e1 = 11;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant {
                    value: json!("alice"),
                }),
                invoke_builtin(BuiltinKind::Constant { value: json!(99) }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(0),
                handle(e1, read_var(1), chain(perform(e1), perform(e0))),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Perform(e1) is caught by inner Handle(e1). readVar(1) → state[1] = 99. Resume with 99.
        // Chain trampolines to Perform(e0). Bubbles past inner Handle, caught by outer Handle(e0).
        // readVar(0) → state[0] = "alice". Resume.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, Some(json!("alice")));
        assert!(ts.is_empty());
    }

    /// Bind test 5: Nested binds.
    /// Outer bind: All("outer", Identity) → Handle(e_outer, readVar(0), ...)
    /// Inner bind: All("inner", Identity) → Handle(e_inner, readVar(0), ...)
    /// Body: Chain(Perform(e_outer), Perform(e_inner))
    /// e_outer bubbles past inner Handle, caught by outer. e_inner caught by inner.
    #[test]
    fn bind_nested() {
        let e_outer = 10;
        let e_inner = 11;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant {
                    value: json!("outer"),
                }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e_outer,
                read_var(0),
                chain(
                    invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
                    // Inner bind operates on pipeline_input
                    chain(
                        parallel(vec![
                            invoke_builtin(BuiltinKind::Constant {
                                value: json!("inner"),
                            }),
                            invoke_builtin(BuiltinKind::Identity),
                        ]),
                        handle(
                            e_inner,
                            read_var(0),
                            chain(
                                invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
                                chain(perform(e_outer), perform(e_inner)),
                            ),
                        ),
                    ),
                ),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, Some(json!("inner")));
        assert!(ts.is_empty());
    }

    /// Bind test 6: Bind inside ForEach.
    /// ForEach(Chain(All(Identity, Identity), Handle(e0, readVar(0), Chain(Perform(e0), Invoke(echo)))))
    /// Input [10, 20]. Each iteration binds its own element.
    #[test]
    fn bind_inside_foreach() {
        let e0 = 10;
        let mut engine = engine_from(for_each(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Identity),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(0),
                chain(perform(e0), invoke("./echo.ts", "echo")),
            ),
        )));
        let root = engine.workflow_root();
        engine.advance(root, json!([10, 20]), None).unwrap();

        // Both ForEach iterations run. Each binds its own element via Identity.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 2); // Two echo dispatches

        // Complete both echo invocations.
        assert_eq!(engine.complete(ts[0].task_id, json!("r10")).unwrap(), None,);
        assert_eq!(
            engine.complete(ts[1].task_id, json!("r20")).unwrap(),
            Some(json!(["r10", "r20"])),
        );
    }

    /// Bind test 7: Handler receives correct state shape.
    /// Handle(e, TS_handler, Perform(e)). Advance with "input".
    /// State = "input" (Handle initializes from pipeline value).
    /// Handler dispatch value should be ["input", [42, "input"]].
    #[test]
    fn bind_handler_receives_correct_state() {
        // Use a TS handler so we can inspect the dispatch value.
        let e0 = 10;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant { value: json!(42) }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                invoke("./handler.ts", "handler"),
                chain(
                    invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
                    perform(e0),
                ),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 1);
        // Handler receives [payload, state] = ["input", [42, "input"]]
        // (state is the All output tuple, which is the pipeline value entering the Handle)
        assert_eq!(ts[0].value, json!(["input", [42, "input"]]),);
    }

    /// Bind test 8: readVar(1) produces correct Resume value.
    /// State = `["a", "b", "c"]`. `readVar(1)` should produce `{ kind: "Resume", value: "b" }`.
    #[test]
    fn bind_read_var_produces_correct_resume() {
        let e0 = 10;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant { value: json!("a") }),
                invoke_builtin(BuiltinKind::Constant { value: json!("b") }),
                invoke_builtin(BuiltinKind::Constant { value: json!("c") }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(1), // Extract state[1] = "b"
                chain(
                    invoke_builtin(BuiltinKind::ExtractIndex { value: json!(3) }),
                    chain(perform(e0), invoke("./echo.ts", "echo")),
                ),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Perform fires. readVar(1) extracts state[1] = "b". Resumes with "b".
        // Chain trampolines to echo with "b".
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("b"));

        let result = engine.complete(ts[0].task_id, json!("done")).unwrap();
        assert_eq!(result, Some(json!("done")));
    }

    // -- Resume handler non-suspension tests --
    //
    // These tests document the desired behavior after the Resume/Restart
    // handler split: resume handlers should NOT suspend the Handle frame,
    // so concurrent siblings should not be blocked.
    //
    // Currently these fail because the engine suspends on ALL Performs.

    /// Resume handler with async handler DAG should not block sibling
    /// completions.
    #[test]
    #[should_panic(expected = "resume handler should not cause stashing")]
    fn resume_handler_does_not_block_sibling_completion() {
        let mut engine = engine_from(handle(
            1,
            invoke("./handler.ts", "handler"),
            parallel(vec![
                chain(invoke("./a.ts", "a"), perform(1)),
                invoke("./b.ts", "b"),
            ]),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 2); // A and B

        // Complete A → Perform → handler dispatched (async TS handler).
        let (result, handler_ts) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, None);
        assert_eq!(handler_ts.len(), 1);

        // Complete B while handler is in flight.
        // After refactor: should NOT be stashed.
        let result = engine.complete(ts[1].task_id, json!("b_out")).unwrap();
        assert_eq!(result, None);
        assert_eq!(
            engine.stashed_items.len(),
            0,
            "resume handler should not cause stashing"
        );
    }

    /// Two concurrent resume Performs should both dispatch their handlers
    /// without serialization.
    #[test]
    #[should_panic(expected = "both handlers should dispatch concurrently")]
    fn concurrent_resume_performs_not_serialized() {
        let mut engine = engine_from(handle(
            1,
            invoke("./handler.ts", "handler"),
            parallel(vec![perform(1), perform(1)]),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // After refactor: both Performs should dispatch, no stashing.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 2, "both handlers should dispatch concurrently");
        assert_eq!(
            engine.stashed_items.len(),
            0,
            "no Perform should be stashed"
        );
    }

    /// A throw (to an outer restart+Branch) should proceed even while a resume
    /// handler is in flight in a sibling branch.
    #[test]
    #[should_panic(expected = "throw should reach outer handler immediately")]
    fn throw_proceeds_while_resume_handler_in_flight() {
        let inner_e = 1; // resume-style
        let outer_e = 2; // restart-style (tryCatch)

        let inner_handle = handle(
            inner_e,
            invoke("./handler.ts", "handler"), // async resume handler
            parallel(vec![
                chain(invoke("./a.ts", "a"), perform(inner_e)),
                chain(invoke("./b.ts", "b"), break_perform(outer_e)),
            ]),
        );

        let mut engine = engine_from(restart_branch(outer_e, inner_handle, identity_action()));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 2); // A and B

        // Complete A → Perform(inner_e) → resume handler dispatched.
        let (result, handler_ts) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, None);
        assert_eq!(handler_ts.len(), 1);

        // Complete B → break_perform(outer_e). Should bubble up past inner Handle
        // (which is NOT suspended for a resume handler) and reach outer Handle.
        let (result, _) = complete_and_drive(&mut engine, ts[1].task_id, json!("b_out")).unwrap();

        // Outer handler restarts body, Branch takes Break arm, Handle exits.
        assert_eq!(
            result,
            Some(json!("b_out")),
            "throw should reach outer handler immediately"
        );
    }
}
