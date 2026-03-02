//! Shared test utilities for agent pool integration tests.

// Test utilities can be more relaxed
#![allow(dead_code)]
#![expect(clippy::expect_used)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::print_stderr)]

use agent_pool::{Response, default_pool_root, id_to_path, wait_for_pool_ready};
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

/// Generate a unique pool name for a test.
/// Format: `<test_name>_<uuid>` - ensures no conflicts between test runs.
pub fn generate_pool(test_name: &str) -> String {
    let uuid = &Uuid::new_v4().to_string()[..4];
    format!("{test_name}_{uuid}")
}

/// Short abbreviation for `DataSource` + `NotifyMethod` to keep pool names short.
/// Unix socket paths have a 104-byte limit on macOS.
pub fn mode_abbrev(data_source: DataSource, notify_method: NotifyMethod) -> &'static str {
    match (data_source, notify_method) {
        (DataSource::Inline, NotifyMethod::Socket) => "IS",
        (DataSource::Inline, NotifyMethod::File) => "IF",
        (DataSource::FileReference, NotifyMethod::Socket) => "FS",
        (DataSource::FileReference, NotifyMethod::File) => "FF",
    }
}

/// Get the path for a pool name.
/// Pools live in `/tmp/agent_pool/<pool>/`.
pub fn pool_path(pool: &str) -> PathBuf {
    id_to_path(&default_pool_root(), pool)
}

/// Clean up a pool by name.
pub fn cleanup_pool(pool: &str) {
    let dir = pool_path(pool);
    let _ = fs::remove_dir_all(&dir);
}

// =============================================================================
// Filesystem State Assertions
// =============================================================================

/// Snapshot of the agents directory structure.
///
/// Note: With the anonymous worker protocol, agents use flat files instead of
/// directories. This struct checks for stale directories to ensure clean state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentsSnapshot {
    /// Agent directories that exist (legacy - should be empty with anonymous workers)
    pub agent_dirs: BTreeSet<String>,
}

impl AgentsSnapshot {
    /// Take a snapshot of the agents directory.
    pub fn capture(pool: &str) -> Self {
        let agents_dir = pool_path(pool).join("agents");
        let mut agent_dirs = BTreeSet::new();

        if let Ok(entries) = fs::read_dir(&agents_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path
                        .file_name()
                        .expect("path has filename")
                        .to_string_lossy()
                        .to_string();
                    agent_dirs.insert(name);
                }
            }
        }

        Self { agent_dirs }
    }

    /// Assert no agent directories exist.
    pub fn assert_no_agents(&self) {
        assert!(
            self.agent_dirs.is_empty(),
            "Expected no agents, but found: {:?}",
            self.agent_dirs
        );
    }
}

/// Snapshot of the submissions directory structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmissionsSnapshot {
    /// Request files (*.request.json)
    pub request_files: BTreeSet<String>,
    /// Response files (*.response.json)
    pub response_files: BTreeSet<String>,
}

impl SubmissionsSnapshot {
    /// Take a snapshot of the submissions directory.
    pub fn capture(pool: &str) -> Self {
        let submissions_dir = pool_path(pool).join("submissions");
        let mut request_files = BTreeSet::new();
        let mut response_files = BTreeSet::new();

        if let Ok(entries) = fs::read_dir(&submissions_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".request.json") {
                    request_files.insert(name);
                } else if name.ends_with(".response.json") {
                    response_files.insert(name);
                }
            }
        }

        Self {
            request_files,
            response_files,
        }
    }

    /// Assert no pending submissions (no request files, no response files).
    pub fn assert_empty(&self) {
        assert!(
            self.request_files.is_empty() && self.response_files.is_empty(),
            "Expected no submissions, but found requests: {:?}, responses: {:?}",
            self.request_files,
            self.response_files
        );
    }
}

/// Check if Unix socket IPC is available.
///
/// Always returns true. Tests should be run outside the sandbox where IPC works.
/// If running in a restricted environment, set `SKIP_IPC_TESTS=1`.
pub fn is_ipc_available(_root: &std::path::Path) -> bool {
    std::env::var("SKIP_IPC_TESTS").is_err()
}

// =============================================================================
// Test Agent (CLI-based)
// =============================================================================

