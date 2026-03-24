//! Shared test utilities for Barnum CLI integration tests.

#![allow(dead_code)]
#![expect(clippy::expect_used)]
#![expect(clippy::print_stderr)]

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
use troupe::{STATUS_FILE, TaskAssignment, VerifiedWatcher, wait_for_task, write_response};

/// Get the path to the test data directory for a given test file.
///
/// Uses `TEST_TMPDIR` env var when set (CI sets this to `/tmp/bt` to keep
/// paths short enough for the Unix socket `sun_path` 108-byte limit).
/// Falls back to `CARGO_MANIFEST_DIR/.td/` for local development.
fn test_data_dir(test_file: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("TEST_TMPDIR") {
        return PathBuf::from(dir).join(test_file);
    }
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
/// `FSEvents` to stay warm across test runs.
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
    ///
    /// The `pool_root` parameter follows the same convention as `TroupeHandle::start`:
    /// it's the logical pool path (e.g., `.test-data/test_name/pool`), which is decomposed
    /// into `--root` (parent) and `--pool` (basename). The CLI adds `pools/` internally.
    pub fn start(pool_root: &Path, output_dir: &Path, transitions: Vec<(String, String)>) -> Self {
        fs::create_dir_all(output_dir).expect("Failed to create output directory");

        // Compute actual pool path the same way TroupeHandle::start does.
        // The CLI adds pools/ between root and pool name.
        let cli_root = pool_root.parent().unwrap_or(pool_root);
        let pool_name = pool_root.file_name().unwrap_or_default();
        let actual_pool_path = cli_root.join("pools").join(pool_name);

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let output_dir = output_dir.to_path_buf();
        let pool_path_for_thread = actual_pool_path.clone();
        let handle = thread::spawn(move || {
            // Create watcher once for the thread
            let mut watcher = match VerifiedWatcher::new(
                &pool_path_for_thread,
                std::slice::from_ref(&pool_path_for_thread),
            ) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("Failed to create watcher: {e}");
                    return;
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

                    let _ = write_response(&pool_path_for_thread, &uuid, &response);
                    continue;
                }

                // Fallback: terminate
                let _ = write_response(&pool_path_for_thread, &uuid, "[]");
            }
        });

        Self {
            running,
            handle: Some(handle),
            pool_root: actual_pool_path,
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
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// =============================================================================
// Troupe Handle
// =============================================================================

fn find_troupe_binary() -> PathBuf {
    if let Ok(bin) = std::env::var("TROUPE_BIN") {
        return PathBuf::from(bin);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("Could not find workspace root");

    workspace_root.join("target/debug/troupe")
}

/// Wrapper that starts the daemon via CLI subprocess.
pub struct TroupeHandle {
    root: PathBuf,
    process: Option<Child>,
    _output_threads: Vec<thread::JoinHandle<()>>,
}

impl TroupeHandle {
    pub fn start(root: &Path) -> Self {
        let bin = find_troupe_binary();
        assert!(
            bin.exists(),
            "troupe binary not found at {}. Run `cargo build -p troupe_cli` first.",
            bin.display()
        );

        // The CLI adds pools/ between root and pool name.
        // If root is ".test-data/test_name/pool":
        //   --root = ".test-data/test_name"
        //   --pool = "pool"
        //   actual pool path = ".test-data/test_name/pools/pool"
        let cli_root = root.parent().unwrap_or(root);
        let pool_name = root.file_name().unwrap_or_default();
        let actual_pool_path = cli_root.join("pools").join(pool_name);

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
            .expect("Troupe did not become ready in time");

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

        let bin = find_troupe_binary();
        let _ = Command::new(&bin)
            .arg("stop")
            .arg("--root")
            .arg(cli_root)
            .arg("--pool")
            .arg(pool_name)
            .output();

        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

// =============================================================================
// Barnum CLI Handle
// =============================================================================

fn find_barnum_binary() -> PathBuf {
    if let Ok(bin) = std::env::var("BARNUM_BIN") {
        return PathBuf::from(bin);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("Could not find workspace root");

    workspace_root.join("target/debug/barnum")
}

/// Run the Barnum CLI with the given arguments.
pub struct BarnumRunner {
    bin: PathBuf,
}

impl BarnumRunner {
    pub fn new() -> Self {
        let bin = find_barnum_binary();
        assert!(
            bin.exists(),
            "barnum binary not found at {}. Run `cargo build -p barnum_cli` first.",
            bin.display()
        );
        Self { bin }
    }

    /// Run `barnum run` with the given config and initial tasks.
    pub fn run(&self, config: &str, initial_tasks: &str) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin)
            .arg("run")
            .arg("--config")
            .arg(config)
            .arg("--initial-state")
            .arg(initial_tasks)
            .output()
    }

    /// Run `barnum config validate` with the given config.
    pub fn validate(&self, config: &str) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin)
            .args(["config", "validate", "--config"])
            .arg(config)
            .output()
    }

    /// Run `barnum config docs` with the given config.
    pub fn docs(&self, config: &str) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin)
            .args(["config", "docs", "--config"])
            .arg(config)
            .output()
    }

    /// Run `barnum config graph` with the given config.
    pub fn graph(&self, config: &str) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin)
            .args(["config", "graph", "--config"])
            .arg(config)
            .output()
    }

    /// Run `barnum config schema` (defaults to Zod output).
    pub fn schema(&self) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin).args(["config", "schema"]).output()
    }

    /// Run `barnum config schema --type json`.
    pub fn schema_json(&self) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin)
            .args(["config", "schema", "--type", "json"])
            .output()
    }
}
