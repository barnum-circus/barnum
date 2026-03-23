//! Task dispatch - spawns threads to execute tasks and process results.
//!
//! Workers run actions, sending unified results back on the channel for
//! main-thread processing via response module functions.

use std::path::Path;
use std::sync::mpsc;

use tracing::debug;
use troupe::Response;

use crate::types::{HookScript, LogTaskId, StepInputValue};
use crate::value_schema::Task;

use super::action::ActionError;
use super::shell::run_shell_command;
use super::submit::{build_agent_payload, submit_via_cli};

/// Unified action output.
pub(super) struct ActionResult {
    pub value: StepInputValue,
    pub output: Result<String, ActionError>,
}

/// Routing tag: determines whether result goes to `convert_task_result` or `convert_finally_result`.
pub(super) enum WorkerKind {
    Task,
    Finally { parent_id: LogTaskId },
}

/// Result from a worker thread: the task identity, routing tag, and action output.
pub struct WorkerResult {
    pub task_id: LogTaskId,
    pub task: Task,
    pub kind: WorkerKind,
    pub result: ActionResult,
}

/// Execute a pool task (runs in spawned thread).
///
/// Submits to the agent pool and sends the unified result back on the channel.
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

    let output = match submit_via_cli(&pool.root, &payload, &pool.invoker) {
        Ok(Response::Processed { stdout, .. }) => Ok(stdout),
        Ok(Response::NotProcessed { .. }) => {
            Err(ActionError::Failed("not processed by pool".into()))
        }
        Err(e) => Err(ActionError::Failed(e.to_string())),
    };
    let _ = tx.send(WorkerResult {
        task_id,
        task,
        kind: WorkerKind::Task,
        result: ActionResult { value, output },
    });
}

/// Execute a command task (runs in spawned thread).
///
/// Executes the shell command and sends the unified result back on the channel.
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

    let output =
        run_shell_command(script, &task_json, Some(working_dir)).map_err(ActionError::Failed);
    let _ = tx.send(WorkerResult {
        task_id,
        task,
        kind: WorkerKind::Task,
        result: ActionResult { value, output },
    });
}

/// Execute a finally task (runs in spawned thread).
///
/// Runs the finally script and sends the unified result back on the channel.
pub fn dispatch_finally_task(
    parent_id: LogTaskId,
    task: Task,
    finally_script: &HookScript,
    working_dir: &Path,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let value = task.value.clone();
    let input_json = serde_json::to_string(&serde_json::json!({
        "kind": &task.step,
        "value": &value.0,
    }))
    .unwrap_or_default();

    let output = run_shell_command(finally_script.as_str(), &input_json, Some(working_dir))
        .map_err(ActionError::Failed);
    let _ = tx.send(WorkerResult {
        task_id: parent_id,
        task,
        kind: WorkerKind::Finally { parent_id },
        result: ActionResult { value, output },
    });
}
