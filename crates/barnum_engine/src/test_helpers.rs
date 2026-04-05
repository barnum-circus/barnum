//! Shared test helpers for `barnum_engine` tests.
//!
//! Provides AST construction helpers and engine driving utilities used
//! across test modules in advance, complete, effects, and frame.

use crate::complete::complete;
use crate::{CompleteError, CompletionEvent, DispatchEvent, PendingEffectKind, WorkflowState};

use barnum_ast::flat::flatten;
use barnum_ast::*;
use intern::string_key::Intern;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// AST construction helpers
// ---------------------------------------------------------------------------

pub fn ts_handler(module: &str, func: &str) -> HandlerKind {
    HandlerKind::TypeScript(TypeScriptHandler {
        module: ModulePath::from(module.intern()),
        func: FuncName::from(func.intern()),
        input_schema: None,
        output_schema: None,
    })
}

pub fn invoke(module: &str, func: &str) -> Action {
    Action::Invoke(InvokeAction {
        handler: ts_handler(module, func),
    })
}

pub fn chain(first: Action, rest: Action) -> Action {
    Action::Chain(ChainAction {
        first: Box::new(first),
        rest: Box::new(rest),
    })
}

pub fn parallel(actions: Vec<Action>) -> Action {
    Action::All(AllAction { actions })
}

pub fn for_each(action: Action) -> Action {
    Action::ForEach(ForEachAction {
        action: Box::new(action),
    })
}

pub fn branch(cases: Vec<(&str, Action)>) -> Action {
    Action::Branch(BranchAction {
        cases: cases
            .into_iter()
            .map(|(k, v)| (KindDiscriminator::from(k.intern()), v))
            .collect(),
    })
}

#[allow(clippy::unwrap_used)]
pub fn engine_from(workflow: Action) -> WorkflowState {
    let config = Config { workflow };
    WorkflowState::new(flatten(config).unwrap())
}

// ---------------------------------------------------------------------------
// Builtin helpers
// ---------------------------------------------------------------------------

pub fn invoke_builtin(builtin: BuiltinKind) -> Action {
    Action::Invoke(InvokeAction {
        handler: HandlerKind::Builtin(BuiltinHandler { builtin }),
    })
}

pub fn tag_builtin(kind: &str) -> Action {
    invoke_builtin(BuiltinKind::Tag { value: json!(kind) })
}

pub fn extract_field(field: &str) -> Action {
    invoke_builtin(BuiltinKind::ExtractField {
        value: json!(field),
    })
}

pub fn extract_index(index: u64) -> Action {
    invoke_builtin(BuiltinKind::ExtractIndex {
        value: json!(index),
    })
}

pub fn identity_action() -> Action {
    invoke_builtin(BuiltinKind::Identity)
}

// ---------------------------------------------------------------------------
// ResumeHandle / ResumePerform helpers
// ---------------------------------------------------------------------------

pub fn resume_handle(resume_handler_id: u16, handler: Action, body: Action) -> Action {
    Action::ResumeHandle(ResumeHandleAction {
        resume_handler_id: ResumeHandlerId(resume_handler_id),
        body: Box::new(body),
        handler: Box::new(handler),
    })
}

pub fn resume_perform(resume_handler_id: u16) -> Action {
    Action::ResumePerform(ResumePerformAction {
        resume_handler_id: ResumeHandlerId(resume_handler_id),
    })
}

/// `readVar(n)` for `ResumePerform`: `All(Chain(ExtractIndex(1), ExtractIndex(n)), ExtractIndex(1))`
///
/// Input: `[payload, state]`. Output: `[state[n], state]`.
/// Value is `state[n]`, state is unchanged.
pub fn resume_read_var(n: u64) -> Action {
    parallel(vec![
        chain(extract_index(1), extract_index(n)),
        extract_index(1),
    ])
}

// ---------------------------------------------------------------------------
// RestartHandle / RestartPerform helpers
// ---------------------------------------------------------------------------