/// A test agent that uses the CLI to receive tasks from the daemon.
///
/// The agent runs in a background thread, calling `register` and `next_task`
/// CLI commands to interact with the daemon. This ensures tests exercise
/// the same code paths as real agents.
pub struct TestAgent {
    running: Arc<AtomicBool>,
    /// PID of current CLI subprocess (for killing on stop)
    current_pid: Arc<AtomicU32>,
    handle: Option<thread::JoinHandle<Vec<String>>>,
    /// Receiver that signals when the agent has registered (ready file written).
    ready_rx: Option<mpsc::Receiver<()>>,
    /// Test name for logging purposes
    #[allow(dead_code)]
    test_name: String,
    /// Pool name for deregistration on stop
    pool: String,
    /// Agent name for deregistration on stop
    agent_id: String,
    /// Ready file path (for cleanup on stop)
    ready_file: Arc<std::sync::Mutex<Option<PathBuf>>>,
}

impl TestAgent {
    /// Start a test agent with a custom processing function.
    ///
    /// The processor receives the task content (as JSON string) and agent ID,
    /// returning the response string.
    ///
    /// After starting, call `wait_ready()` to block until the agent has registered
    /// with the daemon and is ready to receive tasks.
    pub fn start<F>(
        pool: &str,
        agent_id: &str,
        processing_delay: Duration,
        processor: F,
        test_name: &str,
    ) -> Self
    where
        F: Fn(&str, &str) -> String + Send + 'static,
    {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let current_pid = Arc::new(AtomicU32::new(0));
        let current_pid_clone = current_pid.clone();
        let agent_id_owned = agent_id.to_string();
        let pool_owned = pool.to_string();
        let bin = find_agent_pool_binary();
        let test_name_owned = test_name.to_string();

        // Channel to signal when the agent has registered (ready file written)
        let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(0);

        // Shared storage for the ready file path (for cleanup on stop)
        let ready_file = Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let ready_file_clone = ready_file.clone();

        let handle = thread::spawn(move || {
            let mut processed_tasks = Vec::new();
            let mut last_response: Option<String> = None;
            let mut response_file_path: Option<String> = None;
            let mut first_iteration = true;

            loop {
                if !running_clone.load(Ordering::SeqCst) {
                    break;
                }

                // Build command: register for first call, next_task with response for subsequent
                let mut cmd = Command::new(&bin);
                if let Some(response) = last_response.take() {
                    let rf = response_file_path
                        .take()
                        .expect("response_file should be set");
                    cmd.arg("next_task")
                        .arg("--pool")
                        .arg(&pool_owned)
                        .arg("--response-file")
                        .arg(&rf)
                        .arg("--data")
                        .arg(&response);
                } else {
                    cmd.arg("register").arg("--pool").arg(&pool_owned);
                }
                if !agent_id_owned.is_empty() {
                    cmd.arg("--name").arg(&agent_id_owned);
                }
                cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

                let mut child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!(
                            "[{test_name_owned}] [agent {agent_id_owned}] Failed to spawn CLI: {e}"
                        );
                        break;
                    }
                };

                // Store PID for potential killing by stop()
                current_pid_clone.store(child.id(), Ordering::SeqCst);

                // Signal ready on first iteration after spawning register command.
                // The register command writes the ready file before waiting for tasks.
                // We need to wait for:
                // 1. CLI to write the ready file
                // 2. FSEvents to deliver the event
                // 3. Daemon to process the event and register the worker
                //
                // We detect a NEW ready file by comparing sets before and after.
                if first_iteration {
                    first_iteration = false;
                    let agents_dir = pool_path(&pool_owned).join("agents");

                    // Get existing ready files
                    let get_ready_files = || -> std::collections::HashSet<PathBuf> {
                        fs::read_dir(&agents_dir)
                            .map(|entries| {
                                entries
                                    .filter_map(Result::ok)
                                    .filter(|e| {
                                        e.path()
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .is_some_and(|n| n.ends_with(".ready.json"))
                                    })
                                    .map(|e| e.path())
                                    .collect()
                            })
                            .unwrap_or_default()
                    };

                    let initial_files = get_ready_files();

                    // Poll for a NEW ready file to appear
                    let start = std::time::Instant::now();
                    while start.elapsed() < Duration::from_secs(5) {
                        let current_files = get_ready_files();
                        let new_files: Vec<_> =
                            current_files.difference(&initial_files).cloned().collect();
                        if !new_files.is_empty() {
                            // Store the ready file path for cleanup
                            if let Ok(mut guard) = ready_file_clone.lock() {
                                *guard = Some(new_files[0].clone());
                            }
                            // Give daemon a moment to process the FSEvent
                            thread::sleep(Duration::from_millis(200));
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    let _ = ready_tx.send(());
                }

                // Forward stderr in background thread so it shows with --nocapture
                let stderr_agent_id = agent_id_owned.clone();
                let stderr_test_name = test_name_owned.clone();
                if let Some(stderr) = child.stderr.take() {
                    thread::spawn(move || {
                        let reader = BufReader::new(stderr);
                        for line in reader.lines().map_while(Result::ok) {
                            eprintln!(
                                "[{stderr_test_name}] [agent {stderr_agent_id} stderr] {line}"
                            );
                        }
                    });
                }

                // Wait for CLI to complete and collect stdout
                let output = match child.wait_with_output() {
                    Ok(o) => o,
                    Err(e) => {
                        eprintln!(
                            "[{test_name_owned}] [agent {agent_id_owned}] CLI process error: {e}"
                        );
                        break;
                    }
                };

                // Clear PID after process exits
                current_pid_clone.store(0, Ordering::SeqCst);

                // Check if we were killed (process killed = should exit)
                if !running_clone.load(Ordering::SeqCst) {
                    break;
                }

                // Check for non-zero exit (killed or error)
                if !output.status.success() {
                    eprintln!(
                        "[{test_name_owned}] [agent {agent_id_owned}] CLI exited with status: {}",
                        output.status
                    );
                    break;
                }

                // Parse task JSON from stdout (concatenate multi-line to single line for logging)
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stdout_oneline = stdout.replace('\n', "\\n").replace('\r', "");
                eprintln!("[{test_name_owned}] [agent {agent_id_owned} stdout] {stdout_oneline}");

                let task_json: serde_json::Value = match serde_json::from_str(&stdout) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!(
                            "[{test_name_owned}] [agent {agent_id_owned}] Failed to parse task JSON: {e}"
                        );
                        break;
                    }
                };

                let kind = task_json
                    .get("kind")
                    .and_then(|k| k.as_str())
                    .unwrap_or("Task");
                let content = task_json
                    .get("content")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);

                // Store response_file for next iteration
                if let Some(rf) = task_json.get("response_file").and_then(|v| v.as_str()) {
                    response_file_path = Some(rf.to_string());
                }

                // Handle control messages
                match kind {
                    "Heartbeat" => {
                        // TestAgent doesn't handle heartbeats by default.
                        // Tests shouldn't depend on heartbeats unless specifically testing that.
                        panic!(
                            "[{test_name_owned}] [agent {agent_id_owned}] \
                            Unexpected Heartbeat received! Tests should not depend on heartbeats."
                        );
                    }
                    "Kicked" => {
                        eprintln!(
                            "[{test_name_owned}] [agent {agent_id_owned}] Received Kicked, exiting"
                        );
                        break;
                    }
                    _ => {}
                }

                // Process task
                thread::sleep(processing_delay);
                let content_str = content.to_string();
                let response = processor(&content_str, &agent_id_owned);
                processed_tasks.push(content_str.trim().to_string());
                last_response = Some(response);
            }

