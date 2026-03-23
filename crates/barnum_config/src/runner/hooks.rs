//! Hook execution (command actions, wake scripts).

use std::io;
use std::path::Path;
use std::process::Command;

use tracing::info;

use super::shell::run_shell_command;

/// Call a wake script before starting the runner.
pub fn call_wake_script(script: &str) -> io::Result<()> {
    info!(script, "calling wake script");
    let status = Command::new("sh").arg("-c").arg(script).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "[E019] wake script failed with status: {status}"
        )))
    }
}

/// Run a command action (shell script) with task JSON on stdin.
pub fn run_command_action(script: &str, task_json: &str, working_dir: &Path) -> io::Result<String> {
    run_shell_command(script, task_json, Some(working_dir))
        .map_err(|e| io::Error::other(format!("[E021] command {e}")))
}
