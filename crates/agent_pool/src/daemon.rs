//! Agent pool daemon - dispatches tasks to available agents.
//!
//! The pool watches a directory for agents and dispatches incoming tasks
//! to whichever agent is available. Each agent is a subdirectory that
//! processes tasks via the file protocol (`{id}.input` → `{id}.output`).
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
//! // ... do work ...
//! handle.shutdown();  // Gracefully stops the daemon
//! ```

use crate::constants::{AGENTS_DIR, LOCK_FILE, SOCKET_NAME};
use crate::lock::acquire_lock;
use crate::response::Response;

/// Stable filename for task input.
const TASK_FILE: &str = "task.json";
/// Stable filename for agent response.
const RESPONSE_FILE: &str = "response.json";
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
use tracing::{debug, info, trace};

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

/// Spawn the daemon in a background thread with graceful shutdown support.
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

    let thread = thread::spawn(move || {
        let _lock = lock;
        let _cleanup = SocketCleanup(socket_path.clone());
        let _watcher = watcher;

        info!(socket = %socket_path.display(), "listening");

        let mut state = PoolState::new(agents_dir);
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
    let root = root.as_ref();

    fs::create_dir_all(root)?;

    let lock_path = root.join(LOCK_FILE);
    let _lock = acquire_lock(&lock_path)?;

    let agents_dir = root.join(AGENTS_DIR);
    fs::create_dir_all(&agents_dir)?;

    let socket_path = root.join(SOCKET_NAME);
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }

    let _cleanup = SocketCleanup(socket_path.clone());

    let listener = create_listener(&socket_path)?;
    let (watcher, fs_events) = create_watcher(&agents_dir)?;
    let _watcher = watcher;

    info!(socket = %socket_path.display(), "listening");

    let mut state = PoolState::new(agents_dir);
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

/// State for a single agent.
struct AgentState {
    /// If busy, holds the stream to respond to when task completes.
    in_flight: Option<Stream>,
}

impl AgentState {
    const fn new() -> Self {
        Self { in_flight: None }
    }

    const fn is_available(&self) -> bool {
        self.in_flight.is_none()
    }
}

/// Runtime state of the agent pool.
struct PoolState {
    agents_dir: PathBuf,
    agents: HashMap<String, AgentState>,
    pending: VecDeque<Task>,
}

struct Task {
    content: String,
    respond_to: Stream,
}

impl PoolState {
    fn new(agents_dir: PathBuf) -> Self {
        Self {
            agents_dir,
            agents: HashMap::new(),
            pending: VecDeque::new(),
        }
    }

    fn scan_agents(&mut self) -> io::Result<()> {
        for entry in fs::read_dir(&self.agents_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                self.register(name);
            }
        }
        Ok(())
    }

    fn in_flight_count(&self) -> usize {
        self.agents
            .values()
            .filter(|a| a.in_flight.is_some())
            .count()
    }

    fn scan_outputs(&mut self) -> io::Result<()> {
        let busy: Vec<_> = self
            .agents
            .iter()
            .filter(|(_, a)| a.in_flight.is_some())
            .map(|(id, _)| id.clone())
            .collect();

        for agent_id in busy {
            let response_path = self.agents_dir.join(&agent_id).join(RESPONSE_FILE);
            if response_path.exists() {
                self.complete_task(&agent_id, &response_path)?;
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

    fn find_available_agent(&self) -> Option<String> {
        self.agents
            .iter()
            .find(|(_, a)| a.is_available())
            .map(|(id, _)| id.clone())
    }

    fn dispatch_to(&mut self, agent_id: &str, task: Task) -> io::Result<()> {
        let Some(agent) = self.agents.get_mut(agent_id) else {
            return Err(io::Error::other("agent not found"));
        };

        let task_path = self.agents_dir.join(agent_id).join(TASK_FILE);
        fs::write(&task_path, &task.content)?;

        info!(agent_id, "task dispatched");
        agent.in_flight = Some(task.respond_to);
        Ok(())
    }

    fn complete_task(&mut self, agent_id: &str, response_path: &Path) -> io::Result<()> {
        let Some(agent) = self.agents.get_mut(agent_id) else {
            return Ok(());
        };

        let Some(stream) = agent.in_flight.take() else {
            return Ok(());
        };

        let output = match fs::read_to_string(response_path) {
            Ok(o) => o,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                agent.in_flight = Some(stream);
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        // Clean up task files - remove task.json first to prevent the agent from
        // re-processing (agent checks: task.json exists && !response.json exists)
        let agent_dir = self.agents_dir.join(agent_id);
        let _ = fs::remove_file(agent_dir.join(TASK_FILE));
        let _ = fs::remove_file(response_path);

        let response = Response::processed(output);
        send_response(stream, &response)?;

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
    let scan_interval = Duration::from_millis(200);
    let mut last_scan = Instant::now();

    loop {
        if signals.is_shutdown_triggered() {
            info!(
                in_flight = state.in_flight_count(),
                "shutdown requested, draining in-flight tasks"
            );
            return drain_and_shutdown(fs_events, state);
        }

        if let Some(task) = accept_task(listener)? {
            state.enqueue(task);
        }

        while let Ok(event) = fs_events.try_recv() {
            handle_fs_event(&event, state)?;
        }

        if last_scan.elapsed() >= scan_interval {
            state.scan_agents()?;
            state.scan_outputs()?;
            last_scan = Instant::now();
        }

        if !signals.is_paused() {
            state.dispatch_pending()?;
        }

        thread::sleep(Duration::from_millis(10));
    }
}

fn drain_and_shutdown(fs_events: &mpsc::Receiver<Event>, state: &mut PoolState) -> io::Result<()> {
    let scan_interval = Duration::from_millis(100);
    let mut last_scan = Instant::now();

    while state.in_flight_count() > 0 {
        while let Ok(event) = fs_events.try_recv() {
            handle_fs_event(&event, state)?;
        }

        if last_scan.elapsed() >= scan_interval {
            state.scan_outputs()?;
            last_scan = Instant::now();
        }

        thread::sleep(Duration::from_millis(10));
    }

    info!("shutdown complete");
    Ok(())
}

fn accept_task(listener: &Listener) -> io::Result<Option<Task>> {
    match listener.accept() {
        Ok(stream) => read_task(stream),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e),
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
        respond_to: stream,
    }))
}

fn send_response(mut stream: Stream, response: &Response) -> io::Result<()> {
    let json = serde_json::to_string(response)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    writeln!(stream, "{}", json.len())?;
    stream.write_all(json.as_bytes())?;
    stream.flush()
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
