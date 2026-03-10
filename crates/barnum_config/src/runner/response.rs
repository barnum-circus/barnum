//! Response processing and retry logic.

use tracing::{debug, error, info, warn};
use troupe::Response;

use crate::resolved::{Options, Step};
use crate::types::StepInputValue;
use crate::value_schema::{CompiledSchemas, Task, validate_response};

use super::dispatch::SubmitResult;
use super::hooks::PostHookInput;

/// Outcome of processing a task submission.
///
/// Separates spawned children (from successful execution) from retries (failed execution).
/// This distinction is crucial for finally hook tracking:
/// - Spawned children are "descendants" that the parent waits for
/// - Retries are continuations of the same logical task, not new descendants
pub enum TaskOutcome {
    /// Task succeeded, may have spawned children.
    Success {
        spawned: Vec<Task>,
        finally_value: StepInputValue,
    },
    /// Task failed, should be retried.
    Retry(Task, FailureKind),
    /// Task failed permanently (max retries exceeded or retry disabled).
    Dropped(FailureKind),
}

/// Why a task failed and needs retry consideration.
#[derive(Debug, Clone, Copy)]
pub enum FailureKind {
    Timeout,
    InvalidResponse,
    SubmitError,
}

/// Output from processing a submit result.
pub struct ProcessedSubmit {
    pub outcome: TaskOutcome,
    pub post_input: PostHookInput,
}

/// Process a submit result, extracting `value` where it exists.
pub fn process_submit_result(
    result: SubmitResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
) -> ProcessedSubmit {
    match result {
        SubmitResult::Pool { value, response } => match response {
            Ok(response) => {
                let (outcome, post_input) =
                    process_pool_response(response, task, &value, step, schemas);
                ProcessedSubmit {
                    outcome,
                    post_input,
                }
            }
            Err(e) => {
                error!(step = %task.step, error = %e, "submit failed");
                let outcome = process_retry(task, &step.options, FailureKind::SubmitError);
                ProcessedSubmit {
                    outcome,
                    post_input: PostHookInput::Error {
                        input: value,
                        error: e.to_string(),
                    },
                }
            }
        },
        SubmitResult::Command { value, output } => match output {
            Ok(stdout) => {
                let (outcome, post_input) =
                    process_command_response(&stdout, task, &value, step, schemas);
                ProcessedSubmit {
                    outcome,
                    post_input,
                }
            }
            Err(e) => {
                error!(step = %task.step, error = %e, "command failed");
                let outcome = process_retry(task, &step.options, FailureKind::SubmitError);
                ProcessedSubmit {
                    outcome,
                    post_input: PostHookInput::Error {
                        input: value,
                        error: e.to_string(),
                    },
                }
            }
        },
        SubmitResult::PreHookError(e) => {
            error!(step = %task.step, error = %e, "pre hook failed");
            let outcome = process_retry(task, &step.options, FailureKind::SubmitError);
            ProcessedSubmit {
                outcome,
                post_input: PostHookInput::PreHookError {
                    input: task.value.clone(),
                    error: e,
                },
            }
        }
        SubmitResult::Finally { value, output } => match output {
            Ok(stdout) => {
                let (outcome, post_input) = process_finally_response(&stdout, task, &value);
                ProcessedSubmit {
                    outcome,
                    post_input,
                }
            }
            Err(e) => {
                error!(step = %task.step, error = %e, "finally hook failed");
                let outcome = process_retry(task, &step.options, FailureKind::SubmitError);
                ProcessedSubmit {
                    outcome,
                    post_input: PostHookInput::Error {
                        input: value,
                        error: e,
                    },
                }
            }
        },
    }
}

/// Process stdout from a finally task.
///
/// Finally hook output is parsed as a JSON array of tasks.
/// Unlike regular tasks, there's no schema validation - finally hooks return raw task objects.
fn process_finally_response(
    stdout: &str,
    task: &Task,
    value: &StepInputValue,
) -> (TaskOutcome, PostHookInput) {
    debug!(stdout = %stdout, "finally hook output");

    match serde_json::from_str::<Vec<Task>>(stdout) {
        Ok(tasks) => {
            info!(step = %task.step, count = tasks.len(), "finally hook completed");
            let post_input = PostHookInput::Success {
                input: value.clone(),
                output: serde_json::json!(tasks),
                next: tasks.clone(),
            };
            let outcome = TaskOutcome::Success {
                spawned: tasks,
                finally_value: value.clone(),
            };
            (outcome, post_input)
        }
        Err(e) => {
            // If output can't be parsed as task array, treat as empty (backwards compatible)
            warn!(step = %task.step, error = %e, "finally hook output is not valid JSON task array");
            let post_input = PostHookInput::Success {
                input: value.clone(),
                output: serde_json::json!([]),
                next: vec![],
            };
            let outcome = TaskOutcome::Success {
                spawned: vec![],
                finally_value: value.clone(),
            };
            (outcome, post_input)
        }
    }
}

