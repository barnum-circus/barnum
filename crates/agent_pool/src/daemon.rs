//! Agent pool daemon - dispatches tasks to available agents.
//!
//! The pool watches a directory for agents and dispatches incoming tasks
//! to whichever agent is available. Each agent is a subdirectory that
//! processes tasks via the file protocol (`task.json` → `response.json`).
//!
//! # Usage
//!
//! For CLI tools that run forever:
//! ```ignore
//! daemon::run(&root)?;  // Never returns on success
//! ```
//!
//! For programmatic control with graceful shutdown:
//! ```ignore
//! let handle = daemon::spawn(&root)?;
//! // ... submit tasks ...
//! handle.shutdown()?;  // Gracefully stops the daemon
//! ```

use crate::constants::{
    AGENTS_DIR, LOCK_FILE, PENDING_DIR, RESPONSE_FILE, SOCKET_NAME, TASK_FILE,
};
use crate::lock::acquire_lock;
use crate::response::Response;
use crate::submit_file::{PENDING_RESPONSE_FILE, PENDING_TASK_FILE};
use interprocess::local_socket::{
    GenericFilePath, Listener, ListenerNonblockingMode, ListenerOptions, Stream, prelude::*,
};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use std::{fs, io, thread};
use tracing::{debug, info, trace, warn};

// =============================================================================
// Public API
// =============================================================================

/// Shared control signals for the daemon.
#[derive(Clone)]
struct DaemonSignals {
    shutdown: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
}

impl DaemonSignals {
    fn new() -> Self {
        Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
        }
    }

    fn trigger_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    fn is_shutdown_triggered(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

/// Handle to a running daemon, allowing control and graceful shutdown.
pub struct DaemonHandle {
    signals: DaemonSignals,
    thread: Option<thread::JoinHandle<io::Result<()>>>,
}

impl DaemonHandle {
    /// Pause task dispatching.
    pub fn pause(&self) {
        self.signals.set_paused(true);
    }

    /// Resume task dispatching after a pause.
    pub fn resume(&self) {
        self.signals.set_paused(false);
    }

    /// Check if the daemon is currently paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.signals.is_paused()
    }

    /// Request graceful shutdown and wait for the daemon to stop.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon thread panicked or encountered an I/O error.
    pub fn shutdown(mut self) -> io::Result<()> {
        self.signals.trigger_shutdown();
        self.join()
    }

    fn join(&mut self) -> io::Result<()> {
        if let Some(handle) = self.thread.take() {
            handle
                .join()
                .map_err(|_| io::Error::other("daemon thread panicked"))?
        } else {
            Ok(())
        }
    }
}

/// Spawn the daemon in a background thread.
///
/// # Errors
///
/// Returns an error if the lock can't be acquired or setup fails.
pub fn spawn(root: impl AsRef<Path>) -> io::Result<DaemonHandle> {
    let root = root.as_ref().to_path_buf();

    fs::create_dir_all(&root)?;

    let lock_path = root.join(LOCK_FILE);
    let lock = acquire_lock(&lock_path)?;

    let agents_dir = root.join(AGENTS_DIR);
    fs::create_dir_all(&agents_dir)?;

    let socket_path = root.join(SOCKET_NAME);
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }

    let listener = create_listener(&socket_path)?;
    let (watcher, fs_events) = create_watcher(&agents_dir)?;

    let signals = DaemonSignals::new();
    let signals_clone = signals.clone();

    // Create pending directory for file-based submissions
    let pending_dir = root.join(PENDING_DIR);
    fs::create_dir_all(&pending_dir)?;

    let thread = thread::spawn(move || {
        let _lock = lock;
        let _cleanup = SocketCleanup(socket_path.clone());
        let _watcher = watcher;

        info!(socket = %socket_path.display(), "listening");

        let mut state = PoolState::new(&root);
        state.scan_agents()?;

        event_loop(&listener, &fs_events, &mut state, &signals_clone)
    });

    thread::sleep(Duration::from_millis(50));

    Ok(DaemonHandle {
        signals,
        thread: Some(thread),
    })
}

