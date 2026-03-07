//! Finally hook tracking and execution.

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use tracing::{info, warn};

use crate::types::{HookScript, LogTaskId};
use crate::value_schema::Task;

pub struct FinallyState {
    pub pending_count: usize,
    pub original_value: serde_json::Value,
    pub finally_command: HookScript,
}

pub struct FinallyTracker {
    tracking: HashMap<LogTaskId, FinallyState>,
}

impl FinallyTracker {
    pub fn new() -> Self {
        Self {
            tracking: HashMap::new(),
        }
    }

    pub fn start_tracking(
        &mut self,
        task_id: LogTaskId,
        pending_count: usize,
        original_value: serde_json::Value,
        finally_command: HookScript,
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

    /// Record that a descendant of `origin_id` has completed.
    ///
    /// Returns `Some(FinallyState)` when all descendants are done and the
    /// finally hook is ready to run. Returns `None` if descendants remain
    /// or if `origin_id` has no finally tracking (no-op for tasks without finally hooks).
    pub fn record_descendant_done(&mut self, origin_id: LogTaskId) -> Option<FinallyState> {
        let ready_for_finally = if let Some(state) = self.tracking.get_mut(&origin_id) {
            state.pending_count = state.pending_count.saturating_sub(1);
            state.pending_count == 0
        } else {
            // Not tracked - origin has no finally hook, this is expected
            return None;
        };

        if ready_for_finally {
            self.tracking.remove(&origin_id)
        } else {
            None
        }
    }
}

#[expect(clippy::needless_pass_by_value)]
pub fn run_finally_hook(state: FinallyState) -> Vec<Task> {
    info!(command = %state.finally_command, "running finally hook");

    let input_json = serde_json::to_string(&state.original_value).unwrap_or_default();

    let result = Command::new("sh")
        .arg("-c")
        .arg(state.finally_command.as_str())
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
