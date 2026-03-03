//! Task queue runner for GSD.
//!
//! Executes tasks through `agent_pool`, validating transitions and handling timeouts.
//!
//! Two APIs are provided:
//! - [`run()`] - Run the queue to completion
//! - [`TaskRunner`] - Iterator over task completions for fine-grained control

use crate::config::{Action, Config, EffectiveOptions, Step};
use crate::docs::generate_step_docs;
use crate::types::StepName;
use crate::value_schema::{CompiledSchemas, Task, validate_response};
use agent_pool::Response;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use tracing::{debug, error, info, warn};

/// Input/output for post hooks.
///
/// Post hooks receive this JSON on stdin and must output (possibly modified)
/// JSON on stdout. The `next` array can be filtered, added to, or transformed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PostHookInput {
    /// The action completed successfully.
    Success {
        /// The input value (possibly modified by pre hook).
        input: serde_json::Value,
        /// The agent's output.
        output: serde_json::Value,
        /// Tasks spawned by this completion. Post hook can modify this.
        next: Vec<Task>,
    },
    /// The action timed out.
    Timeout {
        /// The input value (possibly modified by pre hook).
        input: serde_json::Value,
    },
    /// The action failed with an error.
    Error {
        /// The input value (possibly modified by pre hook).
        input: serde_json::Value,
        /// Error message.
        error: String,
    },
    /// The pre hook failed.
    PreHookError {
        /// The original input value (before pre hook).
        input: serde_json::Value,
        /// Error message from pre hook.
        error: String,
    },
}

/// Runner configuration.
pub struct RunnerConfig<'a> {
    /// Path to the `agent_pool` root directory.
    pub agent_pool_root: &'a Path,
    /// Base path for resolving linked instructions (typically the config file's directory).
    pub config_base_path: &'a Path,
    /// Optional wake script to call before starting.
    pub wake_script: Option<&'a str>,
    /// Initial tasks to process (must not be empty).
    pub initial_tasks: Vec<Task>,
    /// Optional path to the `agent_pool` binary. If not specified, uses `AGENT_POOL` env var or PATH.
    pub agent_pool_binary: Option<&'a Path>,
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
///     println!("Task {} -> {:?}", outcome.task.step, outcome.result);
/// }
/// ```
pub struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a str, &'a Step>,
    queue: VecDeque<QueuedTask>,
    agent_pool_root: &'a Path,
    config_base_path: &'a Path,
    agent_pool_binary: Option<&'a Path>,
    max_concurrency: usize,
    in_flight: usize,
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,
    /// Counter for assigning unique task IDs.
    next_task_id: u64,
    /// Tracks pending descendants for tasks with `finally` hooks.
    /// Key: origin task ID, Value: (pending count, original value, finally command)
    finally_tracking: HashMap<u64, FinallyState>,
}

/// Internal task wrapper with lineage tracking.
struct QueuedTask {
    task: Task,
    /// Unique ID for this task instance.
    id: u64,
    /// If this task descended from a task with `finally`, tracks that origin.
    origin_id: Option<u64>,
}

/// State for tracking when a `finally` hook should run.
struct FinallyState {
    /// Number of descendants still pending (in queue or in flight).
    pending_count: usize,
    /// The original task's value (input to finally hook).
    original_value: serde_json::Value,
    /// The finally hook command.
    finally_command: String,
}

struct InFlightResult {
    task: Task,
    task_id: u64,
    origin_id: Option<u64>,
    step_name: StepName,
    /// The value passed to the action (possibly modified by pre hook).
    effective_value: serde_json::Value,
    result: SubmitResult,
    /// Post hook command to run after processing (if any).
    post_hook: Option<String>,
    /// Finally hook for this step (if any) - used when spawning children.
    finally_hook: Option<String>,
}

enum SubmitResult {
    Pool(io::Result<Response>),
    Command(io::Result<String>),
    /// Pre hook failed before the action could run.
    PreHookError(String),
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

