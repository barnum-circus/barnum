//! Barnum workflow runtime: Scheduler + `run_workflow` loop.
//!
//! The [`Scheduler`] dispatches handler invocations as tokio tasks and collects
//! results via an internal channel. [`run_workflow`] drives the
//! [`WorkflowState`] by repeatedly dispatching pending work and feeding
//! completions back until the workflow terminates.

use std::collections::HashMap;

use barnum_ast::HandlerKind;
use barnum_ast::flat::HandlerId;
use barnum_builtins::{BuiltinError, execute_builtin};
use barnum_engine::advance::advance;
use barnum_engine::complete::complete;
use barnum_engine::effects::process_restart;
use barnum_engine::{
    CompleteError, CompletionEvent, DispatchEvent, PendingEffectKind, RestartEvent, TaskId,
    WorkflowState,
};
use barnum_typescript_handler::{TypeScriptHandlerError, execute_typescript};
use intern::Lookup;
use jsonschema::Validator;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};

// =============================================================================
// HandlerError
// =============================================================================

/// Unified error type for handler execution failures.
#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    /// A builtin handler encountered a type mismatch.
    #[error(transparent)]
    Builtin(#[from] BuiltinError),
    /// A TypeScript handler subprocess failed.
    #[error(transparent)]
    TypeScript(#[from] TypeScriptHandlerError),
}

// =============================================================================
// Event
// =============================================================================

/// The payload of a workflow event. Paired with a `FrameId` liveness key.
enum EventKind {
    /// A handler invocation ready to dispatch to a worker.
    Dispatch(DispatchEvent),
    /// A deferred restart effect ready to process.
    Restart(RestartEvent),
    /// A worker completed a task.
    Completion(CompletionEvent),
}

impl From<PendingEffectKind> for EventKind {
    fn from(kind: PendingEffectKind) -> Self {
        match kind {
            PendingEffectKind::Dispatch(dispatch_event) => EventKind::Dispatch(dispatch_event),
            PendingEffectKind::Restart(restart_event) => EventKind::Restart(restart_event),
        }
    }
}

// =============================================================================
// Scheduler
// =============================================================================

/// Dispatches handler invocations as tokio tasks and collects results.
///
/// Each [`dispatch`](Scheduler::dispatch) call spawns a lightweight tokio task.
/// Results are collected via [`recv`](Scheduler::recv).
pub struct Scheduler {
    result_tx: mpsc::UnboundedSender<(TaskId, Result<Value, HandlerError>)>,
    result_rx: mpsc::UnboundedReceiver<(TaskId, Result<Value, HandlerError>)>,
    /// The command to invoke the worker, e.g. `"node /path/to/tsx/cli.mjs"`.
    executor: String,
    /// Path to `worker.ts`.
    worker_path: String,
}

impl Scheduler {
    /// Create a scheduler that spawns one subprocess per TypeScript handler invocation.
    ///
    /// `executor` is the command to run TypeScript, e.g. `"node /path/to/tsx/cli.mjs"`.
    /// `worker_path` is the absolute path to `worker.ts`.
    #[must_use]
    pub fn new(executor: String, worker_path: String) -> Self {
        let (result_tx, result_rx) = mpsc::unbounded_channel();
        Self {
            result_tx,
            result_rx,
            executor,
            worker_path,
        }
    }

    /// Dispatch a handler invocation.
    ///
    /// Spawns a tokio task that executes the handler and sends the result
    /// through the internal channel. Builtins are executed inline within
    /// the spawned task. TypeScript handlers spawn a subprocess.
    pub fn dispatch(&self, dispatch_event: &DispatchEvent, handler: &HandlerKind) {
        let result_tx = self.result_tx.clone();
        let task_id = dispatch_event.task_id;

        match handler {
            HandlerKind::Builtin(builtin_handler) => {
                debug!(task = %task_id, builtin = ?builtin_handler.builtin, "dispatching builtin handler");
                trace!(task = %task_id, value = %dispatch_event.value, "builtin input");
                let builtin_kind = builtin_handler.builtin.clone();
                let value = dispatch_event.value.clone();
                tokio::spawn(async move {
                    let result = execute_builtin(&builtin_kind, &value)
                        .await
                        .map_err(HandlerError::from);
                    let _ = result_tx.send((task_id, result));
                });
            }
            HandlerKind::TypeScript(ts) => {
                let module = ts.module.lookup().to_owned();
                let func = ts.func.lookup().to_owned();
                info!(task = %task_id, module = %module, func = %func, "dispatching TypeScript handler");
                trace!(task = %task_id, value = %dispatch_event.value, "handler input");
                let value = dispatch_event.value.clone();
                let executor = self.executor.clone();
                let worker_path = self.worker_path.clone();

                tokio::spawn(async move {
                    let result =
                        execute_typescript(&executor, &worker_path, &module, &func, &value)
                            .await
                            .map_err(HandlerError::from);
                    let _ = result_tx.send((task_id, result));
                });
            }
        }
    }

    /// Wait for the next handler result.
    ///
    /// Returns `None` if all senders have been dropped (shouldn't happen
    /// during normal operation since `self` holds a sender).
    pub async fn recv(&mut self) -> Option<(TaskId, Result<Value, HandlerError>)> {
        self.result_rx.recv().await
    }

    /// Inject a fake handler result into the channel (test-only).
    ///
    /// The injected result is buffered immediately and will be received
    /// before any results from actual handler execution.
    #[cfg(test)]
    pub fn inject_completion(&self, task_id: TaskId, value: Value) {
        let _ = self.result_tx.send((task_id, Ok(value)));
    }
}

// =============================================================================
// run_workflow
// =============================================================================

/// Whether a schema validation error concerns the handler's input or output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaDirection {
    /// The value passed *to* the handler.
    Input,
    /// The value returned *from* the handler.
    Output,
}