            processed_tasks
        });

        Self {
            running,
            current_pid,
            handle: Some(handle),
            ready_rx: Some(ready_rx),
            test_name: test_name.to_string(),
            pool: pool.to_string(),
            agent_id: agent_id.to_string(),
            ready_file,
        }
    }

    /// Start a simple echo agent that appends " [processed]" to inputs.
    pub fn echo(pool: &str, agent_id: &str, processing_delay: Duration, test_name: &str) -> Self {
        Self::start(
            pool,
            agent_id,
            processing_delay,
            |task, _| format!("{} [processed]", task.trim()),
            test_name,
        )
    }

    /// Start a greeting agent that responds to "casual" and "formal" styles.
    ///
    /// Expects task content in format: `{"instructions":"...","data":"casual"|"formal"}`
    pub fn greeting(
        pool: &str,
        agent_id: &str,
        processing_delay: Duration,
        test_name: &str,
    ) -> Self {
        Self::start(
            pool,
            agent_id,
            processing_delay,
            |task, agent_id| {
                // Task content is JSON object with "data" field containing the style
                let task_json: serde_json::Value = match serde_json::from_str(task) {
                    Ok(v) => v,
                    Err(e) => return format!("Error: failed to parse task JSON: {e}"),
                };

                let style = task_json.get("data").and_then(|d| d.as_str()).unwrap_or("");

                match style {
                    "casual" => format!("Hi {agent_id}, how are ya?"),
                    "formal" => format!(
                        "Salutations {agent_id}, how are you doing on this most splendiferous and utterly magnificent day?"
                    ),
                    _ => format!("Error: unknown style '{style}' (use 'casual' or 'formal')"),
                }
            },
            test_name,
        )
    }

    /// Wait for the agent to be ready (registered with the daemon).
    ///
    /// This blocks until the agent has written its ready file and the daemon
    /// has processed the registration event.
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

        // Kill any running CLI subprocess
        let pid = self.current_pid.load(Ordering::SeqCst);
        if pid != 0 {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
        }

        // Clean up anonymous worker files so daemon removes the worker
        if let Ok(guard) = self.ready_file.lock()
            && let Some(ref ready_path) = *guard
            && let Some(uuid) = ready_path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".ready.json"))
        {
            let agents_dir = ready_path.parent().expect("ready file has parent");
            // Remove all files for this UUID
            let _ = fs::remove_file(ready_path);
            let _ = fs::remove_file(agents_dir.join(format!("{uuid}.task.json")));
            let _ = fs::remove_file(agents_dir.join(format!("{uuid}.response.json")));
        }

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
///
/// Checks in order:
/// 1. `AGENT_POOL_BIN` environment variable
/// 2. `target/debug/agent_pool` relative to workspace root
fn find_agent_pool_binary() -> PathBuf {
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

/// Submit a task via the CLI.
///
/// Executes: `agent_pool submit_task --pool <pool> --data <payload_json> --notify <method>`
pub fn submit_via_cli(pool: &str, payload_json: &str, notify: &str) -> io::Result<Response> {
    let bin = find_agent_pool_binary();

    let output = Command::new(&bin)
        .arg("submit_task")
        .arg("--pool")
        .arg(pool)
        .arg("--data")
        .arg(payload_json)
        .arg("--notify")
        .arg(notify)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!("CLI failed: {stderr}")));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(io::Error::other)
}

