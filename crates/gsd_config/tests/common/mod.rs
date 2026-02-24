//! Shared test utilities for GSD integration tests.

// Test utilities can be more relaxed
#![allow(dead_code)]
#![expect(clippy::expect_used)]

use agent_pool::{AGENTS_DIR, RESPONSE_FILE, TASK_FILE};
use std::fs;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// Get the path to the test data directory for a given test file.
pub fn test_data_dir(test_file: &str) -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .join(".test-data")
        .join(test_file)
}

/// Clean up and create a fresh test directory.
pub fn setup_test_dir(test_file: &str) -> PathBuf {
    let dir = test_data_dir(test_file);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("Failed to create test directory");
    dir
}

/// Clean up a test directory.
pub fn cleanup_test_dir(test_file: &str) {
    let dir = test_data_dir(test_file);
    let _ = fs::remove_dir_all(&dir);
}

/// Check if Unix socket IPC is available.
#[cfg(unix)]
pub fn is_ipc_available(test_dir: &Path) -> bool {
    if std::env::var("SKIP_IPC_TESTS").is_ok() {
        return false;
    }

    let socket_path = test_dir.join("ipc_test.sock");
    let _ = fs::remove_file(&socket_path);

    let Ok(listener) = UnixListener::bind(&socket_path) else {
        return false;
    };

    listener
        .set_nonblocking(true)
        .expect("Failed to set non-blocking");

    let connect_result = UnixStream::connect(&socket_path);

    drop(listener);
    let _ = fs::remove_file(&socket_path);

    connect_result.is_ok()
}

/// Check if Unix socket IPC is available (non-Unix stub).
#[cfg(not(unix))]
pub fn is_ipc_available(_test_dir: &Path) -> bool {
    false
}

// =============================================================================
// GSD Test Agent
// =============================================================================

/// Parsed task envelope.
struct TaskEnvelope {
    kind: String,
    content: String,
}

/// Extract task kind and content from the envelope format.
///
/// The daemon writes `{"kind": "Task", "content": ...}` to task.json.
fn extract_task_envelope(raw: &str) -> TaskEnvelope {
    if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(raw) {
        let kind = envelope
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("Task")
            .to_string();

        let content = envelope
            .get("content")
            .map_or_else(|| raw.to_string(), serde_json::Value::to_string);

        return TaskEnvelope { kind, content };
    }
    // Not an envelope, return as-is
    TaskEnvelope {
        kind: "Task".to_string(),
        content: raw.to_string(),
    }
}

/// A test agent that understands the GSD protocol.
///
/// GSD sends JSON payloads like:
/// ```json
/// {"task": {"kind": "...", "value": {...}}, "instructions": "...", "timeout_seconds": 60}
/// ```
///
/// And expects JSON array responses:
/// ```json
/// [{"kind": "...", "value": {...}}, ...]
/// ```
pub struct GsdTestAgent {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<Vec<String>>>,
}

impl GsdTestAgent {
    /// Start a GSD test agent with a custom processing function.
    ///
    /// The processor receives the full payload JSON and returns the response JSON.
    pub fn start<F>(root: &Path, agent_id: &str, processing_delay: Duration, processor: F) -> Self
    where
        F: Fn(&str) -> String + Send + 'static,
    {
        let agent_dir = root.join(AGENTS_DIR).join(agent_id);
        fs::create_dir_all(&agent_dir).expect("Failed to create agent directory");

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let handle = thread::spawn(move || {
            let mut processed_tasks = Vec::new();
            let task_file = agent_dir.join(TASK_FILE);
            let response_file = agent_dir.join(RESPONSE_FILE);

            while running_clone.load(Ordering::SeqCst) {
                // Check for task file
                if task_file.exists() && !response_file.exists() {
                    let Ok(raw) = fs::read_to_string(&task_file) else {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    };

                    // Extract kind/content from envelope
                    let envelope = extract_task_envelope(&raw);

                    // Handle daemon control messages immediately
                    match envelope.kind.as_str() {
                        "Heartbeat" => {
                            let _ = fs::write(&response_file, "{}");
                            thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        "Kicked" => {
                            // Agent is being kicked, exit gracefully
                            break;
                        }
                        _ => {}
                    }

                    thread::sleep(processing_delay);

                    let response = processor(&envelope.content);

                    // Push BEFORE writing response to avoid race where daemon sees
                    // response but agent is stopped before incrementing count
                    processed_tasks.push(envelope.content.trim().to_string());

                    // Write response (daemon handles cleanup of both files)
                    let _ = fs::write(&response_file, &response);
                }

                thread::sleep(Duration::from_millis(10));
            }

            processed_tasks
        });

        Self {
            running,
            handle: Some(handle),
        }
    }

    /// Start an agent that always transitions to Done.
    pub fn terminator(root: &Path, agent_id: &str, processing_delay: Duration) -> Self {
        Self::start(root, agent_id, processing_delay, |_| "[]".to_string())
    }

    /// Start an agent that transitions to a fixed next step.
    pub fn transition_to(
        root: &Path,
        agent_id: &str,
        processing_delay: Duration,
        next_kind: &str,
    ) -> Self {
        let next_kind = next_kind.to_string();
        Self::start(root, agent_id, processing_delay, move |_| {
            format!(r#"[{{"kind": "{next_kind}", "value": {{}}}}]"#)
        })
    }

    /// Start a custom agent that maps task kinds to responses.
    pub fn with_transitions(
        root: &Path,
        agent_id: &str,
        processing_delay: Duration,
        transitions: Vec<(&str, &str)>,
    ) -> Self {
        let transitions: Vec<(String, String)> = transitions
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        Self::start(root, agent_id, processing_delay, move |payload| {
            // Parse the kind from the payload
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(payload)
                && let Some(kind) = parsed
                    .get("task")
                    .and_then(|t| t.get("kind"))
                    .and_then(|k| k.as_str())
            {
                for (from, to) in &transitions {
                    if kind == from {
                        if to.is_empty() {
                            return "[]".to_string();
                        }
                        return format!(r#"[{{"kind": "{to}", "value": {{}}}}]"#);
                    }
                }
            }
            // Default: terminate
            "[]".to_string()
        })
    }

    /// Stop the agent and return the list of payloads it processed.
    pub fn stop(mut self) -> Vec<String> {
        self.running.store(false, Ordering::SeqCst);
        self.handle
            .take()
            .expect("Agent already stopped")
            .join()
            .expect("Agent thread panicked")
    }
}

// =============================================================================
// Agent Pool Handle
// =============================================================================

/// Wrapper around the daemon handle for testing.
pub struct AgentPoolHandle {
    handle: Option<agent_pool::DaemonHandle>,
}

impl AgentPoolHandle {
    /// Start the agent pool daemon.
    ///
    /// The daemon signals readiness internally before `spawn()` returns.
    pub fn start(root: &Path) -> Self {
        let handle = agent_pool::spawn(root).expect("Failed to start daemon");
        Self {
            handle: Some(handle),
        }
    }
}

impl Drop for AgentPoolHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown();
        }
    }
}