impl std::fmt::Display for SchemaDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input => f.write_str("input"),
            Self::Output => f.write_str("output"),
        }
    }
}

// =============================================================================
// Schema compilation
// =============================================================================

/// Compiled input/output JSON Schema validators, keyed by [`HandlerId`].
///
/// Only TypeScript handlers with schemas have entries. Builtins are never
/// validated (framework code with known types — no trust boundary).
struct CompiledSchemas {
    input: HashMap<HandlerId, Validator>,
    output: HashMap<HandlerId, Validator>,
}

/// Compile all handler schemas at workflow init.
///
/// Iterates the handler pool, compiles each TypeScript handler's input/output
/// schemas into [`Validator`]s, and returns them keyed by [`HandlerId`].
/// Invalid schemas (not well-formed JSON Schema) cause an immediate error.
fn compile_schemas(workflow_state: &WorkflowState) -> Result<CompiledSchemas, RunWorkflowError> {
    let mut input = HashMap::new();
    let mut output = HashMap::new();

    for (handler_id, handler_kind) in workflow_state.flat_config().handlers() {
        let HandlerKind::TypeScript(ts_handler) = handler_kind else {
            continue;
        };

        if let Some(ref schema) = ts_handler.input_schema {
            let validator =
                Validator::new(&schema.0).map_err(|err| RunWorkflowError::InvalidSchema {
                    module: ts_handler.module.lookup().to_owned(),
                    func: ts_handler.func.lookup().to_owned(),
                    direction: SchemaDirection::Input,
                    error: err.to_string(),
                })?;
            input.insert(handler_id, validator);
        }

        if let Some(ref schema) = ts_handler.output_schema {
            let validator =
                Validator::new(&schema.0).map_err(|err| RunWorkflowError::InvalidSchema {
                    module: ts_handler.module.lookup().to_owned(),
                    func: ts_handler.func.lookup().to_owned(),
                    direction: SchemaDirection::Output,
                    error: err.to_string(),
                })?;
            output.insert(handler_id, validator);
        }
    }

    Ok(CompiledSchemas { input, output })
}

/// Format validation errors into human-readable strings.
fn format_validation_errors(errors: &[jsonschema::ValidationError]) -> Vec<String> {
    errors
        .iter()
        .map(|e| format!("{}: {e}", e.instance_path))
        .collect()
}

// =============================================================================
// RunWorkflowError
// =============================================================================