        // Pool existence/readiness is checked by submit_via_cli on first task submission

        let max_concurrency = config.options.max_concurrency.unwrap_or(usize::MAX);

        info!(
            tasks = runner_config.initial_tasks.len(),
            "starting task queue"
        );

        let (tx, rx) = mpsc::channel();

        // Wrap initial tasks with IDs (no origin since they're root tasks)
        let mut next_task_id = 0u64;
        let queue: VecDeque<QueuedTask> = runner_config
            .initial_tasks
            .into_iter()
            .map(|task| {
                let id = next_task_id;
                next_task_id += 1;
                QueuedTask {
                    task,
                    id,
                    origin_id: None,
                }
            })
            .collect();

        Ok(Self {
            config,
            schemas,
            step_map: config.step_map(),
            queue,
            agent_pool_root: runner_config.agent_pool_root,
            config_base_path: runner_config.config_base_path,
            agent_pool_binary: runner_config.agent_pool_binary,
            max_concurrency,
            in_flight: 0,
            tx,
            rx,
            next_task_id,
            finally_tracking: HashMap::new(),
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

    #[allow(clippy::too_many_lines)]
    fn submit_pending(&mut self) {
        while self.in_flight < self.max_concurrency {
            let Some(queued) = self.queue.pop_front() else {
                break;
            };

            let QueuedTask {
                task,
                id: task_id,
                origin_id,
            } = queued;

            let Some(step) = self.step_map.get(task.step.as_str()) else {
                error!(step = %task.step, "unknown step, skipping task");
                // Decrement origin tracking if this task was being tracked
                self.decrement_origin(origin_id);
                continue;
            };

            if let Err(e) = self.schemas.validate(&task.step, &task.value) {
                error!(step = %task.step, error = %e, "task validation failed, skipping");
                self.decrement_origin(origin_id);
                continue;
            }

            let effective = EffectiveOptions::resolve(&self.config.options, &step.options);
            let step_name = step.name.clone();
            let pre_hook = step.pre.clone();
            let post_hook = step.post.clone();
            let finally_hook = step.finally_hook.clone();

            match &step.action {
                Action::Pool { .. } => {
                    let docs = generate_step_docs(step, self.config, self.config_base_path);
                    let timeout = effective.timeout;
                    let root = self.agent_pool_root.to_path_buf();
                    let binary = self.agent_pool_binary.map(Path::to_path_buf);
                    let tx = self.tx.clone();
                    let original_value = task.value.clone();

                    info!(step = %task.step, "submitting task to pool");

                    thread::spawn(move || {
                        // Run pre hook if present
                        let effective_value = match run_pre_hook(pre_hook.as_ref(), &original_value)
                        {
                            Ok(v) => v,
                            Err(e) => {
                                let _ = tx.send(InFlightResult {
                                    task,
                                    task_id,
                                    origin_id,
                                    step_name,
                                    effective_value: original_value,
                                    result: SubmitResult::PreHookError(e),
                                    post_hook,
                                    finally_hook,
                                });
                                return;
                            }
                        };

                        // Build payload with (possibly modified) value
                        let payload = build_agent_payload_with_value(
                            &step_name,
                            &effective_value,
                            &docs,
                            timeout,
                        );
                        debug!(payload = %payload, "task payload");

                        let result = submit_via_cli(&root, &payload, binary.as_deref());
                        let _ = tx.send(InFlightResult {
                            task,
                            task_id,
                            origin_id,
                            step_name,
                            effective_value,
                            result: SubmitResult::Pool(result),
                            post_hook,
                            finally_hook,
                        });
                    });
                    self.in_flight += 1;
                }
                Action::Command { script } => {
                    let script = script.clone();
                    let tx = self.tx.clone();
                    let original_value = task.value.clone();
                    let task_step = task.step.clone();
                    let working_dir = self.config_base_path.to_path_buf();

                    info!(step = %task.step, script = %script, "executing command");

                    thread::spawn(move || {
                        // Run pre hook if present
                        let effective_value = match run_pre_hook(pre_hook.as_ref(), &original_value)
                        {
                            Ok(v) => v,
                            Err(e) => {
                                let _ = tx.send(InFlightResult {
                                    task,
                                    task_id,
                                    origin_id,
                                    step_name,
                                    effective_value: original_value,
                                    result: SubmitResult::PreHookError(e),
                                    post_hook,
                                    finally_hook,
                                });
                                return;
                            }
                        };

                        let task_json = serde_json::to_string(&serde_json::json!({
                            "kind": task_step,
                            "value": effective_value,
                        }))
                        .unwrap_or_default();

                        let result = run_command_action(&script, &task_json, &working_dir);
                        let _ = tx.send(InFlightResult {
                            task,
                            task_id,
                            origin_id,
                            step_name,
                            effective_value,
                            result: SubmitResult::Command(result),
                            post_hook,
                            finally_hook,
                        });
                    });
                    self.in_flight += 1;
                }
            }
        }
    }

    /// Decrement the pending count for an origin and run finally if done.
    fn decrement_origin(&mut self, origin_id: Option<u64>) {
        let Some(oid) = origin_id else { return };

        let should_run_finally = if let Some(state) = self.finally_tracking.get_mut(&oid) {
            state.pending_count = state.pending_count.saturating_sub(1);
            state.pending_count == 0
        } else {
            false
        };

        if should_run_finally && let Some(state) = self.finally_tracking.remove(&oid) {
            self.run_finally_hook(state);
        }
    }

    /// Run a finally hook and queue any resulting tasks.
    #[allow(clippy::needless_pass_by_value)] // We own state from HashMap removal
    fn run_finally_hook(&mut self, state: FinallyState) {
        info!(command = %state.finally_command, "running finally hook");

        let input_json = serde_json::to_string(&state.original_value).unwrap_or_default();

        let result = Command::new("sh")
            .arg("-c")
            .arg(&state.finally_command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(input_json.as_bytes());
                }
                child.wait_with_output()
            });

        match result {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                match serde_json::from_str::<Vec<Task>>(&stdout) {
                    Ok(tasks) => {
                        info!(count = tasks.len(), "finally hook spawned tasks");
                        for task in tasks {
                            let id = self.next_task_id;
                            self.next_task_id += 1;
                            self.queue.push_back(QueuedTask {
                                task,
                                id,
                                origin_id: None, // Finally tasks don't inherit origin
                            });
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "finally hook output is not valid JSON (ignored)");
                    }
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!(
                    status = %output.status,
                    stderr = %stderr.trim(),
                    "finally hook failed (ignored)"
                );
            }
            Err(e) => {
                warn!(error = %e, "finally hook failed to run (ignored)");
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn process_result(&mut self, inflight: InFlightResult) -> TaskOutcome {
        let InFlightResult {
            task,
            task_id,
            origin_id,
            step_name,
            effective_value,
            result,
            post_hook,
            finally_hook,
        } = inflight;

        let Some(step) = self.step_map.get(step_name.as_str()) else {
            return TaskOutcome {
                task,
                result: TaskResult::Skipped {
                    reason: "step no longer exists".to_string(),
                },
            };
        };

        let effective = EffectiveOptions::resolve(&self.config.options, &step.options);

        let (task_result, new_tasks, post_input) = match result {
            SubmitResult::Pool(Ok(response)) => process_pool_response(
                response,
                &task,
                &effective_value,
                step,
                self.schemas,
                &effective,
            ),
            SubmitResult::Pool(Err(e)) => {
                error!(step = %task.step, error = %e, "submit failed");
                let (result, tasks) = process_retry(&task, &effective, FailureKind::SubmitError);
                let post_input = PostHookInput::Error {
                    input: effective_value.clone(),
                    error: e.to_string(),
                };
                (result, tasks, post_input)
            }
            SubmitResult::Command(Ok(stdout)) => process_command_response(
                &stdout,
                &task,
                &effective_value,
                step,
                self.schemas,
                &effective,
            ),
            SubmitResult::Command(Err(e)) => {
                error!(step = %task.step, error = %e, "command failed");
                let (result, tasks) = process_retry(&task, &effective, FailureKind::SubmitError);
                let post_input = PostHookInput::Error {
                    input: effective_value.clone(),
                    error: e.to_string(),
                };
                (result, tasks, post_input)
            }
            SubmitResult::PreHookError(e) => {
                error!(step = %task.step, error = %e, "pre hook failed");
                let (result, tasks) = process_retry(&task, &effective, FailureKind::SubmitError);
                let post_input = PostHookInput::PreHookError {
                    input: task.value.clone(),
                    error: e,
                };
                (result, tasks, post_input)
            }
        };

        // Run post hook synchronously - it can modify the next tasks
        let (final_result, final_tasks) = if let Some(hook) = post_hook {
            match run_post_hook(&hook, &post_input) {
                Ok(modified) => {
                    // Extract next tasks from post hook output
                    let tasks = extract_next_tasks(&modified);
                    (task_result, tasks)
                }
                Err(e) => {
                    // Post hook failed - trigger retry
                    warn!(step = %task.step, error = %e, "post hook failed");
                    process_retry(&task, &effective, FailureKind::SubmitError)
                }
            }
        } else {
            (task_result, new_tasks)
        };

        // Determine origin_id for new tasks:
        // - If this task has a finally hook and spawns children, they get this task's ID
        // - Otherwise, children inherit the parent's origin_id
        let child_origin_id = if finally_hook.is_some() && !final_tasks.is_empty() {
            // Set up finally tracking for this task
            self.finally_tracking.insert(
                task_id,
                FinallyState {
                    pending_count: final_tasks.len(),
                    original_value: effective_value,
                    finally_command: finally_hook.unwrap_or_default(),
                },
            );
            Some(task_id)
        } else {
            origin_id
        };

        // Queue new tasks with proper origin tracking
        for new_task in final_tasks {
            let id = self.next_task_id;
            self.next_task_id += 1;
            self.queue.push_back(QueuedTask {
                task: new_task,
                id,
                origin_id: child_origin_id,
            });
        }

        // This task is done - decrement its origin's pending count
        self.decrement_origin(origin_id);

        TaskOutcome {
            task,
            result: final_result,
        }
    }
}

/// Extract next tasks from a post hook result.
fn extract_next_tasks(input: &PostHookInput) -> Vec<Task> {
    match input {
        PostHookInput::Success { next, .. } => next.clone(),
        PostHookInput::Timeout { .. }
        | PostHookInput::Error { .. }
        | PostHookInput::PreHookError { .. } => vec![],
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
    let mut dropped_count = 0u32;
    while let Some(outcome) = runner.next() {
        if matches!(outcome.result, TaskResult::Dropped { .. }) {
            dropped_count += 1;
        }
    }
    if dropped_count > 0 {
        error!(dropped_count, "task queue completed with dropped tasks");
        return Err(io::Error::other(format!(
            "{dropped_count} task(s) were dropped (retries exhausted)"
        )));
    }
    info!("task queue complete");
    Ok(())
}

fn process_pool_response(
    response: Response,
    task: &Task,
    effective_value: &serde_json::Value,
    step: &Step,
    schemas: &CompiledSchemas,
    effective: &EffectiveOptions,
) -> (TaskResult, Vec<Task>, PostHookInput) {
    match response {
        Response::Processed { stdout, .. } => {
            debug!(stdout = %stdout, "agent response");
            process_stdout(&stdout, task, effective_value, step, schemas, effective)
        }
        Response::NotProcessed { reason } => {
            warn!(step = %task.step, ?reason, "task outcome unknown");
            let (result, tasks) = process_retry(task, effective, FailureKind::Timeout);
            let post_input = PostHookInput::Timeout {
                input: effective_value.clone(),
            };
            (result, tasks, post_input)
        }
    }
}

fn process_command_response(
    stdout: &str,
    task: &Task,
    effective_value: &serde_json::Value,
    step: &Step,
    schemas: &CompiledSchemas,
    effective: &EffectiveOptions,
) -> (TaskResult, Vec<Task>, PostHookInput) {
    debug!(stdout = %stdout, "command output");
    process_stdout(stdout, task, effective_value, step, schemas, effective)
}

/// Process stdout from either pool or command action.
fn process_stdout(
    stdout: &str,
    task: &Task,
    effective_value: &serde_json::Value,
    step: &Step,
    schemas: &CompiledSchemas,
    effective: &EffectiveOptions,
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
                (
                    TaskResult::Completed {
                        new_tasks: new_tasks.clone(),
                    },
                    new_tasks,
                    post_input,
                )
            }
            Err(e) => {
                warn!(step = %task.step, error = %e, "invalid response");
                let (result, tasks) = process_retry(task, effective, FailureKind::InvalidResponse);
                let post_input = PostHookInput::Error {
                    input: effective_value.clone(),
                    error: e.to_string(),
                };
                (result, tasks, post_input)
            }
        },
        Err(e) => {
            warn!(step = %task.step, error = %e, "failed to parse response JSON");
            let (result, tasks) = process_retry(task, effective, FailureKind::InvalidResponse);
            let post_input = PostHookInput::Error {
                input: effective_value.clone(),
                error: format!("failed to parse response JSON: {e}"),
            };
            (result, tasks, post_input)
        }
    }
}

fn process_retry(
    task: &Task,
    effective: &EffectiveOptions,
    failure_kind: FailureKind,
) -> (TaskResult, Vec<Task>) {
    let retry_allowed = match failure_kind {
        FailureKind::Timeout => effective.retry_on_timeout,
        FailureKind::InvalidResponse => effective.retry_on_invalid_response,
        FailureKind::SubmitError => true,
    };

    if !retry_allowed {
        warn!(step = %task.step, failure = ?failure_kind, "retry disabled, dropping task");
        return (
            TaskResult::Dropped {
                reason: format!("retry disabled for {failure_kind:?}"),
            },
            vec![],
        );
    }

    let mut retry_task = task.clone();
    retry_task.retries += 1;

    if retry_task.retries <= effective.max_retries {
        info!(
            step = %task.step,
            retry = retry_task.retries,
            max = effective.max_retries,
            failure = ?failure_kind,
            "requeuing task"
        );
        (
            TaskResult::Requeued {
                reason: format!("{failure_kind:?}"),
                retry_count: retry_task.retries,
            },
            vec![retry_task],
        )
    } else {
        error!(step = %task.step, retries = retry_task.retries, "max retries exceeded, dropping task");
        (
            TaskResult::Dropped {
                reason: format!("max retries ({}) exceeded", effective.max_retries),
            },
            vec![],
        )
    }
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

fn run_command_action(script: &str, task_json: &str, working_dir: &Path) -> io::Result<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        // Ignore BrokenPipe - command may exit without reading stdin (e.g., `echo '[]'`)
        let _ = stdin.write_all(task_json.as_bytes());
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::other(format!(
            "command failed with status {}: {}",
            output.status,
            stderr.trim()
        )))
    }
}

