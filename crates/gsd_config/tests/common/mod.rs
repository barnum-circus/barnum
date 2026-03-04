//! Shared test utilities for GSD integration tests.

// Test utilities can be more relaxed
#![allow(dead_code)]
#![expect(clippy::expect_used)]

use agent_pool::{STATUS_FILE, TaskAssignment, VerifiedWatcher, wait_for_task, write_response};
use std::fs;
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
    pool_root: PathBuf,
}

impl GsdTestAgent {
    /// Start a GSD test agent with a custom processing function.
    ///
    /// Uses the anonymous worker protocol:
    /// 1. Writes `<uuid>.ready.json` to signal availability
    /// 2. Waits for `<uuid>.task.json` from daemon
    /// 3. Processes task and writes `<uuid>.response.json`
    ///
    /// The processor receives the full payload JSON and returns the response JSON.
    pub fn start<F>(root: &Path, processing_delay: Duration, processor: F) -> Self
    where
        F: Fn(&str) -> String + Send + 'static,
    {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let pool_root = root.to_path_buf();

        let handle = thread::spawn(move || {
            let mut processed_tasks = Vec::new();

            // Create watcher once for the thread
            let mut watcher =
                match VerifiedWatcher::new(&pool_root, std::slice::from_ref(&pool_root)) {
                    Ok(w) => w,
                    Err(e) => {
                        eprintln!("Failed to create watcher: {e}");
                        return processed_tasks;
                    }
                };

            while running_clone.load(Ordering::SeqCst) {
                // Use timeout so we can check running flag periodically.
                // CLI stop may not reliably cause the watcher to error.
                let Ok(assignment) = wait_for_task(
                    &mut watcher,
                    &pool_root,
                    None,
                    Some(Duration::from_millis(500)),
                ) else {
                    // Timeout or error - check running flag and retry
                    continue;
                };

                let TaskAssignment { uuid, content } = assignment;
                let envelope = extract_task_envelope(&content);

                // Handle daemon control messages (Kicked = shutdown)
                if envelope.kind == "Kicked" {
                    break;
                }

                thread::sleep(processing_delay);

                let response = processor(&envelope.content);

                // Track processed tasks before writing response
                processed_tasks.push(envelope.content.trim().to_string());

                let _ = write_response(&pool_root, &uuid, &response);
            }

            processed_tasks
        });

        Self {
            running,
            handle: Some(handle),
            pool_root: root.to_path_buf(),
        }
    }

    /// Start an agent that always transitions to Done.
    pub fn terminator(root: &Path, processing_delay: Duration) -> Self {
        Self::start(root, processing_delay, |_| "[]".to_string())
    }

    /// Start an agent that transitions to a fixed next step.
    pub fn transition_to(root: &Path, processing_delay: Duration, next_kind: &str) -> Self {
        let next_kind = next_kind.to_string();
        Self::start(root, processing_delay, move |_| {
            format!(r#"[{{"kind": "{next_kind}", "value": {{}}}}]"#)
        })
    }

    /// Start a custom agent that maps task kinds to responses.
    pub fn with_transitions(
        root: &Path,
        processing_delay: Duration,
        transitions: Vec<(&str, &str)>,
    ) -> Self {
        let transitions: Vec<(String, String)> = transitions
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        Self::start(root, processing_delay, move |payload| {
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
    ///
    /// Sets the running flag to false and stops the daemon via CLI.
    /// The agent thread exits on the next timeout check.
    pub fn stop(mut self) -> Vec<String> {
        self.running.store(false, Ordering::SeqCst);
        // Stop the daemon via CLI - this kicks all agents as part of cleanup
        let bin = find_agent_pool_binary();
        let _ = Command::new(&bin)
            .arg("stop")
            .arg("--pool-root")
            .arg(self.pool_root.parent().unwrap_or(&self.pool_root))
            .arg("--pool")
            .arg(self.pool_root.file_name().unwrap_or_default())
            .output();
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

/// Find the `agent_pool` binary.
pub fn find_agent_pool_binary() -> PathBuf {
    if let Ok(bin) = std::env::var("AGENT_POOL_BIN") {
        return PathBuf::from(bin);
    }

    // Find workspace root by looking for Cargo.toml with [workspace]
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("Could not find workspace root");

    workspace_root.join("target/debug/agent_pool")
}

/// Wrapper that starts the daemon via CLI subprocess.
///
/// Automatically shuts down the daemon when dropped.
pub struct AgentPoolHandle {
    root: PathBuf,
    process: Option<Child>,
    /// Handles for threads forwarding stdout/stderr (so they get captured by tests)
    _output_threads: Vec<thread::JoinHandle<()>>,
}

impl AgentPoolHandle {
    /// Start the agent pool daemon.
    pub fn start(root: &Path) -> Self {
        let bin = find_agent_pool_binary();
        assert!(
            bin.exists(),
            "agent_pool binary not found at {}. Run `cargo build -p agent_pool_cli` first.",
            bin.display()
        );

        // Build command - use --pool-root and pool name
        let mut cmd = Command::new(&bin);
        cmd.arg("start")
            .arg("--pool-root")
            .arg(root.parent().unwrap_or(root))
            .arg("--pool")
            .arg(root.file_name().unwrap_or_default())
            .arg("--log-level")
            .arg("trace")
            // No heartbeats needed - agents signal ready immediately
            .arg("--no-heartbeat");

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut process = cmd.spawn().expect("Failed to spawn agent_pool process");

        // Set up output capture
        let mut output_threads = Vec::new();

        if let Some(stdout) = process.stdout.take() {
            output_threads.push(thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[daemon stdout] {line}");
                }
            }));
        }

        if let Some(stderr) = process.stderr.take() {
            output_threads.push(thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[daemon stderr] {line}");
                }
            }));
        }

        // Wait for daemon to be ready using a filesystem watcher.
        //
        // Race condition: The daemon CLI deletes and recreates the pool directory on
        // startup. If we create the pool directory here, the daemon may delete it while
        // we're setting up the watcher, causing `watcher.watch()` to fail with PathNotFound.
        //
        // Solution: Watch the parent directory (pool_root) instead, which is never deleted.
        // The watcher will see the status file when the daemon creates it in the subdirectory.
        let pool_root = root.parent().unwrap_or(root);
        fs::create_dir_all(pool_root).expect("Failed to create pool root directory");
        let pool_root_buf = pool_root.to_path_buf();
        let mut watcher = VerifiedWatcher::new(pool_root, std::slice::from_ref(&pool_root_buf))
            .expect("Failed to create watcher");
        let status_path = root.join(STATUS_FILE);
        watcher
            .wait_for_file_with_timeout(&status_path, Duration::from_secs(10))
            .expect("Agent pool did not become ready in time");

        Self {
            root: root.to_path_buf(),
            process: Some(process),
            _output_threads: output_threads,
        }
    }
}

impl Drop for AgentPoolHandle {
    fn drop(&mut self) {
        // Try graceful shutdown via CLI
        let bin = find_agent_pool_binary();
        let _ = Command::new(&bin)
            .arg("stop")
            .arg("--pool-root")
            .arg(self.root.parent().unwrap_or(&self.root))
            .arg("--pool")
            .arg(self.root.file_name().unwrap_or_default())
            .output();

        // Kill the process if still running
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}