/// How the task content is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSource {
    /// Content is inline in the JSON/command (`--data` or `Inline` in task.json)
    Inline,
    /// Content is in a separate file (`--file` or `FileReference` in task.json)
    FileReference,
}

/// How to submit and wait for response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyMethod {
    /// CLI with `--notify socket`
    Socket,
    /// CLI with `--notify file`
    File,
}

/// Test timeout for file-based submissions.
/// Set higher than rstest timeout to ensure file notification isn't the bottleneck.
const TEST_FILE_TIMEOUT_SECS: u64 = 60;

/// Submit a task using the specified data source and notify method.
///
/// This is the cross-product of `DataSource` × `NotifyMethod` = 4 combinations.
pub fn submit_with_mode(
    pool: &str,
    payload_json: &str,
    data_source: DataSource,
    notify_method: NotifyMethod,
) -> io::Result<Response> {
    let bin = find_agent_pool_binary();
    let mut cmd = Command::new(&bin);

    cmd.arg("submit_task").arg("--pool").arg(pool);

    // Set up data source
    let _temp_file;
    match data_source {
        DataSource::Inline => {
            cmd.arg("--data").arg(payload_json);
        }
        DataSource::FileReference => {
            let mut temp = tempfile::NamedTempFile::new()?;
            std::io::Write::write_all(&mut temp, payload_json.as_bytes())?;
            cmd.arg("--file").arg(temp.path());
            _temp_file = temp; // Keep alive until command completes
        }
    }

    // Set up notify method
    match notify_method {
        NotifyMethod::Socket => cmd.arg("--notify").arg("socket"),
        NotifyMethod::File => cmd
            .arg("--notify")
            .arg("file")
            .arg("--timeout-secs")
            .arg(TEST_FILE_TIMEOUT_SECS.to_string()),
    };

    let output = cmd.output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "CLI failed (data={data_source:?}, notify={notify_method:?}): {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(io::Error::other)
}

