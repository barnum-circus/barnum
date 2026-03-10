//! Task dispatch - spawns threads to execute pool and command tasks.

use std::io;
use std::path::Path;
use std::sync::mpsc;

use cli_invoker::Invoker;
use tracing::debug;
use troupe::Response;
use troupe_cli::TroupeCli;

use crate::types::{HookScript, LogTaskId, StepInputValue};
use crate::value_schema::Task;

use super::hooks::{run_command_action, run_pre_hook};
use super::shell::run_shell_command;
use super::submit::{build_agent_payload, submit_via_cli};

/// Identity of a task being processed.
#[derive(Clone)]
pub struct TaskIdentity {
    pub task: Task,
    pub task_id: LogTaskId,
}

/// Result of task execution, returned from dispatch threads.
pub struct InFlightResult {
    pub identity: TaskIdentity,
    pub result: SubmitResult,
}

/// Result of task submission. Value only exists when pre-hook succeeded.
pub enum SubmitResult {
    Pool {
        value: StepInputValue,
        response: io::Result<Response>,
    },
    Command {
        value: StepInputValue,
        output: io::Result<String>,
    },
    /// Result from a finally task (no pre-hook, stdout parsed as task array).
    Finally {
        value: StepInputValue,
        output: Result<String, String>,
    },
    PreHookError(String),
}

/// Context for dispatching a task.
pub struct TaskContext {
    pub identity: TaskIdentity,
    pub pre_hook: Option<HookScript>,
}

/// Run pre-hook if present, returning the value or sending an error result.
///
/// Returns `Some(StepInputValue)` to continue processing, `None` if error was sent.
fn run_pre_hook_or_send_error(
    ctx: &TaskContext,
    original_value: &StepInputValue,
    working_dir: &Path,
    tx: &mpsc::Sender<InFlightResult>,
) -> Option<StepInputValue> {
    let Some(hook) = &ctx.pre_hook else {
        // No pre-hook, original value passes through unchanged
        return Some(original_value.clone());
    };

    match run_pre_hook(hook, &original_value.0, working_dir) {
        Ok(v) => Some(StepInputValue(v)),
        Err(e) => {
            let _ = tx.send(InFlightResult {
                identity: ctx.identity.clone(),
                result: SubmitResult::PreHookError(e),
            });
            None
        }
    }
}

/// Execute a pool task (runs in spawned thread).
pub fn dispatch_pool_task(
    ctx: TaskContext,
    docs: &str,
    timeout: Option<u64>,
    pool_root: &Path,
    working_dir: &Path,
    invoker: &Invoker<TroupeCli>,
    tx: &mpsc::Sender<InFlightResult>,
) {
    let original_value = &ctx.identity.task.value;

    let Some(value) = run_pre_hook_or_send_error(&ctx, original_value, working_dir, tx) else {
        return;
    };

    let payload = build_agent_payload(&ctx.identity.task.step, &value.0, docs, timeout);
    debug!(payload = %payload, "task payload");

    let response = submit_via_cli(pool_root, &payload, invoker);
    let _ = tx.send(InFlightResult {
        identity: ctx.identity,
        result: SubmitResult::Pool { value, response },
    });
}

/// Execute a command task (runs in spawned thread).
pub fn dispatch_command_task(
    ctx: TaskContext,
    script: &str,
    working_dir: &Path,
    tx: &mpsc::Sender<InFlightResult>,
) {
    let original_value = &ctx.identity.task.value;

    let Some(value) = run_pre_hook_or_send_error(&ctx, original_value, working_dir, tx) else {
        return;
    };

    let task_json = serde_json::to_string(&serde_json::json!({
        "kind": &ctx.identity.task.step,
        "value": &value.0,
    }))
    .unwrap_or_default();

    let output = run_command_action(script, &task_json, working_dir);
    let _ = tx.send(InFlightResult {
        identity: ctx.identity,
        result: SubmitResult::Command { value, output },
    });
}

/// Execute a finally task (runs in spawned thread).
///
/// Finally tasks have no pre-hook. The script is run directly with the value as JSON input.
/// Output is parsed as a task array by the receiver.
pub fn dispatch_finally_task(
    identity: TaskIdentity,
    script: &HookScript,
    working_dir: &Path,
    tx: &mpsc::Sender<InFlightResult>,
) {
    let value = identity.task.value.clone();
    let input_json = serde_json::to_string(&value.0).unwrap_or_default();

    let output = run_shell_command(script.as_str(), &input_json, Some(working_dir));
    let _ = tx.send(InFlightResult {
        identity,
        result: SubmitResult::Finally { value, output },
    });
}
