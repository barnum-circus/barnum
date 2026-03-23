//! Task submission to the troupe pool.

use cli_invoker::Invoker;
use std::io;
use std::path::Path;
use troupe::Response;
use troupe_cli::TroupeCli;

use crate::types::StepName;

/// Build the agent payload JSON string.
///
/// `pool_timeout` is the agent lifecycle timeout from the pool action config.
/// If set, it's included as `timeout_seconds` in the payload. This is separate
/// from barnum's worker timeout (step.options.timeout).
pub fn build_agent_payload(
    step_name: &StepName,
    value: &serde_json::Value,
    docs: &str,
    pool_timeout: Option<u64>,
) -> String {
    let mut payload = serde_json::json!({
        "task": { "kind": step_name, "value": value },
        "instructions": docs,
    });
    if let Some(t) = pool_timeout {
        payload["timeout_seconds"] = serde_json::json!(t);
    }
    serde_json::to_string(&payload).unwrap_or_default()
}

/// Submit a task to the pool via the troupe CLI.
///
/// `root` and `pool` are optional — if not provided, troupe uses its own defaults.
pub fn submit_via_cli(
    root: Option<&Path>,
    pool: Option<&str>,
    payload: &str,
    invoker: &Invoker<TroupeCli>,
) -> io::Result<Response> {
    let mut args = vec!["submit_task"];
    let root_str;
    if let Some(root) = root {
        root_str = root.to_string_lossy().into_owned();
        args.extend(["--root", &root_str]);
    }
    if let Some(pool) = pool {
        args.extend(["--pool", pool]);
    }
    args.extend(["--notify", "file", "--data", payload]);

    let output = invoker.run(&args)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "[E016] troupe submit_task failed\n  invoker: {}\n  root: {}\n  pool: {}\n  error: {}",
            invoker.description(),
            root.map_or("<default>", |r| r.to_str().unwrap_or("?")),
            pool.unwrap_or("<default>"),
            stderr.trim()
        )));
    }

    serde_json::from_slice(&output.stdout).map_err(|e| {
        let stdout = String::from_utf8_lossy(&output.stdout);
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "[E017] failed to parse troupe output\n  invoker: {}\n  root: {}\n  pool: {}\n  error: {e}\n  stdout: {}",
                invoker.description(),
                root.map_or("<default>", |r| r.to_str().unwrap_or("?")),
                pool.unwrap_or("<default>"),
                stdout.trim()
            ),
        )
    })
}
