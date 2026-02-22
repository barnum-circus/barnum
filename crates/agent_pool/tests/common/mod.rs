//! Shared test utilities for agent pool integration tests.

// Test utilities can be more relaxed
#![allow(dead_code)]
#![expect(clippy::expect_used)]
#![expect(clippy::collapsible_if)]

use agent_pool::{AGENTS_DIR, INPUT_EXT, OUTPUT_EXT};
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

            while running_clone.load(Ordering::SeqCst) {
                // Find an input file
                if let Some((task_id, task)) = find_input_file(&agent_dir) {
                    let input_path = agent_dir.join(format!("{task_id}.{INPUT_EXT}"));

                    thread::sleep(processing_delay);

                    let response = processor(&task, &agent_id_owned);
                    processed_tasks.push(task.trim().to_string());

                    // Check if we were timed out (input deleted)
                    if input_path.exists() {
                        let output_path = agent_dir.join(format!("{task_id}.{OUTPUT_EXT}"));
                        let _ = fs::write(&output_path, &response);
                        let _ = fs::remove_file(&input_path);
                    }
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

/// Find an input file in the agent directory, returning (`task_id`, content).
fn find_input_file(agent_dir: &Path) -> Option<(u64, String)> {
    let Ok(entries) = fs::read_dir(agent_dir) else {
        return None;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(stem) = name.strip_suffix(&format!(".{INPUT_EXT}")) {
                if let Ok(task_id) = stem.parse::<u64>() {
                    if let Ok(content) = fs::read_to_string(&path) {
                        return Some((task_id, content));
                    }
                }
            }
        }
    }

    None
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