/// Configuration for the daemon when starting via CLI.
#[derive(Debug, Clone, Default)]
pub struct DaemonConfig {
    /// How long an idle worker can wait before receiving a heartbeat.
    pub idle_timeout: Duration,
    /// Default timeout for tasks.
    pub default_task_timeout: Duration,
    /// Whether to send periodic heartbeats to idle workers.
    pub heartbeat_enabled: bool,
}

impl DaemonConfig {
    /// Create a new config with test-appropriate defaults.
    ///
    /// Heartbeats are disabled by default - tests should not depend on them
    /// unless specifically testing heartbeat behavior.
    pub fn new() -> Self {
        Self {
            idle_timeout: Duration::from_secs(60),
            default_task_timeout: Duration::from_secs(30),
            // Heartbeats disabled - tests should not rely on them
            heartbeat_enabled: false,
        }
    }
}

impl From<agent_pool::DaemonConfig> for DaemonConfig {
    fn from(config: agent_pool::DaemonConfig) -> Self {
        Self {
            idle_timeout: config.idle_timeout,
            default_task_timeout: config.default_task_timeout,
            heartbeat_enabled: config.heartbeat_enabled,
        }
    }
}

/// Wrapper that starts the daemon via CLI subprocess.
///
/// Automatically shuts down the daemon when dropped.
pub struct AgentPoolHandle {
    pool: String,
    process: Option<Child>,
    /// Handles for threads forwarding stdout/stderr (so they get captured by tests)
    _output_threads: Vec<thread::JoinHandle<()>>,
    /// Test name for logging purposes
    test_name: String,
}

impl AgentPoolHandle {
    /// Start the agent pool daemon with default configuration.
    pub fn start(pool: &str, test_name: &str) -> Self {
        Self::start_with_config(pool, DaemonConfig::new(), test_name)
    }

    /// Start the agent pool daemon with custom configuration.
    pub fn start_with_config(pool: &str, config: DaemonConfig, test_name: &str) -> Self {
        let root = pool_path(pool);
        let bin = find_agent_pool_binary();
        assert!(
            bin.exists(),
            "agent_pool binary not found at {}. Run `cargo build -p agent_pool_cli` first.",
            bin.display()
        );

        // Build command
        let mut cmd = Command::new(&bin);
        cmd.arg("start")
            .arg("--pool")
            .arg(pool)
            .arg("--log-level")
            .arg("trace")
            .arg("--idle-timeout-secs")
            .arg(config.idle_timeout.as_secs().to_string())
            .arg("--task-timeout-secs")
            .arg(config.default_task_timeout.as_secs().to_string());

        if !config.heartbeat_enabled {
            cmd.arg("--no-heartbeat");
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut process = cmd.spawn().expect("Failed to spawn agent_pool process");

        // Set up output capture before waiting so we capture logs even if startup fails
        let mut output_threads = Vec::new();
        let test_name_stdout = test_name.to_string();
        let test_name_stderr = test_name.to_string();

        if let Some(stdout) = process.stdout.take() {
            output_threads.push(thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[{test_name_stdout}] [daemon stdout] {line}");
                }
            }));
        }

        if let Some(stderr) = process.stderr.take() {
            output_threads.push(thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[{test_name_stderr}] [daemon stderr] {line}");
                }
            }));
        }

        wait_for_pool_ready(&root, Duration::from_secs(10))
            .expect("Agent pool did not become ready in time");

        Self {
            pool: pool.to_string(),
            process: Some(process),
            _output_threads: output_threads,
            test_name: test_name.to_string(),
        }
    }
}

impl Drop for AgentPoolHandle {
    fn drop(&mut self) {
        let test_name = &self.test_name;
        eprintln!("[{test_name}] [pool drop] Starting graceful shutdown...");
        // Try graceful shutdown via CLI
        let bin = find_agent_pool_binary();
        let _ = Command::new(&bin)
            .arg("stop")
            .arg("--pool")
            .arg(&self.pool)
            .output();

        eprintln!(
            "[{test_name}] [pool drop] Graceful shutdown complete, killing process if needed..."
        );
        // Kill the process if still running
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
        eprintln!("[{test_name}] [pool drop] Pool drop complete");
    }
}
