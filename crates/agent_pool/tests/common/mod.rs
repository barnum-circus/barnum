//! Shared test utilities for agent pool integration tests.

// Test utilities can be more relaxed
#![allow(dead_code)]
#![expect(clippy::expect_used)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::print_stderr)]

use agent_pool::{Response, wait_for_pool_ready};
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    use std::os::unix::net::{UnixListener, UnixStream};

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
    /// Receiver that signals when the agent has processed its first message (heartbeat).
    ready_rx: Option<mpsc::Receiver<()>>,
    /// Test name for logging purposes
    #[allow(dead_code)]
    test_name: String,
    /// Pool root for deregistration on stop
    root: PathBuf,
    /// Agent name for deregistration on stop
    agent_id: String,
}

impl TestAgent {
    /// Start a test agent with a custom processing function.
    ///
    /// The processor receives the task content (as JSON string) and agent ID,
    /// returning the response string.
    ///
    /// After starting, call `wait_ready()` to block until the agent has processed
    /// its first message (heartbeat) and is ready to receive real tasks.
    pub fn start<F>(
        root: &Path,
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
        let root_owned = root.to_path_buf();
        let bin = find_agent_pool_binary();
        let test_name_owned = test_name.to_string();

        // Channel to signal when the agent has processed its first message (heartbeat)
        let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(0);

        let handle = thread::spawn(move || {
            let mut processed_tasks = Vec::new();
            let mut first_message_processed = false;
            let mut last_response: Option<String> = None;

            loop {
                if !running_clone.load(Ordering::SeqCst) {
                    break;
                }

                // Build command: register for first call, next_task with response for subsequent
                let mut cmd = Command::new(&bin);
                if let Some(response) = last_response.take() {
                    cmd.arg("next_task")
                        .arg("--pool")
                        .arg(&root_owned)
                        .arg("--name")
                        .arg(&agent_id_owned)
                        .arg("--data")
                        .arg(&response);
                } else {
                    cmd.arg("register")
                        .arg("--pool")
                        .arg(&root_owned)
                        .arg("--name")
                        .arg(&agent_id_owned);
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

                // Handle control messages
                match kind {
                    "Heartbeat" => {
                        last_response = Some("{}".to_string());
                        if !first_message_processed {
                            first_message_processed = true;
                            let _ = ready_tx.send(());
                        }
                        continue;
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

                if !first_message_processed {
                    first_message_processed = true;
                    let _ = ready_tx.send(());
                }
            }

            processed_tasks
        });

        Self {
            running,
            current_pid,
            handle: Some(handle),
            ready_rx: Some(ready_rx),
            test_name: test_name.to_string(),
            root: root.to_path_buf(),
            agent_id: agent_id.to_string(),
        }
    }

    /// Start a simple echo agent that appends " [processed]" to inputs.
    pub fn echo(root: &Path, agent_id: &str, processing_delay: Duration, test_name: &str) -> Self {
        Self::start(
            root,
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
        root: &Path,
        agent_id: &str,
        processing_delay: Duration,
        test_name: &str,
    ) -> Self {
        Self::start(
            root,
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
        // Use deregister_agent CLI which writes a Kicked message, then removes the directory.
        // This makes the CLI exit cleanly.
        let bin = find_agent_pool_binary();
        let _ = Command::new(&bin)
            .arg("deregister_agent")
            .arg("--pool")
            .arg(&self.root)
            .arg("--name")
            .arg(&self.agent_id)
            .output();

        self.running.store(false, Ordering::SeqCst);

        // Kill any running CLI subprocess (in case it didn't see the Kicked message)
        let pid = self.current_pid.load(Ordering::SeqCst);
        if pid != 0 {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
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
/// Executes: `agent_pool submit_task --pool <root> --data <payload_json> --notify <method>`
pub fn submit_via_cli(root: &Path, payload_json: &str, notify: &str) -> io::Result<Response> {
    let bin = find_agent_pool_binary();

    let output = Command::new(&bin)
        .arg("submit_task")
        .arg("--pool")
        .arg(root)
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
    /// Raw file protocol: direct write to `pending/`, wait with notify
    Raw,
}

/// Test timeout for file-based submissions (20 seconds).
const TEST_FILE_TIMEOUT_SECS: u64 = 20;

/// Submit a task using the specified data source and notify method.
///
/// This is the cross-product of `DataSource` × `NotifyMethod` = 6 combinations.
pub fn submit_with_mode(
    root: &Path,
    payload_json: &str,
    data_source: DataSource,
    notify_method: NotifyMethod,
) -> io::Result<Response> {
    // Raw mode bypasses CLI entirely
    if notify_method == NotifyMethod::Raw {
        return submit_raw(root, payload_json, data_source);
    }

    let bin = find_agent_pool_binary();
    let mut cmd = Command::new(&bin);

    cmd.arg("submit_task").arg("--pool").arg(root);

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
        NotifyMethod::Raw => unreachable!("handled above"),
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

/// Submit a task using raw file protocol (direct call to `submit_file` library function).
///
/// This bypasses the CLI and calls the production `submit_file` function directly.
fn submit_raw(root: &Path, payload_json: &str, data_source: DataSource) -> io::Result<Response> {
    // Build the payload, keeping any temp file alive until submit completes
    let _temp_file;
    let payload = match data_source {
        DataSource::Inline => agent_pool::Payload::inline(payload_json),
        DataSource::FileReference => {
            let temp = tempfile::NamedTempFile::new()?;
            fs::write(temp.path(), payload_json)?;
            let payload = agent_pool::Payload::file_ref(temp.path());
            _temp_file = temp; // Keep alive
            payload
        }
    };

    agent_pool::submit_file(root, &payload)
}

/// Configuration for the daemon when starting via CLI.
#[derive(Debug, Clone, Default)]
pub struct DaemonConfig {
    /// How long an idle agent can wait before being deregistered.
    pub idle_agent_timeout: Duration,
    /// Default timeout for tasks.
    pub default_task_timeout: Duration,
    /// Whether to send an immediate heartbeat when an agent connects.
    pub immediate_heartbeat_enabled: bool,
    /// Whether to send periodic heartbeats after idle timeout.
    pub periodic_heartbeat_enabled: bool,
}

impl DaemonConfig {
    /// Create a new config with default values matching the CLI defaults.
    pub fn new() -> Self {
        Self {
            idle_agent_timeout: Duration::from_secs(60),
            default_task_timeout: Duration::from_secs(300),
            immediate_heartbeat_enabled: true,
            periodic_heartbeat_enabled: true,
        }
    }
}

impl From<agent_pool::DaemonConfig> for DaemonConfig {
    fn from(config: agent_pool::DaemonConfig) -> Self {
        Self {
            idle_agent_timeout: config.idle_agent_timeout,
            default_task_timeout: config.default_task_timeout,
            immediate_heartbeat_enabled: config.immediate_heartbeat_enabled,
            periodic_heartbeat_enabled: config.periodic_heartbeat_enabled,
        }
    }
}

/// Wrapper that starts the daemon via CLI subprocess.
///
/// Automatically shuts down the daemon when dropped.
pub struct AgentPoolHandle {
    root: PathBuf,
    process: Option<Child>,
    /// Handles for threads forwarding stdout/stderr (so they get captured by tests)
    _output_threads: Vec<thread::JoinHandle<()>>,
    /// Test name for logging purposes
    test_name: String,
}

impl AgentPoolHandle {
    /// Start the agent pool daemon with default configuration.
    pub fn start(root: &Path, test_name: &str) -> Self {
        Self::start_with_config(root, DaemonConfig::new(), test_name)
    }

    /// Start the agent pool daemon with custom configuration.
    pub fn start_with_config(root: &Path, config: DaemonConfig, test_name: &str) -> Self {
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
            .arg(root)
            .arg("--log-level")
            .arg("trace")
            .arg("--idle-agent-timeout-secs")
            .arg(config.idle_agent_timeout.as_secs().to_string())
            .arg("--task-timeout-secs")
            .arg(config.default_task_timeout.as_secs().to_string());

        if !config.immediate_heartbeat_enabled && !config.periodic_heartbeat_enabled {
            cmd.arg("--no-heartbeat");
        } else if !config.immediate_heartbeat_enabled {
            cmd.arg("--no-immediate-heartbeat");
        } else if !config.periodic_heartbeat_enabled {
            cmd.arg("--no-periodic-heartbeat");
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

        wait_for_pool_ready(root, Duration::from_secs(10))
            .expect("Agent pool did not become ready in time");

        Self {
            root: root.to_path_buf(),
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
            .arg(&self.root)
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
