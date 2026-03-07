//! Shell hook execution (pre, post, command actions).

use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{debug, info};

use super::PostHookInput;

/// Run a pre hook if present, returning the (possibly modified) value.
pub fn run_pre_hook(
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
pub fn run_post_hook(script: &str, input: &PostHookInput) -> Result<PostHookInput, String> {
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
        String::from_utf8(output.stdout).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("[E020] invalid UTF-8 in command output: {e}"),
            )
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::other(format!(
            "[E021] command failed with status {}: {}",
            output.status,
            stderr.trim()
        )))
    }
}
