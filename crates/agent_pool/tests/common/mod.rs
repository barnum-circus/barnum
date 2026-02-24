//! Shared test utilities for agent pool integration tests.

// Test utilities can be more relaxed
#![allow(dead_code)]
#![expect(clippy::expect_used)]

use agent_pool::{
    AGENTS_DIR, RESPONSE_FILE, TASK_FILE, Transport, create_watcher, wait_for_task_with_timeout,
};
use std::fs;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Get the path to the test data directory for a given test file.
/// Each test file gets its own unique subdirectory to avoid conflicts.
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
// Test Agent
// =============================================================================

/// Parsed task envelope.
struct TaskEnvelope {
    kind: String,
    content: String,
}

/// Extract task kind and content from the envelope format.
///
/// The daemon writes `{"kind": "Task", "content": ...}` to task.json.
/// Returns (kind, content) tuple.
#[allow(clippy::option_if_let_else)] // if-let-else is clearer here
fn extract_task_envelope(raw: &str) -> TaskEnvelope {
    if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(raw) {
        let kind = envelope
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("Task")
            .to_string();

        let content = if let Some(content) = envelope.get("content") {
            // If content is a string, return it directly
            if let Some(s) = content.as_str() {
                s.to_string()
            } else {
                // Otherwise return the JSON representation
                content.to_string()
            }
        } else {
            raw.to_string()
        };

        return TaskEnvelope { kind, content };
    }
    // Not an envelope, return as-is
    TaskEnvelope {
        kind: "Task".to_string(),
        content: raw.to_string(),
    }
}

/// A test agent that polls for tasks and processes them with a custom function.
///
/// The agent runs in a background thread, watching for `*.input` files,
/// processing them, and writing results to `*.output`.
pub struct TestAgent {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<Vec<String>>>,
    /// Receiver that signals when the agent has processed its first message (heartbeat).
    /// This allows tests to wait for the agent to be fully ready without arbitrary sleeps.
    ready_rx: Option<mpsc::Receiver<()>>,
}

impl TestAgent {
    /// Start a test agent with a custom processing function.
    ///
    /// The processor receives the task content and agent ID, returning the response.
    ///
    /// After starting, call `wait_ready()` to block until the agent has processed
    /// its first message (heartbeat) and is ready to receive real tasks.
    ///
    /// Uses notify-based waiting instead of polling for better performance.
    pub fn start<F>(root: &Path, agent_id: &str, processing_delay: Duration, processor: F) -> Self
    where
        F: Fn(&str, &str) -> String + Send + 'static,
    {
        let agent_dir = root.join(AGENTS_DIR).join(agent_id);
        fs::create_dir_all(&agent_dir).expect("Failed to create agent directory");

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let agent_id_owned = agent_id.to_string();

        // Channel to signal when the agent has processed its first message (heartbeat)
        let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(0);

        // Set up notify-based waiting
        let transport = Transport::Directory(agent_dir);
        let (watcher, events_rx) = create_watcher(&transport)
            .expect("Failed to create watcher")
            .expect("Expected directory transport");

        let handle = thread::spawn(move || {
            // Keep watcher alive for the duration of the thread
            let _watcher = watcher;
            let mut processed_tasks = Vec::new();
            let mut first_message_processed = false;

            while running_clone.load(Ordering::SeqCst) {
                // Wait for task using notify with timeout so we can check running flag
                // Use short timeout (10ms) to check running flag frequently
                // FSEvents on macOS can have high latency, so we rely on the
                // is_task_ready() check at the start of wait_for_task_with_timeout
                match wait_for_task_with_timeout(&transport, &events_rx, Duration::from_millis(10))
                {
                    Ok(true) => {
                        // Task ready, continue to process
                    }
                    Ok(false) => {
                        // Timeout, check running flag and try again
                        continue;
                    }
                    Err(_) => {
                        // Watcher error or channel closed
                        break;
                    }
                }

                // Read task
                let Ok(raw) = transport.read(TASK_FILE) else {
                    continue;
                };

                // Extract kind/content from envelope
                let envelope = extract_task_envelope(&raw);

                // Handle daemon control messages
                match envelope.kind.as_str() {
                    "Heartbeat" => {
                        let _ = transport.write(RESPONSE_FILE, "{}");
                        // Signal ready after processing first heartbeat
                        if !first_message_processed {
                            first_message_processed = true;
                            let _ = ready_tx.send(());
                        }
                    }
                    "Kicked" => {
                        break;
                    }
                    _ => {
                        thread::sleep(processing_delay);

                        let response = processor(&envelope.content, &agent_id_owned);
                        processed_tasks.push(envelope.content.trim().to_string());

                        // Write response (daemon handles cleanup of both files)
                        let _ = transport.write(RESPONSE_FILE, &response);

                        // Signal ready AFTER writing response for non-heartbeat messages
                        if !first_message_processed {
                            first_message_processed = true;
                            let _ = ready_tx.send(());
                        }
                    }
                }
            }

            processed_tasks
        });

        Self {
            running,
            handle: Some(handle),
            ready_rx: Some(ready_rx),
        }
    }

    /// Start a simple echo agent that appends " [processed]" to inputs.
    pub fn echo(root: &Path, agent_id: &str, processing_delay: Duration) -> Self {
        Self::start(root, agent_id, processing_delay, |task, _| {
            format!("{} [processed]", task.trim())
        })
    }

    /// Start a greeting agent that responds to "casual" and "formal" styles.
    pub fn greeting(root: &Path, agent_id: &str, processing_delay: Duration) -> Self {
        Self::start(
            root,
            agent_id,
            processing_delay,
            |task, agent_id| match task.trim() {
                "casual" => format!("Hi {agent_id}, how are ya?"),
                "formal" => format!(
                    "Salutations {agent_id}, how are you doing on this most splendiferous and utterly magnificent day?"
                ),
                style => format!("Error: unknown style '{style}' (use 'casual' or 'formal')"),
            },
        )
    }

    /// Wait for the agent to be ready (has processed its first message).
    ///
    /// This blocks until the agent has received and processed the initial heartbeat
    /// from the daemon, indicating it's fully registered and ready to receive tasks.
    ///
    /// # Panics
    ///
    /// Panics if the agent thread exits before signaling readiness.
    pub fn wait_ready(&mut self) {
        if let Some(rx) = self.ready_rx.take() {
            rx.recv().expect("Agent exited before signaling readiness");
        }
        // If ready_rx is None, we've already waited - that's fine
    }

    /// Stop the agent and return the list of tasks it processed.
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
///
/// Automatically shuts down the daemon when dropped.
pub struct AgentPoolHandle {
    handle: Option<agent_pool::DaemonHandle>,
}

impl AgentPoolHandle {
    /// Start the agent pool daemon with graceful shutdown support.
    pub fn start(root: &Path) -> Self {
        let handle = agent_pool::spawn(root).expect("Failed to start daemon");
        Self {
            handle: Some(handle),
        }
    }

    /// Start the agent pool daemon with custom configuration.
    pub fn start_with_config(root: &Path, config: agent_pool::DaemonConfig) -> Self {
        let handle = agent_pool::spawn_with_config(root, config).expect("Failed to start daemon");
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
