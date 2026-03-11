//! Shared test utilities for Barnum integration tests.

// Test utilities can be more relaxed
#![allow(dead_code)]
#![expect(clippy::expect_used)]

/// Default number of concurrent workers for ordered agent mode.
/// Keep this low because each watcher takes several seconds to create on macOS
/// due to `FSEvents` batching/coalescing behavior.
const ORDERED_AGENT_WORKER_COUNT: usize = 4;

use cli_invoker::Invoker;
use std::fs;
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use troupe::{STATUS_FILE, TaskAssignment, VerifiedWatcher, wait_for_task, write_response};
use troupe_cli::TroupeCli;

/// Get the path to the test data directory for a given test file.
fn test_data_dir(test_file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".td")
        .join(test_file)
}

/// Clean up and create a fresh test directory.
///
/// Uses a fixed directory name (no unique suffix) so that the pool
/// directory tree at `<parent>/pools/<name>/` persists across test runs.
/// On macOS, `FSEvents` needs several seconds to warm up for newly created
/// directory trees; reusing existing directories avoids this penalty.
///
/// Any stale daemon from a previous run is stopped before returning.
pub fn setup_test_dir(test_file: &str) -> PathBuf {
    let dir = test_data_dir(test_file);
    // Clean test data dir (marker files, etc.) but leave the pool
    // directory tree intact for FSEvents warmth.
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("Failed to create test directory");
    // Stop any stale daemon left by a crashed/timed-out previous run.
    stop_stale_daemon(&dir);
    dir
}

/// Stop any daemon that may be running for this test's pool.
///
/// When rstest's `#[timeout]` fires, the test thread is abandoned without
/// running `TroupeHandle::Drop`, leaking the daemon process. This sends
/// a stop signal so the new daemon can acquire the lock.
fn stop_stale_daemon(root: &Path) {
    let cli_root = root.parent().unwrap_or(root);
    let pool_name = root.file_name().unwrap_or_default();
    let bin = find_troupe_binary();
    let _ = Command::new(&bin)
        .arg("stop")
        .arg("--root")
        .arg(cli_root)
        .arg("--pool")
        .arg(pool_name)
        .output();
}