/// Run the agent pool daemon (blocking, never returns on success).
///
/// # Errors
///
/// Returns an error if the lock can't be acquired or an I/O error occurs.
pub fn run(root: impl AsRef<Path>) -> io::Result<Infallible> {
    let root = root.as_ref().to_path_buf();

    fs::create_dir_all(&root)?;

    let lock_path = root.join(LOCK_FILE);
    let _lock = acquire_lock(&lock_path)?;

    let agents_dir = root.join(AGENTS_DIR);
    fs::create_dir_all(&agents_dir)?;

    // Create pending directory for file-based submissions
    let pending_dir = root.join(PENDING_DIR);
    fs::create_dir_all(&pending_dir)?;

    let socket_path = root.join(SOCKET_NAME);
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }

    let _cleanup = SocketCleanup(socket_path.clone());

    let listener = create_listener(&socket_path)?;
    let (watcher, fs_events) = create_watcher(&agents_dir)?;
    let _watcher = watcher;

    info!(socket = %socket_path.display(), "listening");

    let mut state = PoolState::new(&root);
    state.scan_agents()?;

    let signals = DaemonSignals::new();
    match event_loop(&listener, &fs_events, &mut state, &signals) {
        Ok(()) => unreachable!("event loop returned without shutdown signal"),
        Err(e) => Err(e),
    }
}

// =============================================================================
// Pool State
// =============================================================================

/// Where to send the response when a task completes.
enum ResponseTarget {
    /// Send via Unix socket (standard mode).
    Socket(Stream),
    /// Write to file (for sandboxed environments).
    File(PathBuf),
}

/// The current status of an agent.
enum AgentStatus {
    /// Agent is idle, available for work.
    Idle,
    /// Agent is busy with an in-flight task.
    Busy(InFlight),
}

/// What the agent is currently working on.
enum InFlight {
    /// A real task from a submitter.
    Task { respond_to: ResponseTarget },
}

impl InFlight {
    /// Complete the in-flight work, consuming self.
    ///
    /// For tasks, this sends the response to the submitter.
    fn complete(self, output: String) -> io::Result<()> {
        match self {
            InFlight::Task { respond_to } => {
                let response = Response::processed(output);
                send_response(respond_to, &response)
            }
        }
    }
}

/// State for a single agent.
struct AgentState {
    status: AgentStatus,
}

impl AgentState {
    const fn new() -> Self {
        Self { status: AgentStatus::Idle }
    }

    const fn is_idle(&self) -> bool {
        matches!(self.status, AgentStatus::Idle)
    }
}

/// Runtime state of the agent pool.
struct PoolState {
    agents_dir: PathBuf,
    pending_dir: PathBuf,
    agents: HashMap<String, AgentState>,
    pending: VecDeque<Task>,
}

struct Task {
    content: String,
    respond_to: ResponseTarget,
}

impl PoolState {
    fn new(root: &Path) -> Self {
        let agents_dir = root.join(AGENTS_DIR);
        let pending_dir = root.join(PENDING_DIR);
        Self {
            agents_dir,
            pending_dir,
            agents: HashMap::new(),
            pending: VecDeque::new(),
        }
    }

    fn scan_agents(&mut self) -> io::Result<()> {
        // Collect current directories
        let mut current_dirs = std::collections::HashSet::new();
        for entry in fs::read_dir(&self.agents_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                current_dirs.insert(name.to_string());
                self.register(name);
            }
        }

        // Remove agents whose directories no longer exist
        let stale: Vec<_> = self
            .agents
            .keys()
            .filter(|id| !current_dirs.contains(*id))
            .cloned()
            .collect();
        for id in stale {
            debug!(agent_id = %id, "removing stale agent during scan");
            self.unregister(&id);
        }

