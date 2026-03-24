//! Action trait and dispatch infrastructure.

use std::fmt;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::types::{LogTaskId, StepInputValue, StepName, Task};

// ==================== Worker types ====================

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

// ==================== ActionError ====================

/// Error from action dispatch. Only `run_action` produces `TimedOut`.
pub enum ActionError {
    /// Timeout fired (produced by `run_action`, not by actions themselves).
    TimedOut,
    /// The action returned an error.
    Failed(String),
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut => write!(f, "action timed out"),
            Self::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

// ==================== Action trait ====================

/// Handle returned by `Action::start`. Dropping this handle cancels the action.
///
/// - Call `rx.recv()` or `rx.recv_timeout()` to get the result.
/// - Drop the handle to cancel the action (best-effort via guard's `Drop`).
/// - Late sends to a dropped handle are silently discarded.
pub struct ActionHandle {
    pub rx: mpsc::Receiver<Result<String, String>>,
    /// Held for its `Drop` impl — dropping this handle cancels the action.
    #[expect(dead_code, reason = "held for Drop semantics, not read")]
    drop_guard: Box<dyn Send>,
}

impl ActionHandle {
    /// Create a handle with a type-erased cleanup guard.
    pub fn new(rx: mpsc::Receiver<Result<String, String>>, guard: impl Send + 'static) -> Self {
        Self {
            rx,
            drop_guard: Box::new(guard),
        }
    }
}

/// An executable action. Constructed per dispatch, consumed once by `start`.
///
/// `start` kicks off work (typically by spawning a thread) and returns an
/// `ActionHandle` immediately. It does not block.
pub trait Action: Send {
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle;
}

// ==================== run_action ====================

/// Run an action with an optional timeout.
///
/// Computes a deadline before calling `start`, so time spent in `start`
/// counts against the timeout. On timeout, the handle drops — the guard's
/// `Drop` kills the underlying work.
pub fn run_action(
    action: Box<dyn Action>,
    value: &serde_json::Value,
    timeout: Option<Duration>,
) -> Result<String, ActionError> {
    let deadline = timeout.map(|d| Instant::now() + d);
    let handle = action.start(value.clone());
    let channel_result = match deadline {
        None => handle
            .rx
            .recv()
            .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        Some(deadline) => {
            let remaining = deadline.saturating_duration_since(Instant::now());
            handle.rx.recv_timeout(remaining)
        }
    };
    match channel_result {
        Ok(result) => result.map_err(ActionError::Failed),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(ActionError::TimedOut),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(ActionError::Failed("action panicked".into()))
        }
    }
    // handle drops here — guard's Drop kills the action if still running
}

// ==================== spawn_worker ====================

/// Spawn a worker thread that runs an action and sends the result to the engine.
pub fn spawn_worker(
    tx: mpsc::Sender<WorkerResult>,
    action: Box<dyn Action>,
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
    timeout: Option<Duration>,
) {
    thread::spawn(move || {
        let value = task.value.clone();
        let output = run_action(action, &value.0, timeout);
        let _ = tx.send(WorkerResult {
            task_id,
            task,
            kind,
            result: ActionResult { value, output },
        });
    });
}

// ==================== ShellAction ====================

/// Guard that kills a child process on drop via `Child::kill()`.
struct ProcessGuard {
    child: Arc<Mutex<Child>>,
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

/// JSON envelope piped to the shell script's stdin.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    value: &'a serde_json::Value,
    config: &'a serde_json::Value,
    step_name: &'a StepName,
}

/// Shell action: runs a shell script with the task value on stdin.
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub config: Arc<serde_json::Value>,
    pub working_dir: PathBuf,
}

impl Action for ShellAction {
    #[expect(clippy::expect_used)]
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle {
        let (tx, rx) = mpsc::channel();
        let task_json = serde_json::to_string(&Envelope {
            value: &value,
            config: &self.config,
            step_name: &self.step_name,
        })
        .unwrap_or_default();

        let child = Command::new("sh")
            .arg("-c")
            .arg(&self.script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(&self.working_dir)
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
                return ActionHandle::new(rx, ());
            }
        };

        // Write stdin, then drop to close the pipe.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(task_json.as_bytes());
        }

        // Take pipes before sharing the child.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));

        // Reader thread: reads pipes to completion, then waits for exit.
        let child_for_reader = Arc::clone(&child);
        thread::spawn(move || {
            let stdout_data = stdout
                .map(|mut r| {
                    let mut s = String::new();
                    r.read_to_string(&mut s).ok();
                    s
                })
                .unwrap_or_default();
            let stderr_data = stderr
                .map(|mut r| {
                    let mut s = String::new();
                    r.read_to_string(&mut s).ok();
                    s
                })
                .unwrap_or_default();

            let status = child_for_reader
                .lock()
                .expect("[P080] child mutex poisoned")
                .wait();
            let result = match status {
                Ok(s) if s.success() => Ok(stdout_data),
                Ok(_) => Err(stderr_data),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(result);
        });

        ActionHandle::new(rx, ProcessGuard { child })
    }
}