/// Clean up a test directory.
///
/// Removes the test data directory but preserves the pool directory
/// at `<parent>/pools/<name>/`. Keeping the pool directory tree allows
/// `FSEvents` to stay warm across test runs, avoiding the ~9 second
/// startup penalty on macOS.
pub fn cleanup_test_dir(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
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
// Barnum Test Agent
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

/// A test agent that understands the Barnum protocol.
///
/// Barnum sends JSON payloads like:
/// ```json
/// {"task": {"kind": "...", "value": {...}}, "instructions": "...", "timeout_seconds": 60}
/// ```
///
/// And expects JSON array responses:
/// ```json
/// [{"kind": "...", "value": {...}}, ...]
/// ```
pub struct BarnumTestAgent {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<Vec<String>>>,
    /// Handles for ordered mode workers (when using `ordered()`).
    ordered_handles: Option<Vec<thread::JoinHandle<()>>>,
    pool_root: PathBuf,
}

impl BarnumTestAgent {
    /// Start a Barnum test agent with a custom processing function.
    ///
    /// Uses the anonymous worker protocol:
    /// 1. Writes `<uuid>.ready.json` to signal availability
    /// 2. Waits for `<uuid>.task.json` from daemon
    /// 3. Processes task and writes `<uuid>.response.json`
    ///
    /// The processor receives the full payload JSON and returns the response JSON.
    ///
    /// The `root` parameter is the logical pool path (e.g., `.test-data/test_name`).
    /// The CLI adds `pools/` internally, so the actual pool path is `<parent>/pools/<name>`.
    pub fn start<F>(root: &Path, processing_delay: Duration, processor: F) -> Self
    where
        F: Fn(&str) -> String + Send + 'static,
    {
        // Compute actual pool path the same way TroupeHandle::start does.
        // The CLI adds pools/ between root and pool name.
        let cli_root = root.parent().unwrap_or(root);
        let pool_name = root.file_name().unwrap_or_default();
        let actual_pool_path = cli_root.join("pools").join(pool_name);

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let pool_path_for_thread = actual_pool_path.clone();

        let handle = thread::spawn(move || {
            let mut processed_tasks = Vec::new();

            // Create watcher once for the thread
            let mut watcher = match VerifiedWatcher::new(
                &pool_path_for_thread,
                std::slice::from_ref(&pool_path_for_thread),
            ) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("Failed to create watcher: {e}");
                    return processed_tasks;
                }
            };

            while running_clone.load(Ordering::SeqCst) {
                // Block until assigned or pool stops (Kicked/Stopped).
                // No timeout — avoids UUID flooding from generating a new
                // UUID per retry. Shutdown comes via the daemon stop protocol.
                let Ok(assignment) = wait_for_task(&mut watcher, &pool_path_for_thread, None, None)
                else {
                    // Pool stopped or I/O error - check running flag and retry
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

                let _ = write_response(&pool_path_for_thread, &uuid, &response);
            }

            processed_tasks
        });

        Self {
            running,
            handle: Some(handle),
            ordered_handles: None,
            pool_root: actual_pool_path,
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
        // self.pool_root is the actual pool path: <cli_root>/pools/<pool_name>
        // We need: --root <cli_root> --pool <pool_name>
        // So: --root = grandparent of self.pool_root (skip pools/)
        let cli_root = self
            .pool_root
            .parent() // <cli_root>/pools
            .and_then(|p| p.parent()) // <cli_root>
            .unwrap_or(&self.pool_root);
        let pool_name = self.pool_root.file_name().unwrap_or_default();

        let bin = find_troupe_binary();
        let _ = Command::new(&bin)
            .arg("stop")
            .arg("--root")
            .arg(cli_root)
            .arg("--pool")
            .arg(pool_name)
            .output();

        // Handle ordered mode (multiple worker handles, no return value)
        if let Some(handles) = self.ordered_handles.take() {
            for handle in handles {
                let _ = handle.join();
            }
            return Vec::new();
        }

        // Handle normal mode (single handle with return value)
        self.handle
            .take()
            .expect("Agent already stopped")
            .join()
            .expect("Agent thread panicked")
    }

    /// Start an agent that waits for explicit completion signals.
    ///
    /// Tasks register with the controller when they arrive and block
    /// until the test explicitly completes them. Tests can complete
    /// tasks in any order by index.
    ///
    /// Spawns multiple concurrent worker threads to handle multiple tasks in parallel.
    ///
    /// Returns (agent, controller).
    pub fn ordered(root: &Path) -> (Self, OrderedAgentController) {
        Self::ordered_with_workers(root, ORDERED_AGENT_WORKER_COUNT)
    }

    /// Start an ordered agent with a specific number of concurrent workers.
    ///
    /// Uses a single dispatcher thread with one watcher that fans out tasks
    /// to worker threads via channels. This is much faster than creating
    /// a watcher per worker.
    pub fn ordered_with_workers(
        root: &Path,
        _worker_count: usize,
    ) -> (Self, OrderedAgentController) {
        let waiting: Arc<Mutex<Vec<WaitingTask>>> = Arc::new(Mutex::new(Vec::new()));
        let (arrival_tx, arrival_rx) = mpsc::channel::<()>();
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        // Compute actual pool path
        let cli_root = root.parent().unwrap_or(root);
        let pool_name = root.file_name().unwrap_or_default();
        let actual_pool_path = cli_root.join("pools").join(pool_name);
        let pool_path = actual_pool_path.clone();

        // Single dispatcher thread handles all task reception.
        // Uses one watcher that loops, receiving tasks and spawning
        // handler threads for each.
        let waiting_clone = waiting.clone();
        let handle = thread::spawn(move || {
            let canary_dirs = [pool_path.clone()];
            let Ok(mut watcher) = VerifiedWatcher::new(&pool_path, &canary_dirs) else {
                eprintln!("[ordered dispatcher] watcher creation failed");
                return Vec::new();
            };

            let mut processed_tasks = Vec::new();

            while running_clone.load(Ordering::SeqCst) {
                // Block until assigned or pool stops (Kicked/Stopped).
                let task_result = wait_for_task(&mut watcher, &pool_path, None, None);

                let Ok(assignment) = task_result else {
                    continue; // Timeout or stop - check running flag
                };

                let TaskAssignment { uuid, content } = assignment;

                // Parse envelope and check for Kicked
                let envelope: serde_json::Value =
                    serde_json::from_str(&content).unwrap_or_default();
                if envelope.get("kind").and_then(|k| k.as_str()) == Some("Kicked") {
                    break;
                }

                // Extract inner content - content may be an object or string
                let inner_content = envelope.get("content").map_or_else(
                    || content.clone(),
                    |c| c.as_str().map_or_else(|| c.to_string(), String::from),
                );

                // Parse task kind
                let kind = serde_json::from_str::<serde_json::Value>(&inner_content)
                    .ok()
                    .and_then(|v| v.get("task")?.get("kind")?.as_str().map(String::from))
                    .unwrap_or_else(|| "Unknown".to_string());

                // Create response channel for this task
                let (tx, rx) = mpsc::channel::<String>();

                // Register task with controller
                {
                    let mut waiting = waiting_clone.lock().expect("waiting lock poisoned");
                    waiting.push(WaitingTask {
                        kind,
                        payload: inner_content.clone(),
                        response_tx: tx,
                    });
                }

                // Notify controller of arrival
                let _ = arrival_tx.send(());

                // Track for return value
                processed_tasks.push(inner_content);

                // Spawn thread to wait for completion and write response
                let pool_path_clone = pool_path.clone();
                thread::spawn(move || {
                    let response = rx.recv().unwrap_or_else(|_| "[]".to_string());
                    let _ = write_response(&pool_path_clone, &uuid, &response);
                });
            }

            processed_tasks
        });

        let agent = Self {
            running,
            handle: Some(handle),
            ordered_handles: None,
            pool_root: actual_pool_path,
        };

        let controller = OrderedAgentController {
            waiting,
            arrival_rx,
        };

        (agent, controller)
    }
}

// =============================================================================
// Ordered Agent Controller
// =============================================================================

/// A waiting task that hasn't been completed yet.
struct WaitingTask {
    /// The task kind (e.g., "Worker", "Analyze").
    kind: String,
    /// The full payload JSON.
    payload: String,
    /// Channel to send the response (send once, then drop).
    response_tx: Sender<String>,
}

/// Controller for completing tasks in any order.
///
/// Minimal API: wait for tasks, inspect them, complete by index.
pub struct OrderedAgentController {
    /// Tasks waiting for completion.
    waiting: Arc<Mutex<Vec<WaitingTask>>>,
    /// Channel that receives notifications when tasks arrive.
    arrival_rx: Receiver<()>,
}

impl OrderedAgentController {
    /// Block until at least `count` tasks are waiting.
    pub fn wait_for_tasks(&self, count: usize) {
        loop {
            {
                let waiting = self.waiting.lock().expect("waiting lock poisoned");
                if waiting.len() >= count {
                    return;
                }
            }
            // Block until a task arrives
            if self.arrival_rx.recv().is_err() {
                // Agent dropped - return with whatever we have
                return;
            }
        }
    }

    /// Get list of currently waiting tasks (kind, payload).
    pub fn waiting_tasks(&self) -> Vec<(String, String)> {
        let waiting = self.waiting.lock().expect("waiting lock poisoned");
        waiting
            .iter()
            .map(|t| (t.kind.clone(), t.payload.clone()))
            .collect()
    }

    /// Complete a specific waiting task by index.
    ///
    /// Panics if index is out of bounds.
    pub fn complete_at(&self, index: usize, response: &str) {
        let task = {
            let mut waiting = self.waiting.lock().expect("waiting lock poisoned");
            waiting.remove(index)
        };
        let _ = task.response_tx.send(response.to_string());
    }
}

// =============================================================================
// Troupe Handle
// =============================================================================

/// Find the `troupe` binary.
pub fn find_troupe_binary() -> PathBuf {
    if let Ok(bin) = std::env::var("TROUPE_BIN") {
        return PathBuf::from(bin);
    }

    // Find workspace root by looking for Cargo.toml with [workspace]
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("Could not find workspace root");

    workspace_root.join("target/debug/troupe")
}

/// Create an invoker for the troupe CLI using the test binary.
pub fn create_test_invoker() -> Invoker<TroupeCli> {
    Invoker::from_binary(find_troupe_binary())
}

/// Wrapper that starts the daemon via CLI subprocess.
///
/// Automatically shuts down the daemon when dropped.
pub struct TroupeHandle {
    /// The actual pool path (includes pools/ subdirectory)
    root: PathBuf,
    process: Option<Child>,
    /// Handles for threads forwarding stdout/stderr (so they get captured by tests)
    _output_threads: Vec<thread::JoinHandle<()>>,
}

impl TroupeHandle {
    /// Get the actual pool path (includes pools/ subdirectory).
    pub fn pool_path(&self) -> &Path {
        &self.root
    }
}

impl TroupeHandle {
    /// Start the troupe daemon.
    pub fn start(root: &Path) -> Self {
        let bin = find_troupe_binary();
        assert!(
            bin.exists(),
            "troupe binary not found at {}. Run `cargo build -p troupe_cli` first.",
            bin.display()
        );

        // The CLI adds pools/ between root and pool name.
        // If root is ".test-data/test_name":
        //   --root = ".test-data" (parent)
        //   --pool = "test_name" (basename)
        //   actual pool path = ".test-data/pools/test_name"
        let cli_root = root.parent().unwrap_or(root);
        let pool_name = root.file_name().unwrap_or_default();
        let actual_pool_path = cli_root.join("pools").join(pool_name);

        // Build command - use --root and pool name
        let mut cmd = Command::new(&bin);
        cmd.arg("start")
            .arg("--root")
            .arg(cli_root)
            .arg("--pool")
            .arg(pool_name)
            .arg("--log-level")
            .arg("trace")
            // No heartbeats needed - agents signal ready immediately
            .arg("--no-heartbeat")
            // Preserve directory tree on shutdown so FSEvents stays warm across test runs
            .arg("--preserve-dirs");

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut process = cmd.spawn().expect("Failed to spawn troupe process");

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
        // Watch the parent directory (cli_root) rather than the pool directory itself,
        // because on first run the pool directory may not exist yet (the daemon creates it).
        fs::create_dir_all(cli_root).expect("Failed to create pool root directory");
        let cli_root_buf = cli_root.to_path_buf();
        let mut watcher = VerifiedWatcher::new(cli_root, std::slice::from_ref(&cli_root_buf))
            .expect("Failed to create watcher");
        let status_path = actual_pool_path.join(STATUS_FILE);
        watcher
            .wait_for_file_with_timeout(&status_path, Duration::from_secs(10))
            .expect("Agent pool did not become ready in time");

        Self {
            root: actual_pool_path,
            process: Some(process),
            _output_threads: output_threads,
        }
    }
}

impl Drop for TroupeHandle {
    fn drop(&mut self) {
        // self.root is the actual pool path: <cli_root>/pools/<pool_name>
        // We need: --root <cli_root> --pool <pool_name>
        // So: --root = grandparent of self.root (skip pools/)
        let cli_root = self
            .root
            .parent() // <cli_root>/pools
            .and_then(|p| p.parent()) // <cli_root>
            .unwrap_or(&self.root);
        let pool_name = self.root.file_name().unwrap_or_default();

        // Try graceful shutdown via CLI
        let bin = find_troupe_binary();
        let _ = Command::new(&bin)
            .arg("stop")
            .arg("--root")
            .arg(cli_root)
            .arg("--pool")
            .arg(pool_name)
            .output();

        // Kill the process if still running
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}