        Ok(())
    }

    /// Scan the pending directory for file-based submissions.
    fn scan_pending(&mut self) -> io::Result<()> {
        if !self.pending_dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(&self.pending_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let task_path = entry.path().join(PENDING_TASK_FILE);
            let response_path = entry.path().join(PENDING_RESPONSE_FILE);

            // Only process if task.json exists and response.json doesn't
            if task_path.exists() && !response_path.exists() {
                // Read task content
                let content = match fs::read_to_string(&task_path) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(path = %task_path.display(), error = %e, "failed to read pending task");
                        continue;
                    }
                };

                // Delete task.json to mark as picked up
                if let Err(e) = fs::remove_file(&task_path) {
                    warn!(path = %task_path.display(), error = %e, "failed to remove pending task file");
                    continue;
                }

                info!(submission = %entry.file_name().to_string_lossy(), "file-based task received");

                let task = Task {
                    content,
                    respond_to: ResponseTarget::File(response_path),
                };
                self.pending.push_back(task);
            }
        }

        Ok(())
    }

    fn in_flight_count(&self) -> usize {
        self.agents
            .values()
            .filter(|a| !a.is_idle())
            .count()
    }

    fn scan_outputs(&mut self) -> io::Result<()> {
        let busy: Vec<_> = self
            .agents
            .iter()
            .filter(|(_, a)| !a.is_idle())
            .map(|(id, _)| id.clone())
            .collect();

        for agent_id in busy {
            let response_path = self.agents_dir.join(&agent_id).join(RESPONSE_FILE);
            // Check if response file exists - use metadata to get better error info
            let exists = match fs::metadata(&response_path) {
                Ok(_) => true,
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Err(e) => {
                    return Err(io::Error::new(
                        e.kind(),
                        format!("failed to check response file {}: {e}", response_path.display()),
                    ));
                }
            };
            if exists {
                self.complete_task(&agent_id, &response_path)
                    .map_err(|e| io::Error::new(e.kind(), format!("complete_task for {agent_id} failed: {e}")))?;
            }
        }
        Ok(())
    }

    fn register(&mut self, agent_id: &str) {
        if !self.agents.contains_key(agent_id) {
            info!(agent_id, "agent registered");
            self.agents.insert(agent_id.to_string(), AgentState::new());
        }
    }

    fn unregister(&mut self, agent_id: &str) {
        if self.agents.remove(agent_id).is_some() {
            info!(agent_id, "agent unregistered");
        }
    }

    fn enqueue(&mut self, task: Task) {
        info!(
            bytes = task.content.len(),
            pending = self.pending.len(),
            agents = self.agents.len(),
            "task received"
        );
        debug!(content = %task.content, "task content");
        self.pending.push_back(task);
    }

    fn dispatch_pending(&mut self) -> io::Result<()> {
        while let Some(agent_id) = self.find_available_agent() {
            let Some(task) = self.pending.pop_front() else {
                break;
            };
            self.dispatch_to(&agent_id, task)?;
        }
        Ok(())
    }

    fn find_available_agent(&mut self) -> Option<String> {
        // Find an idle agent with a valid directory
        let candidate = self
            .agents
            .iter()
            .find(|(id, a)| a.is_idle() && self.agents_dir.join(id).is_dir())
            .map(|(id, _)| id.clone());

        // Clean up any stale agents (idle but directory missing)
        if candidate.is_none() {
            let stale: Vec<_> = self
                .agents
                .iter()
                .filter(|(id, a)| a.is_idle() && !self.agents_dir.join(id).is_dir())
                .map(|(id, _)| id.clone())
                .collect();
            for id in stale {
                warn!(agent_id = %id, "removing stale agent (directory missing)");
                self.agents.remove(&id);
            }
        }

        candidate
    }

    fn dispatch_to(&mut self, agent_id: &str, task: Task) -> io::Result<()> {
        let Some(agent) = self.agents.get_mut(agent_id) else {
            return Err(io::Error::other("agent not found"));
        };

        // Wrap task content in envelope with kind
        let envelope = serde_json::json!({
            "kind": "Task",
            "content": serde_json::from_str::<serde_json::Value>(&task.content)
                .unwrap_or_else(|_| serde_json::Value::String(task.content.clone()))
        });

        let task_path = self.agents_dir.join(agent_id).join(TASK_FILE);
        debug!(agent_id, path = %task_path.display(), bytes = task.content.len(), "writing task file");
        fs::write(&task_path, envelope.to_string())?;

        info!(agent_id, "task dispatched");
        agent.status = AgentStatus::Busy(InFlight::Task {
            respond_to: task.respond_to,
        });
        Ok(())
    }

    fn complete_task(&mut self, agent_id: &str, response_path: &Path) -> io::Result<()> {
        let Some(agent) = self.agents.get_mut(agent_id) else {
            return Ok(());
        };

        let AgentStatus::Busy(in_flight) = std::mem::replace(&mut agent.status, AgentStatus::Idle) else {
            return Ok(());
        };

        let output = match fs::read_to_string(response_path) {
            Ok(o) => o,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                agent.status = AgentStatus::Busy(in_flight);
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        // Clean up task files - remove task.json first to prevent the agent from
        // re-processing (agent checks: task.json exists && !response.json exists)
        let agent_dir = self.agents_dir.join(agent_id);
        let _ = fs::remove_file(agent_dir.join(TASK_FILE));
        let _ = fs::remove_file(response_path);

        in_flight.complete(output)?;

        info!(agent_id, "task completed");
        Ok(())
    }
}

