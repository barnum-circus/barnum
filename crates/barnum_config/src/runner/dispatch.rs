//! Task dispatch types and finally dispatch.
//!
//! Workers run actions, sending unified results back on the channel for
//! main-thread processing via response module functions.

use std::path::Path;
use std::sync::mpsc;

use crate::types::{HookScript, LogTaskId, StepInputValue};
use crate::value_schema::Task;

use super::action::ActionError;
use super::shell::run_shell_command;

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
