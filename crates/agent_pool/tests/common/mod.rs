//! Shared test utilities for agent pool integration tests.

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

/// Extract task content from the envelope format.
///
/// The daemon writes `{"kind": "Task", "content": ...}` to task.json.
/// This extracts the content field, or returns the raw input if not in envelope format.
fn extract_task_content(raw: &str) -> String {
    if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(content) = envelope.get("content") {
            // If content is a string, return it directly
            if let Some(s) = content.as_str() {
                return s.to_string();
            }
            // Otherwise return the JSON representation
            return content.to_string();
        }
    }
    // Not an envelope, return as-is
    raw.to_string()
}

/// A test agent that polls for tasks and processes them with a custom function.
///
/// The agent runs in a background thread, watching for `*.input` files,
/// processing them, and writing results to `*.output`.
pub struct TestAgent {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<Vec<String>>>,
}

impl TestAgent {
    /// Start a test agent with a custom processing function.
    ///
    /// The processor receives the task content and agent ID, returning the response.
    pub fn start<F>(root: &Path, agent_id: &str, processing_delay: Duration, processor: F) -> Self
    where
        F: Fn(&str, &str) -> String + Send + 'static,
    {
        let agent_dir = root.join(AGENTS_DIR).join(agent_id);
        fs::create_dir_all(&agent_dir).expect("Failed to create agent directory");

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let agent_id_owned = agent_id.to_string();

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

                    // Extract content from envelope {"kind": "...", "content": ...}
                    let task = extract_task_content(&raw);

                    thread::sleep(processing_delay);

                    let response = processor(&task, &agent_id_owned);
                    processed_tasks.push(task.trim().to_string());

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
}

impl Drop for AgentPoolHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown();
        }
    }
}
