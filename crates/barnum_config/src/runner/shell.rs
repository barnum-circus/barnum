//! Shell command execution utilities.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Run a shell command with stdin input and capture output.
///
/// Returns the stdout on success, or an error message on failure.
pub fn run_shell_command(
    script: &str,
    stdin_input: &str,
    working_dir: Option<&Path>,
) -> Result<String, String> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        // Ignore BrokenPipe - command may exit without reading stdin
        let _ = stdin.write_all(stdin_input.as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "exited with status {}: {}",
            output.status,
            stderr.trim()
        ));
    }

    String::from_utf8(output.stdout).map_err(|e| format!("output is not valid UTF-8: {e}"))
}