// =============================================================================
// Event Loop
// =============================================================================

fn event_loop(
    listener: &Listener,
    fs_events: &mpsc::Receiver<Event>,
    state: &mut PoolState,
    signals: &DaemonSignals,
) -> io::Result<()> {
    // How long to block waiting for fs events before checking other sources
    let poll_timeout = Duration::from_millis(100);
    // How often to run periodic scans (agents, outputs, pending, heartbeats)
    let scan_interval = Duration::from_millis(500);
    let mut last_scan = Instant::now();

    loop {
        if signals.is_shutdown_triggered() {
            info!(
                in_flight = state.in_flight_count(),
                "shutdown requested, draining in-flight tasks"
            );
            return drain_and_shutdown(fs_events, state);
        }

        // Check for socket-based task submissions (non-blocking)
        if let Some(task) = accept_task(listener)? {
            state.enqueue(task);
        }

        // Block waiting for fs events (with timeout)
        match fs_events.recv_timeout(poll_timeout) {
            Ok(event) => {
                handle_fs_event(&event, state)
                    .map_err(|e| io::Error::new(e.kind(), format!("fs event handling failed: {e}")))?;
                // Drain any additional queued events
                while let Ok(event) = fs_events.try_recv() {
                    handle_fs_event(&event, state)
                        .map_err(|e| io::Error::new(e.kind(), format!("fs event handling failed: {e}")))?;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // No fs events, continue to periodic scans
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("fs event channel disconnected"));
            }
        }

        // Periodic scans for things FSEvents might miss
        if last_scan.elapsed() >= scan_interval {
            state.scan_agents()
                .map_err(|e| io::Error::new(e.kind(), format!("scan_agents failed: {e}")))?;
            state.scan_outputs()
                .map_err(|e| io::Error::new(e.kind(), format!("scan_outputs failed: {e}")))?;
            state.scan_pending()
                .map_err(|e| io::Error::new(e.kind(), format!("scan_pending failed: {e}")))?;
            last_scan = Instant::now();
        }

        // Dispatch any pending tasks to available agents
        if !signals.is_paused() {
            state.dispatch_pending()
                .map_err(|e| io::Error::new(e.kind(), format!("dispatch_pending failed: {e}")))?;
        }
    }
}

