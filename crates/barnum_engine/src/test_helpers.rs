//! Shared test helpers for `barnum_engine` tests.
//!
//! Provides AST construction helpers and engine driving utilities used
//! across test modules in advance, complete, effects, and frame.

use crate::{CompleteError, Dispatch, TaskId, WorkflowState};

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
// Handle / Perform helpers
// ---------------------------------------------------------------------------

pub fn handle(effect_id: u16, handler: Action, body: Action) -> Action {
    Action::Handle(HandleAction {
        effect_id: EffectId(effect_id),
        body: Box::new(body),
        handler: Box::new(handler),
    })
}

pub fn perform(effect_id: u16) -> Action {
    Action::Perform(PerformAction {
        effect_id: EffectId(effect_id),
    })
}

pub fn invoke_builtin(builtin: BuiltinKind) -> Action {
    Action::Invoke(InvokeAction {
        handler: HandlerKind::Builtin(BuiltinHandler { builtin }),
    })
}

pub fn constant_handler(value: Value) -> Action {
    invoke_builtin(BuiltinKind::Constant { value })
}

#[allow(clippy::needless_pass_by_value)]
pub fn always_resume_handler(value: Value) -> Action {
    constant_handler(json!({
        "kind": "Resume",
        "value": value,
    }))
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

/// Handler for restart+Branch: extract payload (index 0), tag `RestartBody`.
pub fn restart_body_handler() -> Action {
    chain(extract_index(0), tag_builtin("RestartBody"))
}

/// `Chain(Tag("Break"), Perform(effect_id))` — triggers restart with Break routing.
pub fn break_perform(effect_id: u16) -> Action {
    chain(tag_builtin("Break"), perform(effect_id))
}

/// Build restart+Branch compiled form:
/// `Chain(Tag("Continue"), Handle(effectId, Branch({`
///   `Continue: Chain(ExtractField("value"), continueArm),`
///   `Break: Chain(ExtractField("value"), breakArm),`
/// `}), RestartBodyHandler))`
pub fn restart_branch(effect_id: u16, continue_arm: Action, break_arm: Action) -> Action {
    chain(
        tag_builtin("Continue"),
        handle(
            effect_id,
            restart_body_handler(),
            branch(vec![
                ("Continue", chain(extract_field("value"), continue_arm)),
                ("Break", chain(extract_field("value"), break_arm)),
            ]),
        ),
    )
}

pub fn echo_resume_handler() -> Action {
    chain(
        invoke_builtin(BuiltinKind::ExtractIndex { value: json!(0) }),
        invoke_builtin(BuiltinKind::Tag {
            value: json!("Resume"),
        }),
    )
}

pub fn garbage_output_handler() -> Action {
    constant_handler(json!({ "kind": "Unknown" }))
}

pub fn missing_fields_handler() -> Action {
    constant_handler(json!({ "kind": "Resume" }))
}

/// readVar(n): Chain(ExtractIndex(1), Chain(ExtractIndex(n), Tag("Resume")))
pub fn read_var(n: u64) -> Action {
    chain(
        invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
        chain(
            invoke_builtin(BuiltinKind::ExtractIndex { value: json!(n) }),
            invoke_builtin(BuiltinKind::Tag {
                value: json!("Resume"),
            }),
        ),
    )
}

// ---------------------------------------------------------------------------
// Engine driving helpers
// ---------------------------------------------------------------------------

/// Process all pending builtin dispatches. Returns TypeScript dispatches
/// for manual completion and workflow result (if the workflow terminated).
#[allow(clippy::unwrap_used, clippy::type_complexity)]
pub fn drive_builtins(
    engine: &mut WorkflowState,
) -> Result<(Option<Value>, Vec<Dispatch>), CompleteError> {
    let mut ts_dispatches: Vec<Dispatch> = Vec::new();
    loop {
        let dispatches = engine.take_pending_dispatches();
        if dispatches.is_empty() {
            break;
        }
        let mut had_builtin = false;
        for dispatch in dispatches {
            match engine.handler(dispatch.handler_id).clone() {
                HandlerKind::Builtin(builtin_handler) => {
                    let result =
                        barnum_builtins::execute_builtin(&builtin_handler.builtin, &dispatch.value)
                            .unwrap();
                    if let Some(value) = engine.complete(dispatch.task_id, result)? {
                        return Ok((Some(value), ts_dispatches));
                    }
                    had_builtin = true;
                }
                HandlerKind::TypeScript(_) => {
                    ts_dispatches.push(dispatch);
                }
            }
        }
        if !had_builtin {
            break;
        }
    }
    Ok((None, ts_dispatches))
}

/// Complete a task and then drive all resulting builtins.
#[allow(clippy::unwrap_used)]
pub fn complete_and_drive(
    engine: &mut WorkflowState,
    task_id: TaskId,
    value: Value,
) -> Result<(Option<Value>, Vec<Dispatch>), CompleteError> {
    let result = engine.complete(task_id, value)?;
    if result.is_some() {
        let ts = engine.take_pending_dispatches();
        return Ok((result, ts));
    }
    drive_builtins(engine)
}