/// Run a pre hook if present, returning the (possibly modified) value.
fn run_pre_hook(
    hook: Option<&String>,
    value: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let Some(script) = hook else {
        return Ok(value.clone());
    };

    info!(script = %script, "running pre hook");

    let input = serde_json::to_string(value).unwrap_or_default();

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Err(format!("failed to spawn pre hook: {e}")),
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return Err(format!("pre hook failed: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "pre hook exited with status {}: {}",
            output.status,
            stderr.trim()
        ));
    }

    let stdout = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(e) => return Err(format!("pre hook output is not valid UTF-8: {e}")),
    };

    match serde_json::from_str(&stdout) {
        Ok(v) => {
            debug!("pre hook transformed value");
            Ok(v)
        }
        Err(e) => Err(format!("pre hook output is not valid JSON: {e}")),
    }
}

/// Run a post hook synchronously and return the (possibly modified) result.
///
/// Post hooks can modify the `next` array to filter, add, or transform tasks.
fn run_post_hook(script: &str, input: &PostHookInput) -> Result<PostHookInput, String> {
    info!(script = %script, kind = ?std::mem::discriminant(input), "running post hook");

    let input_json = serde_json::to_string(&input).unwrap_or_default();

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Err(format!("failed to spawn post hook: {e}")),
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input_json.as_bytes());
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return Err(format!("post hook failed: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "post hook exited with status {}: {}",
            output.status,
            stderr.trim()
        ));
    }

    let stdout = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(e) => return Err(format!("post hook output is not valid UTF-8: {e}")),
    };

    match serde_json::from_str(&stdout) {
        Ok(modified) => {
            debug!(script = %script, "post hook completed");
            Ok(modified)
        }
        Err(e) => Err(format!("post hook output is not valid JSON: {e}")),
    }
}

