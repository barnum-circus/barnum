//! Agent pool daemon - dispatches tasks to available agents.
//!
//! The pool watches a directory for agents and dispatches incoming tasks
//! to whichever agent is available. Each agent is a subdirectory that
//! processes tasks via the file protocol (`next_task` → `in_progress` → `output`).
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

use crate::constants::{
    AGENTS_DIR, IN_PROGRESS_FILE, LOCK_FILE, NEXT_TASK_FILE, OUTPUT_FILE, SOCKET_NAME,
};
use crate::lock::acquire_lock;
use crate::response::Response;
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
    /// When true, initiate graceful shutdown.
    shutdown: Arc<AtomicBool>,
    /// When true, pause dispatching new tasks (but keep accepting and queuing).
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
///
/// When dropped without calling `shutdown()`, the daemon continues running
/// in the background thread (fire-and-forget). Call `shutdown()` to
/// gracefully stop the daemon and wait for it to finish.
pub struct DaemonHandle {
    signals: DaemonSignals,
    thread: Option<thread::JoinHandle<io::Result<()>>>,
}

impl DaemonHandle {
    /// Pause task dispatching.
    ///
    /// While paused:
    /// - New connections are still accepted
    /// - Tasks are queued but not dispatched to agents
    /// - In-flight tasks continue to completion
    ///
    /// Use [`resume()`](Self::resume) to resume normal operation.
    pub fn pause(&self) {
        self.signals.set_paused(true);
    }

    /// Resume task dispatching after a pause.
    ///
    /// Queued tasks will begin dispatching to available agents.
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
    /// This will:
    /// 1. Stop accepting new socket connections
    /// 2. Complete any in-flight tasks
    /// 3. Clean up resources
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon thread panicked or encountered an I/O error.
    pub fn shutdown(mut self) -> io::Result<()> {
        self.signals.trigger_shutdown();
        self.join()
    }

    /// Wait for the daemon to stop without triggering shutdown.
    ///
    /// This is useful if shutdown was triggered by another clone of the handle.
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
/// Returns a handle that can be used to shut down the daemon gracefully.
///
/// # Errors
///
/// Returns an error if:
/// - The lock can't be acquired (another instance is running)
/// - Directory or socket setup fails
pub fn spawn(root: impl AsRef<Path>) -> io::Result<DaemonHandle> {
    let root = root.as_ref().to_path_buf();

    // Do initial setup synchronously so errors propagate to caller
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
        // These guards ensure cleanup on drop
        let _lock = lock;
        let _cleanup = SocketCleanup(socket_path.clone());
        let _watcher = watcher;

        info!(socket = %socket_path.display(), "listening");

        let mut state = PoolState::new(agents_dir);
        state.scan_agents()?;

        event_loop(&listener, &fs_events, &mut state, &signals_clone)
    });

    // Give daemon time to start listening
    thread::sleep(Duration::from_millis(50));

    Ok(DaemonHandle {
        signals,
        thread: Some(thread),
    })
}

/// Run the agent pool daemon (blocking).
///
/// This is the entry point for CLI usage. It acquires the lock, sets up the
/// socket, and runs the event loop until the process is killed.
///
/// This function never returns on success - it runs forever until killed.
/// The return type encodes this: `Infallible` can never be constructed.
///
/// For programmatic control with graceful shutdown, use [`spawn()`] instead.
///
/// # Errors
///
/// Returns an error if:
/// - The lock can't be acquired (another instance is running)
/// - Directory or socket setup fails
/// - An I/O error occurs during event processing
pub fn run(root: impl AsRef<Path>) -> io::Result<Infallible> {
    let root = root.as_ref();

    // Ensure root exists (needed for lock file)
    fs::create_dir_all(root)?;

    // Acquire lock FIRST - don't create anything else until we know we own this
    let lock_path = root.join(LOCK_FILE);
    let _lock = acquire_lock(&lock_path)?;

    // Now we own the lock - safe to set up
    let agents_dir = root.join(AGENTS_DIR);
    fs::create_dir_all(&agents_dir)?;

    let socket_path = root.join(SOCKET_NAME);
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }

    // Set up cleanup on drop (lock guard handles lock file, Cleanup handles socket)
    let _cleanup = SocketCleanup(socket_path.clone());

    let listener = create_listener(&socket_path)?;
    let (watcher, fs_events) = create_watcher(&agents_dir)?;
    let _watcher = watcher; // Keep alive

    info!(socket = %socket_path.display(), "listening");

    let mut state = PoolState::new(agents_dir);
    state.scan_agents()?;

    // No shutdown signal - run forever
    let signals = DaemonSignals::new();
    match event_loop(&listener, &fs_events, &mut state, &signals) {
        Ok(()) => {
            // Event loop only returns Ok(()) on shutdown signal, which we never trigger
            unreachable!("event loop returned without shutdown signal")
        }
        Err(e) => Err(e),
    }
}

