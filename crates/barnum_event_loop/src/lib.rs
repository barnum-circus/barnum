//! Barnum workflow runtime: Scheduler + `run_workflow` loop.
//!
//! The [`Scheduler`] dispatches handler invocations as tokio tasks and collects
//! results via an internal channel. [`run_workflow`] drives the
//! [`WorkflowState`] by repeatedly dispatching pending work and feeding
//! completions back until the workflow terminates.

pub mod builtins;

use barnum_ast::HandlerKind;
use barnum_engine::{CompleteError, Dispatch, TaskId, WorkflowState};
use builtins::{BuiltinError, execute_builtin};
use intern::Lookup;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;

// =============================================================================
// HandlerError
// =============================================================================

/// Unified error type for handler execution failures.
#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    /// A builtin handler encountered a type mismatch.
    #[error(transparent)]
    Builtin(#[from] BuiltinError),
    /// A TypeScript subprocess exited with a non-zero exit code.
    #[error("handler {module}:{func} failed (exit {exit_code}): {stderr}")]
    SubprocessFailed {
        /// Module path of the failed handler.
        module: String,
        /// Export name of the failed handler.
        func: String,
        /// Process exit code.
        exit_code: i32,
        /// Captured stderr output.
        stderr: String,
    },
    /// A TypeScript subprocess returned invalid JSON on stdout.
    #[error("handler {module}:{func} returned invalid JSON: {source}")]
    InvalidOutput {
        /// Module path of the failed handler.
        module: String,
        /// Export name of the failed handler.
        func: String,
        /// The JSON parsing error.
        source: serde_json::Error,
    },
}

// =============================================================================
// Scheduler
// =============================================================================

/// How the scheduler executes TypeScript handler invocations.
enum ExecutionMode {
    /// No-op: every TypeScript handler returns `{}`. Used for tests.
    Noop,
    /// Spawn a subprocess per TypeScript invocation: `{executor} {worker_path} <module> <func>`.
    Subprocess {
        /// The command to invoke the worker, e.g. `"node /path/to/tsx/cli.mjs"`.
        executor: String,
        /// Path to `worker.ts`.
        worker_path: String,
    },
}

/// Dispatches handler invocations as tokio tasks and collects results.
///
/// Each [`dispatch`](Scheduler::dispatch) call spawns a lightweight tokio task.
/// Results are collected via [`recv`](Scheduler::recv).
pub struct Scheduler {
    result_tx: mpsc::UnboundedSender<(TaskId, Result<Value, HandlerError>)>,
    result_rx: mpsc::UnboundedReceiver<(TaskId, Result<Value, HandlerError>)>,
    mode: ExecutionMode,
}

impl Scheduler {
    /// Create a no-op scheduler (TypeScript handlers return `{}`). Used for tests.
    #[must_use]
    pub fn new() -> Self {
        let (result_tx, result_rx) = mpsc::unbounded_channel();
        Self {
            result_tx,
            result_rx,
            mode: ExecutionMode::Noop,
        }
    }

    /// Create a scheduler that spawns one subprocess per TypeScript handler invocation.
    ///
    /// `executor` is the command to run TypeScript, e.g. `"node /path/to/tsx/cli.mjs"`.
    /// `worker_path` is the absolute path to `worker.ts`.
    #[must_use]
    pub fn with_executor(executor: String, worker_path: String) -> Self {
        let (result_tx, result_rx) = mpsc::unbounded_channel();
        Self {
            result_tx,
            result_rx,
            mode: ExecutionMode::Subprocess {
                executor,
                worker_path,
            },
        }
    }

    /// Dispatch a handler invocation.
    ///
    /// Spawns a tokio task that executes the handler and sends the result
    /// through the internal channel. Builtins are executed inline within
    /// the spawned task. TypeScript handlers spawn a subprocess.
    pub fn dispatch(&self, dispatch: &Dispatch, handler: &HandlerKind) {
        let result_tx = self.result_tx.clone();
        let task_id = dispatch.task_id;

        match handler {
            HandlerKind::Builtin(builtin_handler) => {
                let builtin_kind = builtin_handler.builtin.clone();
                let value = dispatch.value.clone();
                tokio::spawn(async move {
                    let result = execute_builtin(&builtin_kind, &value).map_err(HandlerError::from);
                    let _ = result_tx.send((task_id, result));
                });
            }
            HandlerKind::TypeScript(ts) => match &self.mode {
                ExecutionMode::Noop => {
                    tokio::spawn(async move {
                        let value = Value::Object(serde_json::Map::default());
                        let _ = result_tx.send((task_id, Ok(value)));
                    });
                }
                ExecutionMode::Subprocess {
                    executor,
                    worker_path,
                } => {
                    let module = ts.module.lookup().to_owned();
                    let func = ts.func.lookup().to_owned();
                    let value = dispatch.value.clone();
                    let executor = executor.clone();
                    let worker_path = worker_path.clone();

                    tokio::spawn(async move {
                        let result =
                            execute_typescript(&executor, &worker_path, &module, &func, &value)
                                .await;
                        let _ = result_tx.send((task_id, result));
                    });
                }
            },
        }
    }

