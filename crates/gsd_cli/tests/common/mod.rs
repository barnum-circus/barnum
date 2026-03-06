//! Shared test utilities for GSD CLI integration tests.

#![allow(dead_code)]
#![expect(clippy::expect_used)]
#![expect(clippy::print_stderr)]

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
// File Writer Agent
// =============================================================================

/// Extract task envelope kind and content.
fn extract_task_envelope(raw: &str) -> (String, String) {
    if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(raw) {
        let kind = envelope
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("Task")
            .to_string();

        let content = envelope
            .get("content")
            .map_or_else(|| raw.to_string(), serde_json::Value::to_string);

        return (kind, content);
    }
    ("Task".to_string(), raw.to_string())
}

/// A test agent that writes a marker file and terminates.
///
/// Each task processed writes to `{output_dir}/{step_name}.done` containing
/// the task data, allowing tests to verify which steps were executed.
pub struct FileWriterAgent {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    pool_root: PathBuf,
}

impl FileWriterAgent {
    /// Start a file writer agent.
    ///
    /// Uses the proper anonymous worker protocol:
    /// 1. Writes `<uuid>.ready.json` to signal availability
    /// 2. Waits for `<uuid>.task.json` using verified file watcher (no polling)
    /// 3. Processes task and writes marker file
    /// 4. Writes `<uuid>.response.json` with transition
    pub fn start(pool_root: &Path, output_dir: &Path, transitions: Vec<(String, String)>) -> Self {
        fs::create_dir_all(output_dir).expect("Failed to create output directory");

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let output_dir = output_dir.to_path_buf();
        let pool_root = pool_root.to_path_buf();
        let pool_root_for_stop = pool_root.clone();
        let handle = thread::spawn(move || {
            // Create watcher once for the thread
            let mut watcher =
                match VerifiedWatcher::new(&pool_root, std::slice::from_ref(&pool_root)) {
                    Ok(w) => w,
                    Err(e) => {
                        eprintln!("Failed to create watcher: {e}");
                        return;
                    }
                };

            while running_clone.load(Ordering::SeqCst) {
                // Wait for task with timeout - allows checking running flag periodically
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
                let (kind, task_content) = extract_task_envelope(&content);

                // Handle control messages
                if kind == "Kicked" {
                    break;
                }

                // Parse task to get step name
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&task_content)
                    && let Some(step_name) = parsed
                        .get("task")
                        .and_then(|t| t.get("kind"))
                        .and_then(|k| k.as_str())
                {
                    // Write marker file
                    let marker_file = output_dir.join(format!("{step_name}.done"));
                    let _ = fs::write(&marker_file, &task_content);

                    // Find transition
                    let response = transitions
                        .iter()
                        .find(|(from, _)| from == step_name)
                        .map_or_else(
                            || "[]".to_string(),
                            |(_, to)| {
                                if to.is_empty() {
                                    "[]".to_string()
                                } else {
                                    format!(r#"[{{"kind": "{to}", "value": {{}}}}]"#)
                                }
                            },
                        );

                    let _ = write_response(&pool_root, &uuid, &response);
                    continue;
                }

                // Fallback: terminate
                let _ = write_response(&pool_root, &uuid, "[]");
            }
        });

        Self {
            running,
            handle: Some(handle),
            pool_root: pool_root_for_stop,
        }
    }

    /// Stop the agent.
    ///
    /// Stops the daemon via CLI, which:
    /// 1. Cleans up the agents directory
    /// 2. Causes `wait_for_task` to fail with a watcher error
    /// 3. Agent threads exit
    pub fn stop(mut self) {
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
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// =============================================================================
// Agent Pool Handle
// =============================================================================

fn find_agent_pool_binary() -> PathBuf {
    if let Ok(bin) = std::env::var("AGENT_POOL_BIN") {
        return PathBuf::from(bin);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("Could not find workspace root");

    workspace_root.join("target/debug/agent_pool")
}

/// Wrapper that starts the daemon via CLI subprocess.
pub struct AgentPoolHandle {
    root: PathBuf,
    process: Option<Child>,
    _output_threads: Vec<thread::JoinHandle<()>>,
}

impl AgentPoolHandle {
    pub fn start(root: &Path) -> Self {
        let bin = find_agent_pool_binary();
        assert!(
            bin.exists(),
            "agent_pool binary not found at {}. Run `cargo build -p agent_pool_cli` first.",
            bin.display()
        );

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
        let bin = find_agent_pool_binary();
        let _ = Command::new(&bin)
            .arg("stop")
            .arg("--pool-root")
            .arg(self.root.parent().unwrap_or(&self.root))
            .arg("--pool")
            .arg(self.root.file_name().unwrap_or_default())
            .output();

        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

// =============================================================================
// GSD CLI Handle
// =============================================================================

fn find_gsd_binary() -> PathBuf {
    if let Ok(bin) = std::env::var("GSD_BIN") {
        return PathBuf::from(bin);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("Could not find workspace root");

    workspace_root.join("target/debug/gsd")
}

/// Run the GSD CLI with the given arguments.
pub struct GsdRunner {
    bin: PathBuf,
}

impl GsdRunner {
    pub fn new() -> Self {
        let bin = find_gsd_binary();
        assert!(
            bin.exists(),
            "gsd binary not found at {}. Run `cargo build -p gsd_cli` first.",
            bin.display()
        );
        Self { bin }
    }

    /// Run `gsd run` with the given config and initial tasks.
    pub fn run(
        &self,
        config: &str,
        initial_tasks: &str,
        root: &Path,
    ) -> std::io::Result<std::process::Output> {
        // root is the test dir, pool_id is derived from its name
        let pool_id = root.file_name().and_then(|s| s.to_str()).unwrap_or("pool");
        Command::new(&self.bin)
            .arg("--root")
            .arg(root)
            .arg("run")
            .arg(config)
            .arg("--initial-state")
            .arg(initial_tasks)
            .arg("--pool")
            .arg(pool_id)
            .output()
    }

    /// Run `gsd config validate` with the given config.
    pub fn validate(&self, config: &str) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin)
            .args(["config", "validate"])
            .arg(config)
            .output()
    }

    /// Run `gsd config docs` with the given config.
    pub fn docs(&self, config: &str) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin)
            .args(["config", "docs"])
            .arg(config)
            .output()
    }

    /// Run `gsd config graph` with the given config.
    pub fn graph(&self, config: &str) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin)
            .args(["config", "graph"])
            .arg(config)
            .output()
    }

    /// Run `gsd config schema`.
    pub fn schema(&self) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin).args(["config", "schema"]).output()
    }
}
