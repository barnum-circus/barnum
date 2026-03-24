//! Response processing and retry logic.

use tracing::{debug, error, info, warn};

use std::collections::HashMap;

use crate::config::{ActionKind, EffectiveOptions, Step};
use crate::types::{StepInputValue, StepName, Task};

use super::action::{ActionError, ActionResult};

/// Task succeeded, may have spawned children.
pub struct TaskSuccess {
    pub spawned: Vec<Task>,
    pub finally_value: StepInputValue,
}

/// Outcome of processing a task submission.
///
/// Separates spawned children (from successful execution) from retries (failed execution).
/// This distinction is crucial for finally hook tracking:
/// - Spawned children are "descendants" that the parent waits for
/// - Retries are continuations of the same logical task, not new descendants
pub enum TaskOutcome {
    /// Task succeeded, may have spawned children.
    Success(TaskSuccess),
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

/// Process a unified action result into a task outcome.
pub fn process_submit_result(
    result: ActionResult,
    task: &Task,
    step: &Step,
    options: &EffectiveOptions,
    step_map: &HashMap<&StepName, &Step>,
) -> TaskOutcome {
    match result.output {
        Ok(stdout) => process_stdout(&stdout, task, &result.value, step, options, step_map),
        Err(ActionError::TimedOut) => {
            warn!(step = %task.step, "action timed out");
            process_retry(task, options, FailureKind::Timeout)
        }
        Err(ActionError::Failed(error)) => {
            error!(step = %task.step, %error, "action failed");
            process_retry(task, options, FailureKind::SubmitError)
        }
    }
}

/// Process stdout from either pool or command action.
fn process_stdout(
    stdout: &str,
    task: &Task,
    value: &StepInputValue,
    step: &Step,
    options: &EffectiveOptions,
    step_map: &HashMap<&StepName, &Step>,
) -> TaskOutcome {
    debug!(stdout = %stdout, "action output");
    match json5::from_str::<serde_json::Value>(stdout) {
        Ok(output_value) => match validate_response(&output_value, step, step_map) {
            Ok(new_tasks) => {
                info!(from = %task.step, new_tasks = new_tasks.len(), "task completed");
                TaskOutcome::Success(TaskSuccess {
                    spawned: new_tasks,
                    finally_value: value.clone(),
                })
            }
            Err(e) => {
                warn!(step = %task.step, error = %e, "invalid response");
                process_retry(task, options, FailureKind::InvalidResponse)
            }
        },
        Err(e) => {
            warn!(step = %task.step, error = %e, stdout = %stdout, "failed to parse response JSONC");
            process_retry(task, options, FailureKind::InvalidResponse)
        }
    }
}

/// Process a task failure, returning the appropriate outcome.
pub fn process_retry(
    task: &Task,
    options: &EffectiveOptions,
    failure_kind: FailureKind,
) -> TaskOutcome {
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

// ==================== Response Validation ====================

/// Validate an agent's response: check format, transition validity, and value schemas.
///
/// Checks that:
/// - Response is a JSON array
/// - Each task's kind is a valid next step from the current step
/// - Each task's value matches the target step's JSON Schema (if present)
pub fn validate_response(
    response: &serde_json::Value,
    current_step: &Step,
    step_map: &HashMap<&StepName, &Step>,
) -> Result<Vec<Task>, ResponseValidationError> {
    let serde_json::Value::Array(items) = response else {
        return Err(ResponseValidationError::NotAnArray);
    };

    let mut tasks = Vec::with_capacity(items.len());

    for (i, item) in items.iter().enumerate() {
        let task: Task = serde_json::from_value(item.clone()).map_err(|e| {
            ResponseValidationError::InvalidTaskFormat {
                index: i,
                error: e.to_string(),
            }
        })?;

        // Check valid transition
        if !current_step.next.contains(&task.step) {
            return Err(ResponseValidationError::InvalidTransition {
                from: current_step.name.clone(),
                to: task.step,
                valid: current_step.next.clone(),
            });
        }

        // Validate value against target step's JSON Schema
        if let Some(target_step) = step_map.get(&task.step)
            && let ActionKind::TypeScript(ts) = &target_step.action
            && let Some(schema) = &ts.value_schema
        {
            validate_value_schema(&task.value.0, schema).map_err(|msg| {
                ResponseValidationError::ValueSchemaViolation {
                    index: i,
                    target_step: task.step.clone(),
                    error: msg,
                }
            })?;
        }

        tasks.push(task);
    }

    Ok(tasks)
}

/// Validate a task value against a JSON Schema.
fn validate_value_schema(
    value: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<(), String> {
    let compiled =
        jsonschema::validator_for(schema).map_err(|e| format!("invalid JSON Schema: {e}"))?;
    let errors: Vec<String> = compiled.iter_errors(value).map(|e| e.to_string()).collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Errors that can occur when validating an agent response.
#[derive(Debug)]
pub enum ResponseValidationError {
    /// Response is not a JSON array.
    NotAnArray,
    /// A task in the array has invalid format.
    InvalidTaskFormat {
        /// Index of the invalid task.
        index: usize,
        /// Parse error message.
        error: String,
    },
    /// Task step is not a valid transition from current step.
    InvalidTransition {
        /// Current step name.
        from: StepName,
        /// Attempted next step.
        to: StepName,
        /// List of valid next steps.
        valid: Vec<StepName>,
    },
    /// Task value doesn't match the target step's JSON Schema.
    ValueSchemaViolation {
        /// Index of the invalid task.
        index: usize,
        /// Target step name.
        target_step: StepName,
        /// Validation error message.
        error: String,
    },
}

impl std::fmt::Display for ResponseValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnArray => write!(f, "response must be a JSON array"),
            Self::InvalidTaskFormat { index, error } => {
                write!(f, "task at index {index} has invalid format: {error}")
            }
            Self::InvalidTransition { from, to, valid } => {
                let valid_str: Vec<&str> = valid.iter().map(StepName::as_str).collect();
                write!(
                    f,
                    "invalid transition from '{from}' to '{to}' (valid: {})",
                    valid_str.join(", ")
                )
            }
            Self::ValueSchemaViolation {
                index,
                target_step,
                error,
            } => {
                write!(
                    f,
                    "task at index {index} targeting '{target_step}' failed schema validation: {error}"
                )
            }
        }
    }
}

impl std::error::Error for ResponseValidationError {}
