//! Finally hook tracking and execution.
//!
//! Tracks pending descendants for tasks with finally hooks. When all descendants
//! complete, the finally hook runs.

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use tracing::{info, warn};

use crate::types::LogTaskId;
use crate::value_schema::Task;

/// State for tracking when a `finally` hook should run.
pub struct FinallyState {
    /// Number of descendants still pending (in queue or in flight).
    pub pending_count: usize,
    /// The original task's value (input to finally hook).
    pub original_value: serde_json::Value,
    /// The finally hook command.
    pub finally_command: String,
}

/// Tracks finally hooks for tasks with pending descendants.
pub struct FinallyTracker {
    /// Key: origin task ID, Value: finally state
    tracking: HashMap<LogTaskId, FinallyState>,
}

impl FinallyTracker {
    pub fn new() -> Self {
        Self {
            tracking: HashMap::new(),
        }
    }

    /// Start tracking a task's finally hook.
    pub fn start_tracking(
        &mut self,
        task_id: LogTaskId,
        pending_count: usize,
        original_value: serde_json::Value,
        finally_command: String,
    ) {
        self.tracking.insert(
            task_id,
            FinallyState {
                pending_count,
                original_value,
                finally_command,
            },
        );
    }

    /// Decrement the pending count for an origin task.
    /// Returns the `FinallyState` if the count reaches zero and the hook should run.
    pub fn decrement(&mut self, origin_id: LogTaskId) -> Option<FinallyState> {
        let should_run = if let Some(state) = self.tracking.get_mut(&origin_id) {
            state.pending_count = state.pending_count.saturating_sub(1);
            state.pending_count == 0
        } else {
            false
        };

        if should_run {
            self.tracking.remove(&origin_id)
        } else {
            None
        }
    }
}

/// Run a finally hook and return any spawned tasks.
#[allow(clippy::needless_pass_by_value)] // We own state from HashMap removal
pub fn run_finally_hook(state: FinallyState) -> Vec<Task> {
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
                    tasks
                }
                Err(e) => {
                    warn!(error = %e, "finally hook output is not valid JSON (ignored)");
                    vec![]
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
            vec![]
        }
        Err(e) => {
            warn!(error = %e, "finally hook failed to run (ignored)");
            vec![]
        }
    }
}