// =============================================================================
// Pool State
// =============================================================================

/// Runtime state of the agent pool.
struct PoolState {
    agents_dir: PathBuf,
    /// Maps `agent_id` to their pending response stream (`None` = available).
    agents: HashMap<String, Option<Stream>>,
    /// Tasks waiting for an available agent
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

    /// Count of tasks currently being processed by agents.
    fn in_flight_count(&self) -> usize {
        self.agents
            .values()
            .filter(|stream| stream.is_some())
            .count()
    }

    fn scan_outputs(&mut self) -> io::Result<()> {
        let busy: Vec<_> = self
            .agents
            .iter()
            .filter(|(_, stream)| stream.is_some())
            .map(|(id, _)| id.clone())
            .collect();

        for agent_id in busy {
            let output_path = self.agents_dir.join(&agent_id).join(OUTPUT_FILE);
            if output_path.exists() {
                self.complete_task(&agent_id, &output_path)?;
            }
        }
        Ok(())
    }

    fn register(&mut self, agent_id: &str) {
        if !self.agents.contains_key(agent_id) {
            info!(agent_id, "agent registered");
            self.agents.insert(agent_id.to_string(), None);
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
            .find(|(_, stream)| stream.is_none())
            .map(|(id, _)| id.clone())
    }

    fn dispatch_to(&mut self, agent_id: &str, task: Task) -> io::Result<()> {
        let task_path = self.agents_dir.join(agent_id).join(NEXT_TASK_FILE);
        fs::write(&task_path, &task.content)?;

        info!(agent_id, "task dispatched");
        self.agents
            .insert(agent_id.to_string(), Some(task.respond_to));
        Ok(())
    }

    fn complete_task(&mut self, agent_id: &str, output_path: &Path) -> io::Result<()> {
        let Some(stream) = self.agents.get_mut(agent_id).and_then(Option::take) else {
            return Ok(()); // Not busy or not registered
        };

        let output = match fs::read_to_string(output_path) {
            Ok(o) => o,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Output disappeared - put stream back
                self.agents.insert(agent_id.to_string(), Some(stream));
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        // Clean up task files
        let agent_dir = self.agents_dir.join(agent_id);
        let _ = fs::remove_file(output_path);
        let _ = fs::remove_file(agent_dir.join(NEXT_TASK_FILE));
        let _ = fs::remove_file(agent_dir.join(IN_PROGRESS_FILE));

        let response = Response::processed(output);
        send_response(stream, &response)?;

        info!(agent_id, "task completed");
        self.agents.insert(agent_id.to_string(), None);
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
        // Check for shutdown request
        if signals.is_shutdown_triggered() {
            info!(
                in_flight = state.in_flight_count(),
                "shutdown requested, draining in-flight tasks"
            );
            return drain_and_shutdown(fs_events, state);
        }

        // Handle incoming task submissions
        if let Some(task) = accept_task(listener)? {
            state.enqueue(task);
        }

        // Handle filesystem events
        while let Ok(event) = fs_events.try_recv() {
            handle_fs_event(&event, state)?;
        }

        // Periodic rescan (FSEvents on macOS can be laggy)
        if last_scan.elapsed() >= scan_interval {
            state.scan_agents()?;
            state.scan_outputs()?;
            last_scan = Instant::now();
        }

        // Match pending tasks with available agents (unless paused)
        if !signals.is_paused() {
            state.dispatch_pending()?;
        }

        thread::sleep(Duration::from_millis(10));
    }
}

/// Drain in-flight tasks during shutdown (don't accept new ones).
fn drain_and_shutdown(fs_events: &mpsc::Receiver<Event>, state: &mut PoolState) -> io::Result<()> {
    let scan_interval = Duration::from_millis(100);
    let mut last_scan = Instant::now();

    while state.in_flight_count() > 0 {
        // Process filesystem events to detect task completion
        while let Ok(event) = fs_events.try_recv() {
            handle_fs_event(&event, state)?;
        }

        // Periodic rescan for completion
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
    // Any file event means the agent exists
    let agent_dir = state.agents_dir.join(agent_id);
    if agent_dir.is_dir() {
        state.register(agent_id);
    }

    // Check for output file
    if filename == OUTPUT_FILE
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

/// Cleans up the socket file on drop.
struct SocketCleanup(PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
