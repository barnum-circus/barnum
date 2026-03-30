//! TypeScript handler subprocess execution.
//!
//! Spawns a subprocess to run a TypeScript handler via a worker script.
//! Protocol: stdin receives `{ "value": <input> }` as JSON, stdout returns
//! the handler result as JSON.

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

// =============================================================================
// TypeScriptHandlerError
// =============================================================================

/// Errors from TypeScript handler subprocess execution.
#[derive(Debug, thiserror::Error)]
pub enum TypeScriptHandlerError {
    /// The subprocess exited with a non-zero exit code.
    /// Stderr is inherited (printed to terminal), not captured.
    #[error("handler {module}:{func} failed (exit {exit_code})")]
    SubprocessFailed {
        /// Module path of the failed handler.
        module: String,
        /// Export name of the failed handler.
        func: String,
        /// Process exit code.
        exit_code: i32,
    },
    /// The subprocess returned invalid JSON on stdout.
    #[error("handler {module}:{func} returned invalid JSON: {source}")]
    InvalidOutput {
        /// Module path of the failed handler.
        module: String,
        /// Export name of the failed handler.
        func: String,
        /// The JSON parsing error.
        source: serde_json::Error,
    },
}

// =============================================================================
// execute_typescript
// =============================================================================

/// Execute a TypeScript handler by spawning a subprocess.
///
/// Protocol:
///   stdin  → `{ "value": <input> }` (JSON)
///   stdout ← handler result (JSON)
///
/// # Errors
///
/// Returns [`TypeScriptHandlerError::SubprocessFailed`] if the process exits
/// with a non-zero code, or [`TypeScriptHandlerError::InvalidOutput`] if
/// stdout is not valid JSON.
///
/// # Panics
///
/// Panics if the subprocess fails to spawn or stdin can't be written.
#[allow(clippy::expect_used)]
pub async fn execute_typescript(
    executor: &str,
    worker_path: &str,
    module: &str,
    func: &str,
    value: &Value,
) -> Result<Value, TypeScriptHandlerError> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("{executor} {worker_path} {module} {func}"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .expect("failed to spawn handler process");

    // Write input to stdin and close it
    let mut stdin = child.stdin.take().expect("no stdin");
    let input =
        serde_json::to_vec(&serde_json::json!({ "value": value })).expect("serialize failed");
    stdin.write_all(&input).await.expect("stdin write failed");
    drop(stdin);

    // Read stdout + wait for exit
    let output = child.wait_with_output().await.expect("wait failed");
    if !output.status.success() {
        return Err(TypeScriptHandlerError::SubprocessFailed {
            module: module.to_owned(),
            func: func.to_owned(),
            exit_code: output.status.code().unwrap_or(-1),
        });
    }

    serde_json::from_slice(&output.stdout).map_err(|source| TypeScriptHandlerError::InvalidOutput {
        module: module.to_owned(),
        func: func.to_owned(),
        source,
    })
}
