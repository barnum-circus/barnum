//! Response processing and retry logic.

use tracing::{debug, error, info, warn};

use crate::resolved::{Options, Step};
use crate::types::StepInputValue;
use crate::value_schema::{CompiledSchemas, Task, validate_response};

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
    schemas: &CompiledSchemas,
) -> TaskOutcome {
    match result.output {
        Ok(stdout) => process_stdout(&stdout, task, &result.value, step, schemas),
        Err(ActionError::TimedOut) => {
            warn!(step = %task.step, "action timed out");
            process_retry(task, &step.options, FailureKind::Timeout)
        }
        Err(ActionError::Failed(error)) => {
            error!(step = %task.step, %error, "action failed");
            process_retry(task, &step.options, FailureKind::SubmitError)
        }
    }
}

/// Process stdout from either pool or command action.
fn process_stdout(
    stdout: &str,
    task: &Task,
    value: &StepInputValue,
    step: &Step,
    schemas: &CompiledSchemas,
) -> TaskOutcome {
    debug!(stdout = %stdout, "action output");
    match json5::from_str::<serde_json::Value>(stdout) {
        Ok(output_value) => match validate_response(&output_value, step, schemas) {
            Ok(new_tasks) => {
                info!(from = %task.step, new_tasks = new_tasks.len(), "task completed");
                TaskOutcome::Success(TaskSuccess {
                    spawned: new_tasks,
                    finally_value: value.clone(),
                })
            }
            Err(e) => {
                warn!(step = %task.step, error = %e, "invalid response");
                process_retry(task, &step.options, FailureKind::InvalidResponse)
            }
        },
        Err(e) => {
            warn!(step = %task.step, error = %e, stdout = %stdout, "failed to parse response JSONC");
            process_retry(task, &step.options, FailureKind::InvalidResponse)
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