pub fn restart_handle(restart_handler_id: u16, handler: Action, body: Action) -> Action {
    Action::RestartHandle(RestartHandleAction {
        restart_handler_id: RestartHandlerId(restart_handler_id),
        body: Box::new(body),
        handler: Box::new(handler),
    })
}

pub fn restart_perform(restart_handler_id: u16) -> Action {
    Action::RestartPerform(RestartPerformAction {
        restart_handler_id: RestartHandlerId(restart_handler_id),
    })
}

/// `Chain(Tag("Break"), RestartPerform(restart_handler_id))` —
/// triggers restart with Break routing.
pub fn break_restart_perform(restart_handler_id: u16) -> Action {
    chain(tag_builtin("Break"), restart_perform(restart_handler_id))
}

/// Handler for restart+Branch: extract payload (index 0) from `[payload, state]`.
/// The raw payload is the new body input.
pub fn restart_extract_payload_handler() -> Action {
    extract_index(0)
}

/// Build restart+Branch compiled form:
/// `Chain(Tag("Continue"), RestartHandle(id, ExtractIndex(0), Branch({`
///   `Continue: Chain(ExtractField("value"), continueArm),`
///   `Break: Chain(ExtractField("value"), breakArm),`
/// `})))`
pub fn restart_branch(restart_handler_id: u16, continue_arm: Action, break_arm: Action) -> Action {
    chain(
        tag_builtin("Continue"),
        restart_handle(
            restart_handler_id,
            restart_extract_payload_handler(),
            branch(vec![
                ("Continue", chain(extract_field("value"), continue_arm)),
                ("Break", chain(extract_field("value"), break_arm)),
            ]),
        ),
    )
}

// ---------------------------------------------------------------------------
// Engine driving helpers
// ---------------------------------------------------------------------------

/// Pop the next pending dispatch from the effect queue.
/// Panics if the next effect is not a `Dispatch`.
/// Test-only convenience — when `Restart` is added, non-exhaustive
/// match will force callers to handle it.
pub fn pop_dispatch(engine: &mut WorkflowState) -> Option<DispatchEvent> {
    let (_, kind) = engine.pop_pending_effect()?;
    let PendingEffectKind::Dispatch(dispatch_event) = kind;
    Some(dispatch_event)
}

/// Process all pending builtin dispatches. Returns TypeScript dispatches
/// for manual completion and workflow result (if the workflow terminated).
#[allow(clippy::unwrap_used, clippy::type_complexity)]
pub fn drive_builtins(
    engine: &mut WorkflowState,
) -> Result<(Option<Value>, Vec<DispatchEvent>), CompleteError> {
    let mut ts_dispatches: Vec<DispatchEvent> = Vec::new();
    loop {
        let Some((frame_id, pending_effect_kind)) = engine.pop_pending_effect() else {
            break;
        };
        if !engine.is_frame_live(frame_id) {
            continue;
        }
        let PendingEffectKind::Dispatch(dispatch_event) = pending_effect_kind;
        match engine.handler(dispatch_event.handler_id).clone() {
            HandlerKind::Builtin(builtin_handler) => {
                let result = barnum_builtins::execute_builtin(
                    &builtin_handler.builtin,
                    &dispatch_event.value,
                )
                .unwrap();
                let completion_event = CompletionEvent {
                    task_id: dispatch_event.task_id,
                    value: result,
                };
                if let Some(value) = complete(engine, completion_event)? {
                    return Ok((Some(value), ts_dispatches));
                }
            }
            HandlerKind::TypeScript(_) => {
                ts_dispatches.push(dispatch_event);
            }
        }
    }
    Ok((None, ts_dispatches))
}

/// Complete a task and then drive all resulting builtins.
#[allow(clippy::unwrap_used)]
pub fn complete_and_drive(
    engine: &mut WorkflowState,
    completion_event: CompletionEvent,
) -> Result<(Option<Value>, Vec<DispatchEvent>), CompleteError> {
    if engine.task_frame_id(completion_event.task_id).is_none() {
        return Ok((None, Vec::new()));
    }
    let result = complete(engine, completion_event)?;
    if result.is_some() {
        return Ok((result, Vec::new()));
    }
    drive_builtins(engine)
}