/// Build agent payload with a specific value (used when pre hook modifies the value).
fn build_agent_payload_with_value(
    step_name: &StepName,
    value: &serde_json::Value,
    docs: &str,
    timeout: Option<u64>,
) -> String {
    let mut payload = serde_json::json!({
        "task": { "kind": step_name, "value": value },
        "instructions": docs,
    });
    if let Some(t) = timeout {
        payload["timeout_seconds"] = serde_json::json!(t);
    }
    serde_json::to_string(&payload).unwrap_or_default()
}

/// Submit a task via the CLI instead of internal API.
fn submit_via_cli(
    pool_path: &Path,
    payload: &str,
    agent_pool_binary: Option<&Path>,
) -> io::Result<Response> {
    let binary = agent_pool_binary.map_or_else(resolve_agent_pool_binary, Path::to_path_buf);

    // Extract pool_root (parent) and pool_id (basename) from full path
    let pool_root = pool_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Invalid pool path (no parent): {}", pool_path.display()),
        )
    })?;
    let pool_id = pool_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid pool path (no basename): {}", pool_path.display()),
            )
        })?;

    // Use 24-hour timeout. TODO: Add --no-timeout support to CLI.
    let output = Command::new(&binary)
        .arg("submit_task")
        .arg("--pool-root")
        .arg(pool_root)
        .arg("--pool")
        .arg(pool_id)
        .arg("--notify")
        .arg("file")
        .arg("--timeout-secs")
        .arg("86400")
        .arg("--data")
        .arg(payload)
        .output()
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Failed to run agent_pool binary '{}': {e}",
                    binary.display()
                ),
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "agent_pool submit_task failed: {}",
            stderr.trim()
        )));
    }

    serde_json::from_slice(&output.stdout).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse agent_pool output: {e}"),
        )
    })
}

/// Resolve the `agent_pool` binary path.
fn resolve_agent_pool_binary() -> std::path::PathBuf {
    // 1. Environment variable override (AGENT_POOL is the standard name)
    if let Ok(path) = std::env::var("AGENT_POOL") {
        return std::path::PathBuf::from(path);
    }

    // 2. Default: assume it's in PATH
    std::path::PathBuf::from("agent_pool")
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn build_payload_includes_task_and_docs() {
        let step_name = StepName::new("Test");
        let value = serde_json::json!({"x": 1});
        let docs = "# Test Step";

        let payload = build_agent_payload_with_value(&step_name, &value, docs, Some(60));
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