fn drain_and_shutdown(fs_events: &mpsc::Receiver<Event>, state: &mut PoolState) -> io::Result<()> {
    let poll_timeout = Duration::from_millis(100);

    while state.in_flight_count() > 0 {
        // Block waiting for fs events (with timeout)
        match fs_events.recv_timeout(poll_timeout) {
            Ok(event) => {
                handle_fs_event(&event, state)?;
                while let Ok(event) = fs_events.try_recv() {
                    handle_fs_event(&event, state)?;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Scan for outputs that FSEvents might have missed
                state.scan_outputs()?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    info!("shutdown complete");
    Ok(())
}

fn accept_task(listener: &Listener) -> io::Result<Option<Task>> {
    match listener.accept() {
        Ok(stream) => read_task(stream),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(io::Error::new(e.kind(), format!("socket accept failed: {e}"))),
    }
}

fn read_task(stream: Stream) -> io::Result<Option<Task>> {
    let mut reader = BufReader::new(&stream);

    let mut len_line = String::new();
    reader.read_line(&mut len_line)?;

    let len: usize = match len_line.trim().parse() {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };

    let mut content = vec![0u8; len];
    reader.read_exact(&mut content)?;

    let Ok(content) = String::from_utf8(content) else {
        return Ok(None);
    };

    Ok(Some(Task {
        content,
        respond_to: ResponseTarget::Socket(stream),
    }))
}

fn send_response(target: ResponseTarget, response: &Response) -> io::Result<()> {
    let json = serde_json::to_string(response)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    match target {
        ResponseTarget::Socket(mut stream) => {
            writeln!(stream, "{}", json.len())?;
            stream.write_all(json.as_bytes())?;
            stream.flush()
        }
        ResponseTarget::File(path) => {
            // Ensure parent directory exists (submitter may have been killed)
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    warn!(path = %path.display(), "submission directory gone, submitter likely died");
                    return Ok(()); // Submitter is gone, nothing to do
                }
            }
            fs::write(&path, &json).map_err(|e| {
                io::Error::new(e.kind(), format!("failed to write response to {}: {e}", path.display()))
            })?;
            debug!(path = %path.display(), "wrote file-based response");
            Ok(())
        }
    }
}

// =============================================================================
// Filesystem Events
// =============================================================================

fn handle_fs_event(event: &Event, state: &mut PoolState) -> io::Result<()> {
    trace!(kind = ?event.kind, paths = ?event.paths, "fs event");

    for path in &event.paths {
        let Some(relative) = path.strip_prefix(&state.agents_dir).ok() else {
            continue;
        };

        let components: Vec<_> = relative.components().collect();
        let Some(agent_id) = components
            .first()
            .and_then(|c| c.as_os_str().to_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };

        debug!(agent_id, components = components.len(), "processing event");

        match components.len() {
            1 => handle_agent_dir_event(event, agent_id, state),
            2 => {
                let Some(filename) = components[1].as_os_str().to_str() else {
                    continue;
                };
                handle_agent_file_event(event, agent_id, filename, path, state)?;
            }
            _ => {}
        }
    }

    Ok(())
}

fn handle_agent_dir_event(event: &Event, agent_id: &str, state: &mut PoolState) {
    let agent_dir = state.agents_dir.join(agent_id);

    if matches!(event.kind, EventKind::Remove(_)) {
        state.unregister(agent_id);
    } else if agent_dir.is_dir() {
        state.register(agent_id);
    }
}

fn handle_agent_file_event(
    event: &Event,
    agent_id: &str,
    filename: &str,
    path: &Path,
    state: &mut PoolState,
) -> io::Result<()> {
    let agent_dir = state.agents_dir.join(agent_id);
    if agent_dir.is_dir() {
        state.register(agent_id);
    }

    // Check for response file
    if filename == RESPONSE_FILE
        && matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_))
        && path.exists()
    {
        state.complete_task(agent_id, path)?;
    }

    Ok(())
}

// =============================================================================
// Setup Helpers
// =============================================================================

fn create_listener(socket_path: &Path) -> io::Result<Listener> {
    let name = socket_path
        .to_fs_name::<GenericFilePath>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    let listener = ListenerOptions::new()
        .name(name)
        .create_sync()
        .map_err(|e| io::Error::new(io::ErrorKind::AddrInUse, e))?;

    listener
        .set_nonblocking(ListenerNonblockingMode::Accept)
        .map_err(io::Error::other)?;

    Ok(listener)
}

fn create_watcher(agents_dir: &Path) -> io::Result<(RecommendedWatcher, mpsc::Receiver<Event>)> {
    let (tx, rx) = mpsc::channel();

    let config = notify::Config::default().with_poll_interval(Duration::from_millis(100));

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        config,
    )
    .map_err(io::Error::other)?;

    watcher
        .watch(agents_dir, RecursiveMode::Recursive)
        .map_err(io::Error::other)?;

    Ok((watcher, rx))
}

struct SocketCleanup(PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
