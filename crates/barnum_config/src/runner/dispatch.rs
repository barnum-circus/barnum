//! Task dispatch - spawns threads to execute tasks and process results.
//!
//! Workers run actions, sending raw results back on the channel for
//! main-thread processing via response module functions.

use std::io;
use std::path::Path;
use std::sync::mpsc;

use tracing::debug;
use troupe::Response;

use crate::types::{HookScript, LogTaskId, StepInputValue};
use crate::value_schema::Task;

use super::hooks::run_command_action;
use super::shell::run_shell_command;
use super::submit::{build_agent_payload, submit_via_cli};

/// Result from a worker thread: the task identity and raw action result.
pub struct WorkerResult {
    pub task_id: LogTaskId,
    pub task: Task,
    pub result: SubmitResult,
}

/// Raw result from a pool action.
pub(super) struct PoolResult {
    pub value: StepInputValue,
    pub response: io::Result<Response>,
}

/// Raw result from a command action.
pub(super) struct CommandResult {
    pub value: StepInputValue,
    pub output: io::Result<String>,
}

/// Raw result from a finally hook.
pub(super) struct FinallyResult {
    pub value: StepInputValue,
    pub output: Result<String, String>,
}

/// Raw result of task execution (internal to runner module).
pub(super) enum SubmitResult {
    Pool(PoolResult),
    Command(CommandResult),
    Finally(FinallyResult),
}

/// Execute a pool task (runs in spawned thread).
///
/// Submits to the agent pool and sends the raw result back on the channel
/// for main-thread processing.
pub fn dispatch_pool_task(
    task_id: LogTaskId,
    task: Task,
    docs: &str,
    timeout: Option<u64>,
    pool: &super::PoolConnection,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let value = task.value.clone();
    let payload = build_agent_payload(&task.step, &value.0, docs, timeout);
    debug!(payload = %payload, "task payload");

    let response = submit_via_cli(&pool.root, &payload, &pool.invoker);
    let _ = tx.send(WorkerResult {
        task_id,
        task,
        result: SubmitResult::Pool(PoolResult { value, response }),
    });
}

/// Execute a command task (runs in spawned thread).
///
/// Executes the shell command and sends the raw result back on the channel
/// for main-thread processing.
pub fn dispatch_command_task(
    task_id: LogTaskId,
    task: Task,
    script: &str,
    working_dir: &Path,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let value = task.value.clone();
    let task_json = serde_json::to_string(&serde_json::json!({
        "kind": &task.step,
        "value": &value.0,
    }))
    .unwrap_or_default();

    let output = run_command_action(script, &task_json, working_dir);
    let _ = tx.send(WorkerResult {
        task_id,
        task,
        result: SubmitResult::Command(CommandResult { value, output }),
    });
}

/// Execute a finally task (runs in spawned thread).
///
/// Runs the finally script and sends the raw result back on the channel
/// for main-thread processing.
pub fn dispatch_finally_task(
    task_id: LogTaskId,
    task: Task,
    finally_script: &HookScript,
    working_dir: &Path,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let value = task.value.clone();
    let input_json = serde_json::to_string(&value.0).unwrap_or_default();

    let output = run_shell_command(finally_script.as_str(), &input_json, Some(working_dir));
    let _ = tx.send(WorkerResult {
        task_id,
        task,
        result: SubmitResult::Finally(FinallyResult { value, output }),
    });
}
