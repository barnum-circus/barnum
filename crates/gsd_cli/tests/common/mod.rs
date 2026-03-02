//! Shared test utilities for GSD CLI integration tests.

#![allow(dead_code)]
#![expect(clippy::expect_used)]

use agent_pool::{AGENTS_DIR, RESPONSE_FILE, TASK_FILE, wait_for_pool_ready};
use std::fs;
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
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

/// Parsed task envelope.
struct TaskEnvelope {
    kind: String,
    content: String,
}

/// Extract task kind and content from the envelope format.
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
    TaskEnvelope {
        kind: "Task".to_string(),
        content: raw.to_string(),
    }
}

/// A test agent that writes a marker file and terminates.
///
/// Each task processed writes to `{output_dir}/{step_name}.done` containing
/// the task data, allowing tests to verify which steps were executed.
pub struct FileWriterAgent {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    ready_rx: Option<mpsc::Receiver<()>>,
}

impl FileWriterAgent {
    /// Start a file writer agent.
    ///
    /// For each task, it:
    /// 1. Extracts the step name from `task.kind`
    /// 2. Writes `{output_dir}/{step_name}.done` with the task data
    /// 3. Returns the configured transition (or terminates)
    pub fn start(
        pool_root: &Path,
        agent_id: &str,
        output_dir: &Path,
        transitions: Vec<(String, String)>,
    ) -> Self {
        let agent_dir = pool_root.join(AGENTS_DIR).join(agent_id);
        fs::create_dir_all(&agent_dir).expect("Failed to create agent directory");
        fs::create_dir_all(output_dir).expect("Failed to create output directory");

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let output_dir = output_dir.to_path_buf();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(0);

        let handle = thread::spawn(move || {
            let task_file = agent_dir.join(TASK_FILE);
            let response_file = agent_dir.join(RESPONSE_FILE);
            let mut first_message_processed = false;

            while running_clone.load(Ordering::SeqCst) {
                if task_file.exists() && !response_file.exists() {
                    let Ok(raw) = fs::read_to_string(&task_file) else {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    };

                    if raw.is_empty() {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }

                    let envelope = extract_task_envelope(&raw);

                    match envelope.kind.as_str() {
                        "Heartbeat" => {
                            let _ = fs::write(&response_file, "{}");
                            if !first_message_processed {
                                first_message_processed = true;
                                let _ = ready_tx.send(());
                            }
                            thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        "Kicked" => break,
                        _ => {}
                    }

                    // Parse the task to get the step name and write marker file
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&envelope.content)
                        && let Some(step_name) = parsed
                            .get("task")
                            .and_then(|t| t.get("kind"))
                            .and_then(|k| k.as_str())
                    {
                        // Write marker file
                        let marker_file = output_dir.join(format!("{step_name}.done"));
                        let _ = fs::write(&marker_file, &envelope.content);

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

                        let _ = fs::write(&response_file, &response);

                        if !first_message_processed {
                            first_message_processed = true;
                            let _ = ready_tx.send(());
                        }

                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }

                    // Fallback: terminate
                    let _ = fs::write(&response_file, "[]");
                }

                thread::sleep(Duration::from_millis(10));
            }
        });

        Self {
            running,
            handle: Some(handle),
            ready_rx: Some(ready_rx),
        }
    }

    /// Wait for the agent to be ready.
    pub fn wait_ready(&mut self) {
        if let Some(rx) = self.ready_rx.take() {
            rx.recv().expect("Agent exited before signaling readiness");
        }
    }

    /// Stop the agent.
    pub fn stop(mut self) {
        self.running.store(false, Ordering::SeqCst);
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
            .arg("trace");

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

        wait_for_pool_ready(root, Duration::from_secs(10))
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
        pool_root: &Path,
    ) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin)
            .arg("run")
            .arg(config)
            .arg("--initial")
            .arg(initial_tasks)
            .arg("--pool")
            .arg(pool_root)
            .output()
    }

    /// Run `gsd validate` with the given config.
    pub fn validate(&self, config: &str) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin).arg("validate").arg(config).output()
    }

    /// Run `gsd docs` with the given config.
    pub fn docs(&self, config: &str) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin).arg("docs").arg(config).output()
    }

    /// Run `gsd graph` with the given config.
    pub fn graph(&self, config: &str) -> std::io::Result<std::process::Output> {
        Command::new(&self.bin).arg("graph").arg(config).output()
    }
}
