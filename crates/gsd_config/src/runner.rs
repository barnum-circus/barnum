//! Task queue runner for GSD.
//!
//! Executes tasks through `agent_pool`, validating transitions and handling timeouts.
//!
//! Two APIs are provided:
//! - [`run()`] - Run the queue to completion
//! - [`TaskRunner`] - Iterator over task completions for fine-grained control

use crate::config::{Config, EffectiveOptions, Step};
use crate::docs::generate_step_docs;
use crate::value_schema::{CompiledSchemas, Task, validate_response};
use agent_pool::{Response, ResponseKind};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::Path;
use std::process::Command;
use tracing::{debug, error, info, warn};

/// Runner configuration.
pub struct RunnerConfig<'a> {
    /// Path to the `agent_pool` root directory.
    pub agent_pool_root: &'a Path,
    /// Optional wake script to call before starting.
    pub wake_script: Option<&'a str>,
    /// Initial tasks to process (must not be empty).
    pub initial_tasks: Vec<Task>,
}

/// The outcome of processing a task.
#[derive(Debug)]
pub struct TaskOutcome {
    /// The task that was processed.
    pub task: Task,
    /// What happened to the task.
    pub result: TaskResult,
}

/// Result of processing a single task.
#[derive(Debug)]
pub enum TaskResult {
    /// Task completed successfully, spawning new tasks.
    Completed {
        /// New tasks spawned by this task's completion.
        new_tasks: Vec<Task>,
    },
    /// Task was requeued for retry.
    Requeued {
        /// Reason for retry.
        reason: String,
        /// Current retry count.
        retry_count: u32,
    },
    /// Task was dropped (validation failed or retries exhausted).
    Dropped {
        /// Reason the task was dropped.
        reason: String,
    },
    /// Task was skipped (unknown step or validation failure).
    Skipped {
        /// Reason the task was skipped.
        reason: String,
    },
}

/// Task queue runner that yields outcomes as tasks complete.
///
/// Use this for fine-grained control over task execution:
///
/// ```ignore
/// let mut runner = TaskRunner::new(&config, &schemas, runner_config)?;
/// while let Some(outcome) = runner.next() {
///     println!("Task {} -> {:?}", outcome.task.kind, outcome.result);
/// }
/// ```
pub struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a str, &'a Step>,
    queue: VecDeque<Task>,
    agent_pool_root: &'a Path,
}

impl<'a> TaskRunner<'a> {
    /// Create a new task runner.
    ///
    /// # Errors
    ///
    /// Returns an error if the wake script fails.
    pub fn new(
        config: &'a Config,
        schemas: &'a CompiledSchemas,
        runner_config: RunnerConfig<'a>,
    ) -> io::Result<Self> {
        if let Some(script) = runner_config.wake_script {
            call_wake_script(script)?;
        }

        if let Some(max) = config.options.max_concurrency {
            debug!(
                max_concurrency = max,
                "concurrency limit configured (not yet enforced)"
            );
        }

        info!(
            tasks = runner_config.initial_tasks.len(),
            "starting task queue"
        );

        Ok(Self {
            config,
            schemas,
            step_map: config.step_map(),
            queue: runner_config.initial_tasks.into(),
            agent_pool_root: runner_config.agent_pool_root,
        })
    }