/// Process a response from the agent pool.
fn process_pool_response(
    response: Response,
    task: &Task,
    value: &StepInputValue,
    step: &Step,
    schemas: &CompiledSchemas,
) -> (TaskOutcome, PostHookInput) {
    match response {
        Response::Processed { stdout, .. } => {
            debug!(stdout = %stdout, "agent response");
            process_stdout(&stdout, task, value, step, schemas)
        }
        Response::NotProcessed { reason } => {
            warn!(step = %task.step, ?reason, "task outcome unknown");
            let outcome = process_retry(task, &step.options, FailureKind::Timeout);
            let post_input = PostHookInput::Timeout {
                input: value.clone(),
            };
            (outcome, post_input)
        }
    }
}

/// Process stdout from a command action.
fn process_command_response(
    stdout: &str,
    task: &Task,
    value: &StepInputValue,
    step: &Step,
    schemas: &CompiledSchemas,
) -> (TaskOutcome, PostHookInput) {
    debug!(stdout = %stdout, "command output");
    process_stdout(stdout, task, value, step, schemas)
}

/// Process stdout from either pool or command action.
fn process_stdout(
    stdout: &str,
    task: &Task,
    value: &StepInputValue,
    step: &Step,
    schemas: &CompiledSchemas,
) -> (TaskOutcome, PostHookInput) {
    match serde_json::from_str::<serde_json::Value>(stdout) {
        Ok(output_value) => match validate_response(&output_value, step, schemas) {
            Ok(new_tasks) => {
                info!(from = %task.step, new_tasks = new_tasks.len(), "task completed");
                let post_input = PostHookInput::Success {
                    input: value.clone(),
                    output: output_value,
                    next: new_tasks.clone(),
                };
                let outcome = TaskOutcome::Success {
                    spawned: new_tasks,
                    finally_value: value.clone(),
                };
                (outcome, post_input)
            }
            Err(e) => {
                warn!(step = %task.step, error = %e, "invalid response");
                let outcome = process_retry(task, &step.options, FailureKind::InvalidResponse);
                let post_input = PostHookInput::Error {
                    input: value.clone(),
                    error: e.to_string(),
                };
                (outcome, post_input)
            }
        },
        Err(e) => {
            warn!(step = %task.step, error = %e, stdout = %stdout, "failed to parse response JSON");
            let outcome = process_retry(task, &step.options, FailureKind::InvalidResponse);
            let post_input = PostHookInput::Error {
                input: value.clone(),
                error: format!("failed to parse response JSON: {e}"),
            };
            (outcome, post_input)
        }
    }
}

/// Process a task failure, returning the appropriate outcome.
pub fn process_retry(task: &Task, options: &Options, failure_kind: FailureKind) -> TaskOutcome {
    let retry_allowed = match failure_kind {
        FailureKind::Timeout => options.retry_on_timeout,
        FailureKind::InvalidResponse => options.retry_on_invalid_response,
        FailureKind::SubmitError => true,
    };

    if !retry_allowed {
        warn!(step = %task.step, failure = ?failure_kind, "retry disabled, dropping task");
        return TaskOutcome::Dropped(failure_kind);
    }

    let mut retry_task = task.clone();
    retry_task.retries += 1;

    if retry_task.retries <= options.max_retries {
        info!(
            step = %task.step,
            retry = retry_task.retries,
            max = options.max_retries,
            failure = ?failure_kind,
            "requeuing task"
        );
        TaskOutcome::Retry(retry_task, failure_kind)
    } else {
        error!(step = %task.step, retries = retry_task.retries, "max retries exceeded");
        TaskOutcome::Dropped(failure_kind)
    }
}
