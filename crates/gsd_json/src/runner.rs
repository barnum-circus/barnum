//! State machine runner for GSD.
//!
//! Executes tasks through `agent_pool`, validating transitions and handling timeouts.

use crate::config::Config;
use crate::docs::generate_step_docs;
use crate::schema::{CompiledSchemas, Task, validate_response};
use agent_pool::{Response, ResponseKind};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::Path;
use std::process::Command;
use tracing::{debug, error, info, warn};

/// Runner configuration.
pub struct RunnerConfig<'a> {
    /// Path to the `agent_pool` root directory.
    pub agent_pool_root: &'a Path,
    /// Optional wake script to call before starting.
    pub wake_script: Option<&'a str>,
    /// Initial tasks to process.
    pub initial_tasks: Vec<Task>,
}

/// Run the state machine to completion.
///
/// # Errors
///
/// Returns an error if:
/// - The `agent_pool` can't be reached
/// - The wake script fails
/// - An I/O error occurs
pub fn run(
    config: &Config,
    schemas: &CompiledSchemas,
    runner_config: RunnerConfig<'_>,
) -> io::Result<()> {
    if let Some(script) = runner_config.wake_script {
        call_wake_script(script)?;
    }

    let step_map = config.step_map();
    let mut queue: VecDeque<Task> = runner_config.initial_tasks.into();
    let mut retry_counts: HashMap<String, u32> = HashMap::new();

    info!(tasks = queue.len(), "starting state machine");

    while let Some(task) = queue.pop_front() {
        process_task(
            &task,
            config,
            schemas,
            &step_map,
            runner_config.agent_pool_root,
            &mut queue,
            &mut retry_counts,
        );
    }

    info!("state machine complete");
    Ok(())
}

fn call_wake_script(script: &str) -> io::Result<()> {
    info!(script, "calling wake script");
    let status = Command::new("sh").arg("-c").arg(script).status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "wake script failed with status: {status}"
        )))
    }
}

fn process_task(
    task: &Task,
    config: &Config,
    schemas: &CompiledSchemas,
    step_map: &HashMap<&str, &crate::config::Step>,
    agent_pool_root: &Path,
    queue: &mut VecDeque<Task>,
    retry_counts: &mut HashMap<String, u32>,
) {
    let Some(step) = step_map.get(task.kind.as_str()) else {
        error!(kind = task.kind, "unknown step, skipping task");
        return;
    };

    if let Err(e) = schemas.validate(&task.kind, &task.value) {
        error!(kind = task.kind, error = %e, "task validation failed, skipping");
        return;
    }

    let docs = generate_step_docs(step, config);
    let payload = build_agent_payload(task, &docs, config.options.timeout);

    info!(kind = task.kind, "submitting task");
    debug!(payload = %payload, "task payload");

    let result = submit_with_timeout(agent_pool_root, &payload, config.options.timeout);

    handle_submit_result(
        result,
        task.clone(),
        step,
        schemas,
        config.options.max_retries,
        queue,
        retry_counts,
    );
}

fn handle_submit_result(
    result: io::Result<Response>,
    task: Task,
    step: &crate::config::Step,
    schemas: &CompiledSchemas,
    max_retries: u32,
    queue: &mut VecDeque<Task>,
    retry_counts: &mut HashMap<String, u32>,
) {
    match result {
        Ok(response) => {
            handle_response(
                response,
                task,
                step,
                schemas,
                max_retries,
                queue,
                retry_counts,
            );
        }
        Err(e) => {
            error!(kind = task.kind, error = %e, "submit failed, requeuing");
            requeue_with_retry(queue, retry_counts, task, max_retries);
        }
    }
}

fn handle_response(
    response: Response,
    task: Task,
    step: &crate::config::Step,
    schemas: &CompiledSchemas,
    max_retries: u32,
    queue: &mut VecDeque<Task>,
    retry_counts: &mut HashMap<String, u32>,
) {
    match response.kind {
        ResponseKind::Processed => {
            let stdout = response.stdout.unwrap_or_default();
            debug!(stdout = %stdout, "agent response");

            match serde_json::from_str::<serde_json::Value>(&stdout) {
                Ok(value) => match validate_response(&value, step, schemas) {
                    Ok(new_tasks) => {
                        info!(
                            from = task.kind,
                            new_tasks = new_tasks.len(),
                            "transition successful"
                        );
                        for new_task in new_tasks {
                            queue.push_back(new_task);
                        }
                    }
                    Err(e) => {
                        warn!(kind = task.kind, error = %e, "invalid response, requeuing task");
                        requeue_with_retry(queue, retry_counts, task, max_retries);
                    }
                },
                Err(e) => {
                    warn!(kind = task.kind, error = %e, "failed to parse response JSON, requeuing");
                    requeue_with_retry(queue, retry_counts, task, max_retries);
                }
            }
        }
        ResponseKind::NotProcessed => {
            let reason = response
                .reason
                .map_or_else(|| "unknown".to_string(), |r| format!("{r:?}"));
            warn!(kind = task.kind, reason, "task not processed, requeuing");
            requeue_with_retry(queue, retry_counts, task, max_retries);
        }
    }
}

fn build_agent_payload(task: &Task, docs: &str, timeout: Option<u64>) -> String {
    let mut payload = serde_json::json!({
        "task": {
            "kind": task.kind,
            "value": task.value,
        },
        "instructions": docs,
    });

    if let Some(t) = timeout {
        payload["timeout_seconds"] = serde_json::json!(t);
    }

    serde_json::to_string(&payload).unwrap_or_default()
}

fn submit_with_timeout(root: &Path, payload: &str, timeout: Option<u64>) -> io::Result<Response> {
    // TODO: Implement actual timeout with process killing
    if let Some(t) = timeout {
        debug!(timeout = t, "timeout configured but not yet enforced");
    }

    agent_pool::submit(root, payload)
}

fn requeue_with_retry(
    queue: &mut VecDeque<Task>,
    retry_counts: &mut HashMap<String, u32>,
    task: Task,
    max_retries: u32,
) {
    let task_key = format!(
        "{}:{}",
        task.kind,
        serde_json::to_string(&task.value).unwrap_or_default()
    );
    let count = retry_counts.entry(task_key).or_insert(0);
    *count += 1;

    if *count <= max_retries {
        info!(
            kind = task.kind,
            retry = *count,
            max = max_retries,
            "requeuing task"
        );
        queue.push_back(task);
    } else {
        error!(
            kind = task.kind,
            retries = *count,
            "max retries exceeded, dropping task"
        );
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn build_payload_includes_task_and_docs() {
        let task = Task {
            kind: "Test".to_string(),
            value: serde_json::json!({"x": 1}),
        };
        let docs = "# Test Step";

        let payload = build_agent_payload(&task, docs, Some(60));
        let parsed: serde_json::Value = serde_json::from_str(&payload).unwrap();

        assert_eq!(parsed["task"]["kind"], "Test");
        assert_eq!(parsed["timeout_seconds"], 60);
        assert!(
            parsed["instructions"]
                .as_str()
                .unwrap()
                .contains("Test Step")
        );
    }
}
