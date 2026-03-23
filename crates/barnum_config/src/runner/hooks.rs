//! Hook execution (pre, post, command actions).

use std::io;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::types::{HookScript, StepInputValue};
use crate::value_schema::Task;

use super::shell::run_shell_command;

/// The action completed successfully.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostHookSuccess {
    /// The task's input value.
    pub input: StepInputValue,
    /// The agent's output.
    pub output: serde_json::Value,
    /// Tasks spawned by this completion. Post hook can modify this.
    pub next: Vec<Task>,
}

/// The action timed out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostHookTimeout {
    /// The task's input value.
    pub input: StepInputValue,
}

/// The action failed with an error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostHookError {
    /// The task's input value.
    pub input: StepInputValue,
    /// Error message.
    pub error: String,
}

/// The pre hook failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostHookPreHookError {
    /// The original input value (before pre hook).
    pub input: StepInputValue,
    /// Error message from pre hook.
    pub error: String,
}

/// Input/output for post hooks.
///
/// Post hooks receive this JSON on stdin and must output (possibly modified)
/// JSON on stdout. The `next` array can be filtered, added to, or transformed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PostHookInput {
    /// The action completed successfully.
    Success(PostHookSuccess),
    /// The action timed out.
    Timeout(PostHookTimeout),
    /// The action failed with an error.
    Error(PostHookError),
    /// The pre hook failed.
    PreHookError(PostHookPreHookError),
}

/// Run a pre hook and return the (possibly modified) value.
pub fn run_pre_hook(
    script: &HookScript,
    value: &serde_json::Value,
    working_dir: &Path,
) -> Result<serde_json::Value, String> {
    info!(script = %script, "running pre hook");

    let input = serde_json::to_string(value).unwrap_or_default();
    let stdout = run_shell_command(script.as_str(), &input, Some(working_dir))
        .map_err(|e| format!("pre hook {e}"))?;

    serde_json::from_str(&stdout)
        .map_err(|e| format!("pre hook output is not valid JSON: {e}"))
        .inspect(|_| {
            debug!("pre hook transformed value");
        })
}

/// Run a post hook synchronously and return the (possibly modified) result.
///
/// Post hooks can modify the `next` array to filter, add, or transform tasks.
pub fn run_post_hook(
    script: &HookScript,
    input: &PostHookInput,
    working_dir: &Path,
) -> Result<PostHookInput, String> {
    info!(script = %script, kind = ?std::mem::discriminant(input), "running post hook");

    let input_json = serde_json::to_string(&input).unwrap_or_default();
    let stdout = run_shell_command(script.as_str(), &input_json, Some(working_dir))
        .map_err(|e| format!("post hook {e}"))?;

    serde_json::from_str(&stdout)
        .map_err(|e| format!("post hook output is not valid JSON: {e}"))
        .inspect(|_| {
            debug!(script = %script.as_str(), "post hook completed");
        })
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
    run_shell_command(script, task_json, Some(working_dir))
        .map_err(|e| io::Error::other(format!("[E021] command {e}")))
}
