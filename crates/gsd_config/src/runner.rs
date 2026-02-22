//! Task queue runner for GSD.
//!
//! Executes tasks through `agent_pool`, validating transitions and handling timeouts.
//!
//! Two APIs are provided:
//! - [`run()`] - Run the queue to completion
//! - [`TaskRunner`] - Iterator over task completions for fine-grained control

use crate::config::{Action, Config, EffectiveOptions, Step};
use crate::docs::generate_step_docs;
use crate::value_schema::{CompiledSchemas, Task, validate_response};
use agent_pool::Response;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
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
/// Tasks are submitted concurrently, and results are yielded as they complete.
///
/// ```text
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
    max_concurrency: usize,
    in_flight: usize,
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,
}

struct InFlightResult {
    task: Task,
    step_name: String,
    result: SubmitResult,
}

enum SubmitResult {
    Pool(io::Result<Response>),
    Command(io::Result<String>),
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

        let max_concurrency = config
            .options
            .max_concurrency
            .map(|n| n as usize)
            .unwrap_or(usize::MAX);

        info!(tasks = runner_config.initial_tasks.len(), "starting task queue");

        let (tx, rx) = mpsc::channel();

        Ok(Self {
            config,
            schemas,
            step_map: config.step_map(),
            queue: runner_config.initial_tasks.into(),
            agent_pool_root: runner_config.agent_pool_root,
            max_concurrency,
            in_flight: 0,
            tx,
            rx,
        })
    }

    /// Get the next completed task outcome.
    ///
    /// This submits pending tasks concurrently and returns results as they complete.
    /// Returns `None` when queue is empty and no tasks are in flight.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<TaskOutcome> {
        self.submit_pending();

        if self.in_flight == 0 {
            return None;
        }

        let result = self.rx.recv().ok()?;
        self.in_flight -= 1;

        Some(self.process_result(result))
    }

    /// Returns the number of tasks in the queue (not including in-flight).
    #[must_use]
    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// Returns true if queue is empty and no tasks are in flight.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.in_flight == 0
    }

    fn submit_pending(&mut self) {
        while self.in_flight < self.max_concurrency {
            let Some(task) = self.queue.pop_front() else {
                break;
            };

            let Some(step) = self.step_map.get(task.kind.as_str()) else {
                error!(kind = task.kind, "unknown step, skipping task");
                continue;
            };

            if let Err(e) = self.schemas.validate(&task.kind, &task.value) {
                error!(kind = task.kind, error = %e, "task validation failed, skipping");
                continue;
            }

            let effective = EffectiveOptions::resolve(&self.config.options, &step.options);
            let step_name = step.name.clone();

            match &step.action {
                Action::Pool { .. } => {
                    let docs = generate_step_docs(step, self.config);
                    let payload = build_agent_payload(&task, &docs, effective.timeout);
                    let root = self.agent_pool_root.to_path_buf();
                    let tx = self.tx.clone();

                    info!(kind = task.kind, "submitting task to pool");
                    debug!(payload = %payload, "task payload");

                    thread::spawn(move || {
                        let result = agent_pool::submit(&root, &payload);
                        let _ = tx.send(InFlightResult {
                            task,
                            step_name,
                            result: SubmitResult::Pool(result),
                        });
                    });
                    self.in_flight += 1;
                }
                Action::Command { script } => {
                    let task_json = serde_json::to_string(&serde_json::json!({
                        "kind": task.kind,
                        "value": task.value,
                    }))
                    .unwrap_or_default();
                    let script = script.clone();
                    let tx = self.tx.clone();

                    info!(kind = task.kind, script = %script, "executing command");

                    thread::spawn(move || {
                        let result = run_command_action(&script, &task_json);
                        let _ = tx.send(InFlightResult {
                            task,
                            step_name,
                            result: SubmitResult::Command(result),
                        });
                    });
                    self.in_flight += 1;
                }
            }
        }
    }

    fn process_result(&mut self, inflight: InFlightResult) -> TaskOutcome {
        let InFlightResult { task, step_name, result } = inflight;

        let Some(step) = self.step_map.get(step_name.as_str()) else {
            return TaskOutcome {
                task,
                result: TaskResult::Skipped {
                    reason: "step no longer exists".to_string(),
                },
            };
        };

        let effective = EffectiveOptions::resolve(&self.config.options, &step.options);

        let (task_result, new_tasks) = match result {
            SubmitResult::Pool(Ok(response)) => {
                process_pool_response(response, &task, step, self.schemas, &effective)
            }
            SubmitResult::Pool(Err(e)) => {
                error!(kind = task.kind, error = %e, "submit failed");
                process_retry(&task, &effective, FailureKind::SubmitError)
            }
            SubmitResult::Command(Ok(stdout)) => {
                process_command_response(&stdout, &task, step, self.schemas, &effective)
            }
            SubmitResult::Command(Err(e)) => {
                error!(kind = task.kind, error = %e, "command failed");
                process_retry(&task, &effective, FailureKind::SubmitError)
            }
        };

        for new_task in &new_tasks {
            self.queue.push_back(new_task.clone());
        }

        TaskOutcome { task, result: task_result }
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

fn process_pool_response(
    response: Response,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
    effective: &EffectiveOptions,
) -> (TaskResult, Vec<Task>) {
    match response {
        Response::Processed { stdout, .. } => {
            debug!(stdout = %stdout, "agent response");
            match serde_json::from_str::<serde_json::Value>(&stdout) {
                Ok(value) => match validate_response(&value, step, schemas) {
                    Ok(new_tasks) => {
                        info!(from = task.kind, new_tasks = new_tasks.len(), "task completed");
                        (TaskResult::Completed { new_tasks: new_tasks.clone() }, new_tasks)
                    }
                    Err(e) => {
                        warn!(kind = task.kind, error = %e, "invalid response");
                        process_retry(task, effective, FailureKind::InvalidResponse)
                    }
                },
                Err(e) => {
                    warn!(kind = task.kind, error = %e, "failed to parse response JSON");
                    process_retry(task, effective, FailureKind::InvalidResponse)
                }
            }
        }
        Response::NotProcessed { reason } => {
            warn!(kind = task.kind, ?reason, "task outcome unknown");
            process_retry(task, effective, FailureKind::Timeout)
        }
    }
}

fn process_command_response(
    stdout: &str,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
    effective: &EffectiveOptions,
) -> (TaskResult, Vec<Task>) {
    debug!(stdout = %stdout, "command output");
    match serde_json::from_str::<serde_json::Value>(stdout) {
        Ok(value) => match validate_response(&value, step, schemas) {
            Ok(new_tasks) => {
                info!(from = task.kind, new_tasks = new_tasks.len(), "command completed");
                (TaskResult::Completed { new_tasks: new_tasks.clone() }, new_tasks)
            }
            Err(e) => {
                warn!(kind = task.kind, error = %e, "invalid command response");
                process_retry(task, effective, FailureKind::InvalidResponse)
            }
        },
        Err(e) => {
            warn!(kind = task.kind, error = %e, "failed to parse command output JSON");
            process_retry(task, effective, FailureKind::InvalidResponse)
        }
    }
}

fn process_retry(task: &Task, effective: &EffectiveOptions, failure_kind: FailureKind) -> (TaskResult, Vec<Task>) {
    let retry_allowed = match failure_kind {
        FailureKind::Timeout => effective.retry_on_timeout,
        FailureKind::InvalidResponse => effective.retry_on_invalid_response,
        FailureKind::SubmitError => true,
    };

    if !retry_allowed {
        warn!(kind = task.kind, failure = ?failure_kind, "retry disabled, dropping task");
        return (TaskResult::Dropped { reason: format!("retry disabled for {failure_kind:?}") }, vec![]);
    }

    let mut retry_task = task.clone();
    retry_task.retries += 1;

    if retry_task.retries <= effective.max_retries {
        info!(
            kind = task.kind,
            retry = retry_task.retries,
            max = effective.max_retries,
            failure = ?failure_kind,
            "requeuing task"
        );
        (TaskResult::Requeued { reason: format!("{failure_kind:?}"), retry_count: retry_task.retries }, vec![retry_task])
    } else {
        error!(kind = task.kind, retries = retry_task.retries, "max retries exceeded, dropping task");
        (TaskResult::Dropped { reason: format!("max retries ({}) exceeded", effective.max_retries) }, vec![])
    }
}

fn call_wake_script(script: &str) -> io::Result<()> {
    info!(script, "calling wake script");
    let status = Command::new("sh").arg("-c").arg(script).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("wake script failed with status: {status}")))
    }
}

fn build_agent_payload(task: &Task, docs: &str, timeout: Option<u64>) -> String {
    let mut payload = serde_json::json!({
        "task": { "kind": task.kind, "value": task.value },
        "instructions": docs,
    });
    if let Some(t) = timeout {
        payload["timeout_seconds"] = serde_json::json!(t);
    }
    serde_json::to_string(&payload).unwrap_or_default()
}

fn run_command_action(script: &str, task_json: &str) -> io::Result<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(task_json.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::other(format!("command failed with status {}: {}", output.status, stderr.trim())))
    }
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
        assert!(parsed["instructions"].as_str().unwrap().contains("Test Step"));
    }
}
