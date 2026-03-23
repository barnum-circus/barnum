//! Task dispatch - spawns threads to execute tasks and process results.
//!
//! Workers run actions, sending raw results back on the channel.
//! `process_and_finalize` handles validation and post-hooks on the main thread.

use std::io;
use std::path::Path;
use std::sync::mpsc;

use tracing::{debug, warn};
use troupe::Response;

use crate::resolved::Step;
use crate::types::{HookScript, LogTaskId, StepInputValue};
use crate::value_schema::{CompiledSchemas, Task};

use super::hooks::{PostHookInput, PostHookSuccess, run_command_action, run_post_hook};
use super::response::{
    FailureKind, ProcessedSubmit, TaskOutcome, TaskSuccess, process_retry, process_submit_result,
};
use super::shell::run_shell_command;
use super::submit::{build_agent_payload, submit_via_cli};

/// Result from a worker thread: the task identity and raw action result.
///
/// Workers handle pre-hooks and actions. The main thread processes the raw
/// result through validation, post-hooks, and retry logic via [`process_and_finalize`].
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

/// Process a raw submit result through validation, post-hook, and retry logic.
///
/// Called on the main thread after receiving a [`WorkerResult`].
pub(super) fn process_and_finalize(
    result: SubmitResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
    working_dir: &Path,
) -> TaskOutcome {
    let ProcessedSubmit {
        outcome,
        post_input,
    } = process_submit_result(result, task, step, schemas);

    // Post hook can modify the outcome (e.g., filter spawned tasks)
    if let Some(hook) = &step.post {
        match run_post_hook(hook, &post_input, working_dir) {
            Ok(modified) => match outcome {
                TaskOutcome::Success(TaskSuccess { finally_value, .. }) => {
                    let tasks = extract_next_tasks(&modified);
                    TaskOutcome::Success(TaskSuccess {
                        spawned: tasks,
                        finally_value,
                    })
                }
                other => other,
            },
            Err(e) => {
                warn!(step = %task.step, error = %e, "post hook failed");
                process_retry(task, &step.options, FailureKind::SubmitError)
            }
        }
    } else {
        outcome
    }
}

/// Extract next tasks from a post hook result.
fn extract_next_tasks(input: &PostHookInput) -> Vec<Task> {
    match input {
        PostHookInput::Success(PostHookSuccess { next, .. }) => next.clone(),
        PostHookInput::Timeout(..) | PostHookInput::Error(..) => {
            vec![]
        }
    }
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
