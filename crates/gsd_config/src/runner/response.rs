//! Response processing and retry logic.

use agent_pool::Response;
use tracing::{debug, error, info, warn};

use crate::resolved::{Options, Step};
use crate::value_schema::{CompiledSchemas, Task, validate_response};

use super::types::{EffectiveValue, SubmitResult};
use super::{PostHookInput, TaskResult};

/// Why a task failed and needs retry consideration.
#[derive(Debug, Clone, Copy)]
pub enum FailureKind {
    Timeout,
    InvalidResponse,
    SubmitError,
}

/// Output from processing a submit result.
pub struct ProcessedSubmit {
    pub result: TaskResult,
    pub tasks: Vec<Task>,
    pub post_input: PostHookInput,
    /// Value to pass to finally hook (effective value if pre-hook ran, else original).
    pub finally_value: serde_json::Value,
}

/// Process a submit result, extracting `effective_value` where it exists.
pub fn process_submit_result(
    result: SubmitResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
) -> ProcessedSubmit {
    match result {
        SubmitResult::Pool {
            effective_value,
            response,
        } => match response {
            Ok(response) => {
                let (result, tasks, post_input) =
                    process_pool_response(response, task, &effective_value, step, schemas);
                ProcessedSubmit {
                    result,
                    tasks,
                    post_input,
                    finally_value: effective_value.0,
                }
            }
            Err(e) => {
                error!(step = %task.step, error = %e, "submit failed");
                let (result, tasks) = process_retry(task, &step.options, FailureKind::SubmitError);
                ProcessedSubmit {
                    result,
                    tasks,
                    post_input: PostHookInput::Error {
                        input: effective_value.0.clone(),
                        error: e.to_string(),
                    },
                    finally_value: effective_value.0,
                }
            }
        },
        SubmitResult::Command {
            effective_value,
            output,
        } => match output {
            Ok(stdout) => {
                let (result, tasks, post_input) =
                    process_command_response(&stdout, task, &effective_value, step, schemas);
                ProcessedSubmit {
                    result,
                    tasks,
                    post_input,
                    finally_value: effective_value.0,
                }
            }
            Err(e) => {
                error!(step = %task.step, error = %e, "command failed");
                let (result, tasks) = process_retry(task, &step.options, FailureKind::SubmitError);
                ProcessedSubmit {
                    result,
                    tasks,
                    post_input: PostHookInput::Error {
                        input: effective_value.0.clone(),
                        error: e.to_string(),
                    },
                    finally_value: effective_value.0,
                }
            }
        },
        SubmitResult::PreHookError(e) => {
            error!(step = %task.step, error = %e, "pre hook failed");
            let (result, tasks) = process_retry(task, &step.options, FailureKind::SubmitError);
            ProcessedSubmit {
                result,
                tasks,
                post_input: PostHookInput::PreHookError {
                    input: task.value.clone(),
                    error: e,
                },
                // Pre-hook failed, so use original task value for finally hook
                finally_value: task.value.clone(),
            }
        }
    }
}

/// Process a response from the agent pool.
fn process_pool_response(
    response: Response,
    task: &Task,
    effective_value: &EffectiveValue,
    step: &Step,
    schemas: &CompiledSchemas,
) -> (TaskResult, Vec<Task>, PostHookInput) {
    match response {
        Response::Processed { stdout, .. } => {
            debug!(stdout = %stdout, "agent response");
            process_stdout(&stdout, task, &effective_value.0, step, schemas)
        }
        Response::NotProcessed { reason } => {
            warn!(step = %task.step, ?reason, "task outcome unknown");
            let (result, tasks) = process_retry(task, &step.options, FailureKind::Timeout);
            let post_input = PostHookInput::Timeout {
                input: effective_value.0.clone(),
            };
            (result, tasks, post_input)
        }
    }
}

/// Process stdout from a command action.
fn process_command_response(
    stdout: &str,
    task: &Task,
    effective_value: &EffectiveValue,
    step: &Step,
    schemas: &CompiledSchemas,
) -> (TaskResult, Vec<Task>, PostHookInput) {
    debug!(stdout = %stdout, "command output");
    process_stdout(stdout, task, &effective_value.0, step, schemas)
}

/// Process stdout from either pool or command action.
fn process_stdout(
    stdout: &str,
    task: &Task,
    effective_value: &serde_json::Value,
    step: &Step,
    schemas: &CompiledSchemas,
) -> (TaskResult, Vec<Task>, PostHookInput) {
    match serde_json::from_str::<serde_json::Value>(stdout) {
        Ok(output_value) => match validate_response(&output_value, step, schemas) {
            Ok(new_tasks) => {
                info!(from = %task.step, new_tasks = new_tasks.len(), "task completed");
                let post_input = PostHookInput::Success {
                    input: effective_value.clone(),
                    output: output_value,
                    next: new_tasks.clone(),
                };
                (TaskResult::Completed, new_tasks, post_input)
            }
            Err(e) => {
                warn!(step = %task.step, error = %e, "invalid response");
                let (result, tasks) =
                    process_retry(task, &step.options, FailureKind::InvalidResponse);
                let post_input = PostHookInput::Error {
                    input: effective_value.clone(),
                    error: e.to_string(),
                };
                (result, tasks, post_input)
            }
        },
        Err(e) => {
            warn!(step = %task.step, error = %e, stdout = %stdout, "failed to parse response JSON");
            let (result, tasks) = process_retry(task, &step.options, FailureKind::InvalidResponse);
            let post_input = PostHookInput::Error {
                input: effective_value.clone(),
                error: format!("failed to parse response JSON: {e}"),
            };
            (result, tasks, post_input)
        }
    }
}

/// Process a task failure, potentially creating a retry task.
pub fn process_retry(
    task: &Task,
    options: &Options,
    failure_kind: FailureKind,
) -> (TaskResult, Vec<Task>) {
    let retry_allowed = match failure_kind {
        FailureKind::Timeout => options.retry_on_timeout,
        FailureKind::InvalidResponse => options.retry_on_invalid_response,
        FailureKind::SubmitError => true,
    };

    if !retry_allowed {
        warn!(step = %task.step, failure = ?failure_kind, "retry disabled, dropping task");
        return (TaskResult::Dropped, vec![]);
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
        (TaskResult::Requeued, vec![retry_task])
    } else {
        error!(step = %task.step, retries = retry_task.retries, "max retries exceeded");
        (TaskResult::Dropped, vec![])
    }
}
