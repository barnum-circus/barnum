//! Response processing and retry logic.

use agent_pool::Response;
use tracing::{debug, error, info, warn};

use crate::resolved::{Options, Step};
use crate::value_schema::{CompiledSchemas, Task, validate_response};

use super::PostHookInput;
use super::types::{EffectiveValue, SubmitResult, TaskOutcome};

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
                let (outcome, post_input) =
                    process_pool_response(response, task, &effective_value, step, schemas);
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
                        input: effective_value.0,
                        error: e.to_string(),
                    },
                }
            }
        },
        SubmitResult::Command {
            effective_value,
            output,
        } => match output {
            Ok(stdout) => {
                let (outcome, post_input) =
                    process_command_response(&stdout, task, &effective_value, step, schemas);
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
                        input: effective_value.0,
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
    }
}

/// Process a response from the agent pool.
fn process_pool_response(
    response: Response,
    task: &Task,
    effective_value: &EffectiveValue,
    step: &Step,
    schemas: &CompiledSchemas,
) -> (TaskOutcome, PostHookInput) {
    match response {
        Response::Processed { stdout, .. } => {
            debug!(stdout = %stdout, "agent response");
            process_stdout(&stdout, task, effective_value, step, schemas)
        }
        Response::NotProcessed { reason } => {
            warn!(step = %task.step, ?reason, "task outcome unknown");
            let outcome = process_retry(task, &step.options, FailureKind::Timeout);
            let post_input = PostHookInput::Timeout {
                input: effective_value.0.clone(),
            };
            (outcome, post_input)
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
) -> (TaskOutcome, PostHookInput) {
    debug!(stdout = %stdout, "command output");
    process_stdout(stdout, task, effective_value, step, schemas)
}

/// Process stdout from either pool or command action.
fn process_stdout(
    stdout: &str,
    task: &Task,
    effective_value: &EffectiveValue,
    step: &Step,
    schemas: &CompiledSchemas,
) -> (TaskOutcome, PostHookInput) {
    match serde_json::from_str::<serde_json::Value>(stdout) {
        Ok(output_value) => match validate_response(&output_value, step, schemas) {
            Ok(new_tasks) => {
                info!(from = %task.step, new_tasks = new_tasks.len(), "task completed");
                let post_input = PostHookInput::Success {
                    input: effective_value.0.clone(),
                    output: output_value,
                    next: new_tasks.clone(),
                };
                let outcome = TaskOutcome::Success {
                    spawned: new_tasks,
                    finally_value: effective_value.clone(),
                };
                (outcome, post_input)
            }
            Err(e) => {
                warn!(step = %task.step, error = %e, "invalid response");
                let outcome = process_retry(task, &step.options, FailureKind::InvalidResponse);
                let post_input = PostHookInput::Error {
                    input: effective_value.0.clone(),
                    error: e.to_string(),
                };
                (outcome, post_input)
            }
        },
        Err(e) => {
            warn!(step = %task.step, error = %e, stdout = %stdout, "failed to parse response JSON");
            let outcome = process_retry(task, &step.options, FailureKind::InvalidResponse);
            let post_input = PostHookInput::Error {
                input: effective_value.0.clone(),
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
        return TaskOutcome::Dropped;
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
        TaskOutcome::Retry(retry_task)
    } else {
        error!(step = %task.step, retries = retry_task.retries, "max retries exceeded");
        TaskOutcome::Dropped
    }
}