/// Errors that can occur during [`run_workflow`].
#[derive(Debug, thiserror::Error)]
pub enum RunWorkflowError {
    /// A handler execution failed.
    #[error(transparent)]
    Handler(#[from] HandlerError),
    /// The engine encountered an error during completion.
    #[error(transparent)]
    Complete(#[from] CompleteError),
    /// An advance error occurred during restart processing.
    #[error(transparent)]
    Advance(#[from] barnum_engine::AdvanceError),
    /// A handler's embedded JSON Schema is not valid JSON Schema.
    /// Caught at workflow init during schema compilation.
    #[error("invalid {direction} schema for {module}:{func}: {error}")]
    InvalidSchema {
        /// The handler's module path.
        module: String,
        /// The handler's function name.
        func: String,
        /// Whether the invalid schema is for input or output.
        direction: SchemaDirection,
        /// The schema compilation error message.
        error: String,
    },
    /// A handler's input or output value failed schema validation.
    #[error("{direction} validation failed for {module}:{func}: {errors:?}")]
    SchemaValidation {
        /// The handler's module path.
        module: String,
        /// The handler's function name.
        func: String,
        /// Whether the failing value is the input or output.
        direction: SchemaDirection,
        /// Individual validation error messages.
        errors: Vec<String>,
    },
}

/// Run a workflow to completion.
///
/// Performs the initial advance, then loops: dispatch pending work to the
/// scheduler, receive one result, feed it back to the workflow state. Repeats
/// until the workflow terminates.
///
/// # Errors
///
/// Returns [`RunWorkflowError`] if a handler fails or a completion causes
/// an engine error (e.g., invalid loop result, advance failure during
/// Chain trampoline).
///
/// # Panics
///
/// Panics if the initial advance fails or the scheduler channel closes
/// unexpectedly.
#[allow(clippy::missing_panics_doc, clippy::expect_used)]
pub async fn run_workflow(
    workflow_state: &mut WorkflowState,
    scheduler: &mut Scheduler,
) -> Result<Value, RunWorkflowError> {
    let compiled_schemas = compile_schemas(workflow_state)?;

    let root = workflow_state.workflow_root();
    info!("starting workflow");
    advance(workflow_state, root, Value::Null, None).expect("initial advance failed");

    loop {
        let (frame_id, event_kind) =
            if let Some((frame_id, pending_kind)) = workflow_state.pop_pending_effect() {
                (frame_id, EventKind::from(pending_kind))
            } else {
                debug!("waiting for handler completion");
                let (task_id, result) = scheduler
                    .recv()
                    .await
                    .expect("scheduler channel closed unexpectedly");
                let Some(frame_id) = workflow_state.task_frame_id(task_id) else {
                    debug!(task = %task_id, "ignoring stale completion (task torn down)");
                    continue;
                };
                (
                    frame_id,
                    EventKind::Completion(CompletionEvent {
                        task_id,
                        value: result?,
                    }),
                )
            };

        if !workflow_state.is_frame_live(frame_id) {
            debug!("ignoring event for dead frame");
            continue;
        }

        match event_kind {
            EventKind::Dispatch(dispatch_event) => {
                // Validate input before dispatching to the handler.
                validate_value(
                    &compiled_schemas.input,
                    dispatch_event.handler_id,
                    &dispatch_event.value,
                    SchemaDirection::Input,
                    workflow_state,
                )?;

                let handler = workflow_state.handler(dispatch_event.handler_id);
                scheduler.dispatch(&dispatch_event, handler);
            }
            EventKind::Restart(restart_event) => {
                info!("processing restart effect");
                process_restart(workflow_state, restart_event)?;
            }
            EventKind::Completion(completion_event) => {
                let handler_id = workflow_state.handler_id_for_task(completion_event.task_id);

                // Log the completion with handler info.
                match workflow_state.handler(handler_id) {
                    HandlerKind::TypeScript(ts) => {
                        info!(
                            task = %completion_event.task_id,
                            module = %ts.module.lookup(),
                            func = %ts.func.lookup(),
                            "handler completed"
                        );
                        trace!(task = %completion_event.task_id, value = %completion_event.value, "handler output");
                    }
                    HandlerKind::Builtin(builtin) => {
                        debug!(
                            task = %completion_event.task_id,
                            builtin = ?builtin.builtin,
                            "builtin completed"
                        );
                        trace!(task = %completion_event.task_id, value = %completion_event.value, "builtin output");
                    }
                }

                // Validate output before delivering the completion.
                validate_value(
                    &compiled_schemas.output,
                    handler_id,
                    &completion_event.value,
                    SchemaDirection::Output,
                    workflow_state,
                )?;

                if let Some(terminal_value) = complete(workflow_state, completion_event)? {
                    info!("workflow completed");
                    trace!(value = %terminal_value, "workflow result");
                    return Ok(terminal_value);
                }
            }
        }
    }
}

/// Validate a value against a compiled schema, if one exists for the handler.
fn validate_value(
    validators: &HashMap<HandlerId, Validator>,
    handler_id: HandlerId,
    value: &Value,
    direction: SchemaDirection,
    workflow_state: &WorkflowState,
) -> Result<(), RunWorkflowError> {
    let Some(validator) = validators.get(&handler_id) else {
        return Ok(());
    };

    let errors: Vec<_> = validator.iter_errors(value).collect();
    if errors.is_empty() {
        return Ok(());
    }

    let handler = workflow_state.handler(handler_id);
    let HandlerKind::TypeScript(ts_handler) = handler else {
        // Only TypeScript handlers have schemas — this branch is unreachable
        // in practice since builtins never have validators compiled.
        return Ok(());
    };

    let formatted_errors = format_validation_errors(&errors);
    warn!(
        module = %ts_handler.module.lookup(),
        func = %ts_handler.func.lookup(),
        direction = %direction,
        errors = ?formatted_errors,
        "schema validation failed"
    );
    Err(RunWorkflowError::SchemaValidation {
        module: ts_handler.module.lookup().to_owned(),
        func: ts_handler.func.lookup().to_owned(),
        direction,
        errors: formatted_errors,
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::doc_markdown, clippy::panic)]
mod tests {
    use super::*;
    use barnum_ast::flat::flatten;
    use barnum_ast::{
        Action, BuiltinHandler, BuiltinKind, Config, FuncName, InvokeAction, JsonSchema,
        ModulePath, TypeScriptHandler,
    };
    use intern::string_key::Intern;

    fn constant(value: Value) -> Action {
        Action::Invoke(InvokeAction {
            handler: HandlerKind::Builtin(BuiltinHandler {
                builtin: BuiltinKind::Constant { value },
            }),
        })
    }

    fn ts_invoke(module: &str, func: &str) -> Action {
        Action::Invoke(InvokeAction {
            handler: HandlerKind::TypeScript(TypeScriptHandler {
                module: ModulePath::from(module.intern()),
                func: FuncName::from(func.intern()),
                input_schema: None,
                output_schema: None,
            }),
        })
    }

    fn ts_invoke_with_schemas(
        module: &str,
        func: &str,
        input_schema: Option<Value>,
        output_schema: Option<Value>,
    ) -> Action {
        Action::Invoke(InvokeAction {
            handler: HandlerKind::TypeScript(TypeScriptHandler {
                module: ModulePath::from(module.intern()),
                func: FuncName::from(func.intern()),
                input_schema: input_schema.map(JsonSchema),
                output_schema: output_schema.map(JsonSchema),
            }),
        })
    }

    fn config(workflow: Action) -> Config {
        Config { workflow }
    }

    /// Scheduler with dummy executor/worker paths — only builtin handlers
    /// are used, so the subprocess executor is never invoked.
    fn test_scheduler() -> Scheduler {
        Scheduler::new("unused".to_owned(), "unused".to_owned())
    }

    #[tokio::test]
    async fn single_invoke() {
        let flat_config = flatten(config(constant(serde_json::json!({"x": 42})))).unwrap();
        let mut workflow_state = WorkflowState::new(flat_config);
        let mut scheduler = test_scheduler();

        let result = run_workflow(&mut workflow_state, &mut scheduler)
            .await
            .unwrap();

        assert_eq!(result, serde_json::json!({"x": 42}));
    }

    #[tokio::test]
    async fn chain_of_two_invokes() {
        let flat_config = flatten(config(Action::Chain(barnum_ast::ChainAction {
            first: Box::new(constant(serde_json::json!({"a": 1}))),
            rest: Box::new(constant(serde_json::json!({"b": 2}))),
        })))
        .unwrap();
        let mut workflow_state = WorkflowState::new(flat_config);
        let mut scheduler = test_scheduler();

        let result = run_workflow(&mut workflow_state, &mut scheduler)
            .await
            .unwrap();

        // Chain output is the last step's output
        assert_eq!(result, serde_json::json!({"b": 2}));
    }

    #[tokio::test]
    async fn all_two_invokes() {
        let flat_config = flatten(config(Action::All(barnum_ast::AllAction {
            actions: vec![
                constant(serde_json::json!({"a": 1})),
                constant(serde_json::json!({"b": 2})),
            ],
        })))
        .unwrap();
        let mut workflow_state = WorkflowState::new(flat_config);
        let mut scheduler = test_scheduler();

        let result = run_workflow(&mut workflow_state, &mut scheduler)
            .await
            .unwrap();

        // All collects results into an array
        assert_eq!(result, serde_json::json!([{"a": 1}, {"b": 2}]));
    }

    // =========================================================================
    // Schema validation tests
    // =========================================================================

    /// compile_schemas succeeds for handlers with valid JSON Schemas.
    #[test]
    fn compile_schemas_valid() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        });
        let flat_config = flatten(config(ts_invoke_with_schemas(
            "./handler.ts",
            "run",
            Some(schema.clone()),
            Some(schema),
        )))
        .unwrap();
        let workflow_state = WorkflowState::new(flat_config);

        let compiled = compile_schemas(&workflow_state);
        assert!(compiled.is_ok());
        let compiled = compiled.unwrap();
        assert_eq!(compiled.input.len(), 1);
        assert_eq!(compiled.output.len(), 1);
    }

    /// compile_schemas succeeds (with empty maps) for handlers without schemas.
    #[test]
    fn compile_schemas_no_schemas() {
        let flat_config = flatten(config(ts_invoke("./handler.ts", "run"))).unwrap();
        let workflow_state = WorkflowState::new(flat_config);

        let compiled = compile_schemas(&workflow_state).unwrap();
        assert!(compiled.input.is_empty());
        assert!(compiled.output.is_empty());
    }

    /// compile_schemas returns InvalidSchema for a malformed JSON Schema.
    #[test]
    fn compile_schemas_invalid_schema() {
        // `minimum` must be a number, not a string — this is not valid JSON Schema.
        let bad_schema = serde_json::json!({
            "type": "object",
            "properties": { "age": { "type": "integer", "minimum": "not-a-number" } }
        });
        let flat_config = flatten(config(ts_invoke_with_schemas(
            "./handler.ts",
            "run",
            Some(bad_schema),
            None,
        )))
        .unwrap();
        let workflow_state = WorkflowState::new(flat_config);

        let result = compile_schemas(&workflow_state);
        match result {
            Err(RunWorkflowError::InvalidSchema {
                direction: SchemaDirection::Input,
                ..
            }) => {} // expected
            Err(other) => panic!("expected InvalidSchema(Input), got: {other:?}"),
            Ok(_) => panic!("expected InvalidSchema error, got Ok"),
        }
    }

    /// validate_value passes when the value matches the schema.
    #[test]
    fn validate_value_matching() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "integer" } },
            "required": ["x"]
        });
        let flat_config = flatten(config(ts_invoke_with_schemas(
            "./handler.ts",
            "run",
            Some(schema),
            None,
        )))
        .unwrap();
        let workflow_state = WorkflowState::new(flat_config);
        let compiled = compile_schemas(&workflow_state).unwrap();

        let value = serde_json::json!({"x": 42});
        let handler_id = HandlerId(0);
        let result = validate_value(
            &compiled.input,
            handler_id,
            &value,
            SchemaDirection::Input,
            &workflow_state,
        );
        assert!(result.is_ok());
    }

    /// validate_value returns SchemaValidation when the value violates the schema.
    #[test]
    fn validate_value_failing() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "integer" } },
            "required": ["x"]
        });
        let flat_config = flatten(config(ts_invoke_with_schemas(
            "./handler.ts",
            "run",
            Some(schema),
            None,
        )))
        .unwrap();
        let workflow_state = WorkflowState::new(flat_config);
        let compiled = compile_schemas(&workflow_state).unwrap();

        // Value violates: "x" is a string, not integer.
        let value = serde_json::json!({"x": "not-an-integer"});
        let handler_id = HandlerId(0);
        let result = validate_value(
            &compiled.input,
            handler_id,
            &value,
            SchemaDirection::Input,
            &workflow_state,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(
                err,
                RunWorkflowError::SchemaValidation {
                    direction: SchemaDirection::Input,
                    ..
                }
            ),
            "expected SchemaValidation(Input), got: {err:?}"
        );
    }

    /// validate_value returns Ok when no validator is compiled for the handler.
    #[test]
    fn validate_value_no_validator() {
        let flat_config = flatten(config(ts_invoke("./handler.ts", "run"))).unwrap();
        let workflow_state = WorkflowState::new(flat_config);
        let compiled = compile_schemas(&workflow_state).unwrap();

        let value = serde_json::json!({"anything": "goes"});
        let handler_id = HandlerId(0);
        let result = validate_value(
            &compiled.input,
            handler_id,
            &value,
            SchemaDirection::Input,
            &workflow_state,
        );
        assert!(result.is_ok());
    }

    /// Builtin handlers are never validated (no schemas compiled).
    #[test]
    fn compile_schemas_skips_builtins() {
        let schema = serde_json::json!({ "type": "integer" });
        // Chain: Constant(42) → TypeScript handler with output schema.
        // The constant builtin should not have any validators.
        let flat_config = flatten(config(Action::Chain(barnum_ast::ChainAction {
            first: Box::new(constant(serde_json::json!(42))),
            rest: Box::new(ts_invoke_with_schemas(
                "./handler.ts",
                "run",
                None,
                Some(schema),
            )),
        })))
        .unwrap();
        let workflow_state = WorkflowState::new(flat_config);
        let compiled = compile_schemas(&workflow_state).unwrap();

        // Only the TypeScript handler has an output validator.
        assert!(compiled.input.is_empty());
        assert_eq!(compiled.output.len(), 1);
    }

    // =========================================================================
    // Integration tests: validation through run_workflow
    // =========================================================================

    /// Input validation failure terminates run_workflow with SchemaValidation.
    ///
    /// Chain(Constant("bad"), TS_handler_with_input_schema({type: integer})):
    /// the constant produces a string, the TS handler expects an integer.
    /// Validation catches the mismatch before the handler is dispatched.
    #[tokio::test]
    async fn run_workflow_rejects_bad_input() {
        let flat_config = flatten(config(Action::Chain(barnum_ast::ChainAction {
            first: Box::new(constant(serde_json::json!("not-an-integer"))),
            rest: Box::new(ts_invoke_with_schemas(
                "./handler.ts",
                "run",
                Some(serde_json::json!({ "type": "integer" })),
                None,
            )),
        })))
        .unwrap();
        let mut workflow_state = WorkflowState::new(flat_config);
        let mut scheduler = test_scheduler();

        let result = run_workflow(&mut workflow_state, &mut scheduler).await;
        match result {
            Err(RunWorkflowError::SchemaValidation {
                direction: SchemaDirection::Input,
                ..
            }) => {} // expected
            Err(other) => panic!("expected SchemaValidation(Input), got: {other:?}"),
            Ok(value) => panic!("expected SchemaValidation error, got Ok({value})"),
        }
    }

    /// Output validation failure terminates run_workflow with SchemaValidation.
    ///
    /// Single TS handler with output_schema({type: integer}). A fake
    /// completion with a string value is injected into the scheduler channel.
    /// Validation catches the mismatch after the handler "completes".
    #[tokio::test]
    async fn run_workflow_rejects_bad_output() {
        let flat_config = flatten(config(ts_invoke_with_schemas(
            "./handler.ts",
            "run",
            None,
            Some(serde_json::json!({ "type": "integer" })),
        )))
        .unwrap();
        let mut workflow_state = WorkflowState::new(flat_config);
        let mut scheduler = test_scheduler();

        // Pre-inject a fake completion for TaskId(0) with a value that
        // violates the output schema. This arrives before any real handler
        // result because it's already buffered in the channel.
        scheduler.inject_completion(TaskId(0), serde_json::json!("not-an-integer"));

        let result = run_workflow(&mut workflow_state, &mut scheduler).await;
        match result {
            Err(RunWorkflowError::SchemaValidation {
                direction: SchemaDirection::Output,
                ..
            }) => {} // expected
            Err(other) => panic!("expected SchemaValidation(Output), got: {other:?}"),
            Ok(value) => panic!("expected SchemaValidation error, got Ok({value})"),
        }
    }
}