    /// Wait for the next handler result.
    ///
    /// Returns `None` if all senders have been dropped (shouldn't happen
    /// during normal operation since `self` holds a sender).
    pub async fn recv(&mut self) -> Option<(TaskId, Result<Value, HandlerError>)> {
        self.result_rx.recv().await
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// TypeScript subprocess execution
// =============================================================================

/// Execute a TypeScript handler by spawning a subprocess.
///
/// Protocol:
///   stdin  → `{ "value": <input> }` (JSON)
///   stdout ← handler result (JSON)
///
/// # Panics
///
/// Panics if the subprocess fails to spawn or stdin can't be written.
/// Non-zero exit and invalid JSON are returned as [`HandlerError`].
#[allow(clippy::expect_used)]
async fn execute_typescript(
    executor: &str,
    worker_path: &str,
    module: &str,
    func: &str,
    value: &Value,
) -> Result<Value, HandlerError> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("{executor} {worker_path} {module} {func}"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn handler process");

    // Write input to stdin and close it
    let mut stdin = child.stdin.take().expect("no stdin");
    let input =
        serde_json::to_vec(&serde_json::json!({ "value": value })).expect("serialize failed");
    stdin.write_all(&input).await.expect("stdin write failed");
    drop(stdin);

    // Read stdout + wait for exit
    let output = child.wait_with_output().await.expect("wait failed");
    if !output.status.success() {
        return Err(HandlerError::SubprocessFailed {
            module: module.to_owned(),
            func: func.to_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    serde_json::from_slice(&output.stdout).map_err(|source| HandlerError::InvalidOutput {
        module: module.to_owned(),
        func: func.to_owned(),
        source,
    })
}

// =============================================================================
// run_workflow
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
    let root = workflow_state.workflow_root();
    workflow_state
        .advance(root, Value::Null, None)
        .expect("initial advance failed");

    loop {
        let dispatches = workflow_state.take_pending_dispatches();
        for dispatch in &dispatches {
            let handler = workflow_state.handler(dispatch.handler_id);
            scheduler.dispatch(dispatch, handler);
        }

        let (task_id, result) = scheduler
            .recv()
            .await
            .expect("scheduler channel closed unexpectedly");

        let value = result?;

        if let Some(terminal_value) = workflow_state.complete(task_id, value)? {
            return Ok(terminal_value);
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use barnum_ast::flat::flatten;
    use barnum_ast::{Action, Config, TypeScriptHandler};
    use intern::string_key::Intern as _;
    use std::collections::HashMap;

    fn ts_handler(module: &str, func: &str) -> Action {
        Action::Invoke(barnum_ast::InvokeAction {
            handler: HandlerKind::TypeScript(TypeScriptHandler {
                module: module.intern().into(),
                func: func.intern().into(),
                step_config_schema: None,
                value_schema: None,
            }),
        })
    }

    fn config(workflow: Action) -> Config {
        Config {
            workflow,
            steps: HashMap::default(),
        }
    }

    #[tokio::test]
    async fn chain_of_two_invokes() {
        let flat_config = flatten(config(Action::Chain(barnum_ast::ChainAction {
            first: Box::new(ts_handler("./a.ts", "a")),
            rest: Box::new(ts_handler("./b.ts", "b")),
        })))
        .unwrap();
        let mut workflow_state = WorkflowState::new(flat_config);
        let mut scheduler = Scheduler::new();

        let result = run_workflow(&mut workflow_state, &mut scheduler)
            .await
            .unwrap();

        // Both handlers return {}, so the final result is {}
        assert_eq!(result, serde_json::json!({}));
    }

    #[tokio::test]
    async fn parallel_two_invokes() {
        let flat_config = flatten(config(Action::Parallel(barnum_ast::ParallelAction {
            actions: vec![ts_handler("./a.ts", "a"), ts_handler("./b.ts", "b")],
        })))
        .unwrap();
        let mut workflow_state = WorkflowState::new(flat_config);
        let mut scheduler = Scheduler::new();

        let result = run_workflow(&mut workflow_state, &mut scheduler)
            .await
            .unwrap();

        // Parallel collects results into an array
        assert_eq!(result, serde_json::json!([{}, {}]));
    }

    #[tokio::test]
    async fn single_invoke() {
        let flat_config = flatten(config(ts_handler("./a.ts", "a"))).unwrap();
        let mut workflow_state = WorkflowState::new(flat_config);
        let mut scheduler = Scheduler::new();

        let result = run_workflow(&mut workflow_state, &mut scheduler)
            .await
            .unwrap();

        assert_eq!(result, serde_json::json!({}));
    }
}
