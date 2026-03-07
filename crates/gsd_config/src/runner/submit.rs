//! Task submission to the agent pool.

use agent_pool::Response;
use agent_pool_cli::AgentPoolCli;
use cli_invoker::Invoker;
use std::io;
use std::path::Path;

use crate::types::StepName;

/// Build agent payload with a specific value (used when pre hook modifies the value).
pub fn build_agent_payload(
    step_name: &StepName,
    value: &serde_json::Value,
    docs: &str,
    timeout: Option<u64>,
) -> String {
    let mut payload = serde_json::json!({
        "task": { "kind": step_name, "value": value },
        "instructions": docs,
    });
    if let Some(t) = timeout {
        payload["timeout_seconds"] = serde_json::json!(t);
    }
    serde_json::to_string(&payload).unwrap_or_default()
}

/// Submit a task via the CLI instead of internal API.
pub fn submit_via_cli(
    pool_path: &Path,
    payload: &str,
    invoker: &Invoker<AgentPoolCli>,
) -> io::Result<Response> {
    // Extract root (grandparent) and pool_id (basename) from full path
    // pool_path is like /tmp/root/pools/demo
    // We need root=/tmp/root (grandparent) and pool_id=demo (basename)
    // because --root expects the root without pools/, and adds pools/ internally
    let root = pool_path
        .parent() // /tmp/root/pools
        .and_then(|p| p.parent()) // /tmp/root
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "[E014] invalid pool path (need grandparent): {}",
                    pool_path.display()
                ),
            )
        })?;
    let pool_id = pool_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "[E015] invalid pool path (no basename): {}",
                    pool_path.display()
                ),
            )
        })?;

    // Use 24-hour timeout. TODO: Add --no-timeout support to CLI.
    let output = invoker.run([
        "submit_task",
        "--root",
        root.to_str().unwrap_or("."),
        "--pool",
        pool_id,
        "--notify",
        "file",
        "--timeout-secs",
        "86400",
        "--data",
        payload,
    ])?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "[E016] agent_pool submit_task failed\n  invoker: {}\n  root: {}\n  pool: {}\n  error: {}",
            invoker.description(),
            root.display(),
            pool_id,
            stderr.trim()
        )));
    }

    serde_json::from_slice(&output.stdout).map_err(|e| {
        let stdout = String::from_utf8_lossy(&output.stdout);
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "[E017] failed to parse agent_pool output\n  invoker: {}\n  pool: {}\n  error: {e}\n  stdout: {}",
                invoker.description(),
                pool_path.display(),
                stdout.trim()
            ),
        )
    })
}
