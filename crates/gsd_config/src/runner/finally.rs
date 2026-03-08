//! Finally hook execution.

use tracing::{info, warn};

use crate::types::HookScript;
use crate::value_schema::Task;

use super::shell::run_shell_command;

/// Run a finally hook with the given value.
///
/// Returns tasks spawned by the hook (may be empty).
#[expect(clippy::expect_used, reason = "serde_json::Value always serializes")]
pub fn run_finally_hook_direct(
    finally_command: &HookScript,
    value: &serde_json::Value,
) -> Vec<Task> {
    info!(command = %finally_command, "running finally hook");

    let input_json =
        serde_json::to_string(value).expect("[P018] serde_json::Value should always serialize");

    match run_shell_command(finally_command.as_str(), &input_json, None) {
        Ok(stdout) => match serde_json::from_str::<Vec<Task>>(&stdout) {
            Ok(tasks) => {
                info!(count = tasks.len(), "finally hook spawned tasks");
                tasks
            }
            Err(e) => {
                warn!(error = %e, "finally hook output is not valid JSON (ignored)");
                vec![]
            }
        },
        Err(e) => {
            warn!(error = %e, "finally hook failed (ignored)");
            vec![]
        }
    }
}
