//! Action trait and dispatch infrastructure.

use std::fmt;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cli_invoker::Invoker;
use tracing::debug;
use troupe::Response;
use troupe_cli::TroupeCli;

use crate::types::{LogTaskId, StepName};
use crate::value_schema::Task;

use super::dispatch::{ActionResult, WorkerKind, WorkerResult};
use super::submit::{build_agent_payload, submit_via_cli};

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

// ==================== PoolAction ====================

/// Pool action: submits a task to the troupe agent pool.
pub struct PoolAction {
    pub root: PathBuf,
    pub invoker: Invoker<TroupeCli>,
    pub docs: String,
    pub step_name: StepName,
    /// Troupe's agent lifecycle timeout (seconds), passed through in the payload.
    pub pool_timeout: Option<u64>,
}

impl Action for PoolAction {
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let payload =
                build_agent_payload(&self.step_name, &value, &self.docs, self.pool_timeout);
            debug!(payload = %payload, "task payload");
            let result = match submit_via_cli(&self.root, &payload, &self.invoker) {
                Ok(Response::Processed { stdout, .. }) => Ok(stdout),
                Ok(Response::NotProcessed { .. }) => Err("not processed by pool".into()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(result);
        });
        // No-op guard: troupe manages its own agent lifecycle.
        ActionHandle::new(rx, ())
    }
}