    /// Process the next task in the queue.
    ///
    /// Returns `None` when the queue is empty.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<TaskOutcome> {
        let task = self.queue.pop_front()?;
        Some(self.process_task(task))
    }

    /// Returns the number of tasks remaining in the queue.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// Returns true if the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    fn process_task(&mut self, task: Task) -> TaskOutcome {
        let Some(step) = self.step_map.get(task.kind.as_str()) else {
            error!(kind = task.kind, "unknown step, skipping task");
            return TaskOutcome {
                task,
                result: TaskResult::Skipped {
                    reason: "unknown step".to_string(),
                },
            };
        };

        if let Err(e) = self.schemas.validate(&task.kind, &task.value) {
            error!(kind = task.kind, error = %e, "task validation failed, skipping");
            return TaskOutcome {
                task,
                result: TaskResult::Skipped {
                    reason: format!("validation failed: {e}"),
                },
            };
        }

        let effective = EffectiveOptions::resolve(&self.config.options, &step.options);
        let docs = generate_step_docs(step, self.config);
        let payload = build_agent_payload(&task, &docs, effective.timeout);

        info!(kind = task.kind, "submitting task");
        debug!(payload = %payload, "task payload");

        let result = submit_with_timeout(self.agent_pool_root, &payload, effective.timeout);

        self.handle_submit_result(result, task, step, &effective)
    }

    fn handle_submit_result(
        &mut self,
        result: io::Result<Response>,
        task: Task,
        step: &Step,
        effective: &EffectiveOptions,
    ) -> TaskOutcome {
        match result {
            Ok(response) => self.handle_response(response, task, step, effective),
            Err(e) => {
                error!(kind = task.kind, error = %e, "submit failed");
                self.requeue_with_retry(task, effective, FailureKind::SubmitError)
            }
        }
    }

    fn handle_response(
        &mut self,
        response: Response,
        task: Task,
        step: &Step,
        effective: &EffectiveOptions,
    ) -> TaskOutcome {
        match response.kind {
            ResponseKind::Processed => {
                let stdout = response.stdout.unwrap_or_default();
                debug!(stdout = %stdout, "agent response");

                match serde_json::from_str::<serde_json::Value>(&stdout) {
                    Ok(value) => match validate_response(&value, step, self.schemas) {
                        Ok(new_tasks) => {
                            info!(
                                from = task.kind,
                                new_tasks = new_tasks.len(),
                                "task completed"
                            );
                            for new_task in &new_tasks {
                                self.queue.push_back(new_task.clone());
                            }
                            TaskOutcome {
                                task,
                                result: TaskResult::Completed { new_tasks },
                            }
                        }
                        Err(e) => {
                            warn!(kind = task.kind, error = %e, "invalid response");
                            self.requeue_with_retry(task, effective, FailureKind::InvalidResponse)
                        }
                    },
                    Err(e) => {
                        warn!(kind = task.kind, error = %e, "failed to parse response JSON");
                        self.requeue_with_retry(task, effective, FailureKind::InvalidResponse)
                    }
                }
            }
            ResponseKind::NotProcessed => {
                let reason = response
                    .reason
                    .map_or_else(|| "unknown".to_string(), |r| format!("{r:?}"));
                // All NotProcessed cases currently treated as timeout-like failures
                warn!(kind = task.kind, reason, "task outcome unknown");
                self.requeue_with_retry(task, effective, FailureKind::Timeout)
            }
        }
    }

    fn requeue_with_retry(
        &mut self,
        mut task: Task,
        effective: &EffectiveOptions,
        failure_kind: FailureKind,
    ) -> TaskOutcome {
        let retry_allowed = match failure_kind {
            FailureKind::Timeout => effective.retry_on_timeout,
            FailureKind::InvalidResponse => effective.retry_on_invalid_response,
            FailureKind::SubmitError => true,
        };

        if !retry_allowed {
            warn!(
                kind = task.kind,
                failure = ?failure_kind,
                "retry disabled for this failure type, dropping task"
            );
            return TaskOutcome {
                task,
                result: TaskResult::Dropped {
                    reason: format!("retry disabled for {failure_kind:?}"),
                },
            };
        }

        task.retries += 1;

        if task.retries <= effective.max_retries {
            info!(
                kind = task.kind,
                retry = task.retries,
                max = effective.max_retries,
                failure = ?failure_kind,
                "requeuing task"
            );
            let retry_count = task.retries;
            let reason = format!("{failure_kind:?}");
            self.queue.push_back(task.clone());
            TaskOutcome {
                task,
                result: TaskResult::Requeued {
                    reason,
                    retry_count,
                },
            }
        } else {
            error!(
                kind = task.kind,
                retries = task.retries,
                "max retries exceeded, dropping task"
            );
            TaskOutcome {
                task,
                result: TaskResult::Dropped {
                    reason: format!("max retries ({}) exceeded", effective.max_retries),
                },
            }
        }
    }
}

/// Why a task failed and needs retry consideration.
#[derive(Debug, Clone, Copy)]
enum FailureKind {
    Timeout,
    InvalidResponse,
    SubmitError,
}

/// Run the task queue to completion.
///
/// This is a convenience wrapper around [`TaskRunner`].
///
/// # Errors
///
/// Returns an error if the wake script fails or I/O errors occur.
pub fn run(
    config: &Config,
    schemas: &CompiledSchemas,
    runner_config: RunnerConfig<'_>,
) -> io::Result<()> {
    let mut runner = TaskRunner::new(config, schemas, runner_config)?;

    while runner.next().is_some() {}

    info!("task queue complete");
    Ok(())
}

fn call_wake_script(script: &str) -> io::Result<()> {
    info!(script, "calling wake script");
    let status = Command::new("sh").arg("-c").arg(script).status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "wake script failed with status: {status}"
        )))
    }
}

fn build_agent_payload(task: &Task, docs: &str, timeout: Option<u64>) -> String {
    let mut payload = serde_json::json!({
        "task": {
            "kind": task.kind,
            "value": task.value,
        },
        "instructions": docs,
    });

    if let Some(t) = timeout {
        payload["timeout_seconds"] = serde_json::json!(t);
    }

    serde_json::to_string(&payload).unwrap_or_default()
}

fn submit_with_timeout(root: &Path, payload: &str, timeout: Option<u64>) -> io::Result<Response> {
    if let Some(t) = timeout {
        debug!(timeout = t, "timeout configured but not yet enforced");
    }

    agent_pool::submit(root, payload)
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn build_payload_includes_task_and_docs() {
        let task = Task::new("Test", serde_json::json!({"x": 1}));
        let docs = "# Test Step";

        let payload = build_agent_payload(&task, docs, Some(60));
        let parsed: serde_json::Value = serde_json::from_str(&payload).unwrap();

        assert_eq!(parsed["task"]["kind"], "Test");
        assert_eq!(parsed["timeout_seconds"], 60);
        assert!(
            parsed["instructions"]
                .as_str()
                .unwrap()
                .contains("Test Step")
        );
    }
}
