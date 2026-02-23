//! Daemon - Wires together the core state machine and I/O.
//!
//! This module creates the event and effect channels, spawns the event loop,
//! and runs the I/O layer that handles filesystem events, socket connections,
//! and effect execution.

use std::convert::Infallible;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use interprocess::local_socket::{
    GenericFilePath, Listener, ListenerNonblockingMode, ListenerOptions, Stream, prelude::*,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, info, trace, warn};

use std::collections::HashSet;

use crate::client::Payload;
use crate::constants::{AGENTS_DIR, LOCK_FILE, PENDING_DIR, SOCKET_NAME, TASK_FILE};
use crate::lock::acquire_lock;

use super::core::{AgentId, Effect, Event, TaskId};
use super::io::{
    AgentMap, ExternalTaskData, ExternalTaskMap, IoConfig,
    TaskIdAllocator, execute_effect,
};
use super::path_category::{self, PathCategory};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the daemon.
#[derive(Debug, Clone)]
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

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            idle_agent_timeout: Duration::from_secs(60),
            default_task_timeout: Duration::from_secs(300),
            immediate_heartbeat_enabled: true,
            periodic_heartbeat_enabled: true,
        }
    }
}

impl From<DaemonConfig> for IoConfig {
    fn from(config: DaemonConfig) -> Self {
        IoConfig {
            idle_agent_timeout: config.idle_agent_timeout,
            immediate_heartbeat_enabled: config.immediate_heartbeat_enabled,
            periodic_heartbeat_enabled: config.periodic_heartbeat_enabled,
            default_task_timeout: config.default_task_timeout,
        }
    }
}

// =============================================================================
// Daemon State
// =============================================================================

/// Daemon run state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum DaemonState {
    /// Running normally, dispatching tasks.
    Playing = 0,
    /// Paused, not dispatching new tasks.
    Paused = 1,
    /// Shutdown requested.
    Shutdown = 2,
}

impl DaemonState {
    fn from_u32(value: u32) -> Self {
        match value {
            0 => Self::Playing,
            1 => Self::Paused,
            2 => Self::Shutdown,
            _ => Self::Shutdown, // Invalid state treated as shutdown
        }
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Shared control signals for the daemon.
#[derive(Clone)]
struct DaemonSignals {
    state: Arc<AtomicU32>,
}

impl DaemonSignals {
    fn new() -> Self {
        Self {
            state: Arc::new(AtomicU32::new(DaemonState::Playing as u32)),
        }
    }

    fn get_state(&self) -> DaemonState {
        DaemonState::from_u32(self.state.load(Ordering::SeqCst))
    }

    fn trigger_shutdown(&self) {
        self.state.store(DaemonState::Shutdown as u32, Ordering::SeqCst);
    }

    fn is_shutdown_triggered(&self) -> bool {
        self.get_state() == DaemonState::Shutdown
    }

    fn set_paused(&self, paused: bool) {
        let new_state = if paused {
            DaemonState::Paused
        } else {
            DaemonState::Playing
        };
        self.state.store(new_state as u32, Ordering::SeqCst);
    }

    fn is_paused(&self) -> bool {
        self.get_state() == DaemonState::Paused
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

/// Spawn the daemon in a background thread with default configuration.
///
/// # Errors
///
/// Returns an error if the lock can't be acquired or setup fails.
pub fn spawn(root: impl AsRef<Path>) -> io::Result<DaemonHandle> {
    spawn_with_config(root, DaemonConfig::default())
}

/// Spawn the daemon in a background thread with custom configuration.
///
/// # Errors
///
/// Returns an error if the lock can't be acquired or setup fails.
pub fn spawn_with_config(root: impl AsRef<Path>, config: DaemonConfig) -> io::Result<DaemonHandle> {
    fs::create_dir_all(root.as_ref())?;
    // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
    // so FSEvent paths match our stored paths.
    let root = fs::canonicalize(root.as_ref())?;

    let lock_path = root.join(LOCK_FILE);
    let lock = acquire_lock(&lock_path)?;

    // TODO: Add clean option to clear stale agent/pending directories on restart.
    // See run_with_config for details.

    let agents_dir = root.join(AGENTS_DIR);
    fs::create_dir_all(&agents_dir)?;

    let socket_path = root.join(SOCKET_NAME);
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }

    // Create wake channel for event-driven I/O
    let (wake_tx, wake_rx) = mpsc::channel();

    let listener = create_socket_listener(&socket_path)?;
    let (watcher, fs_events) = create_fs_watcher(&root, wake_tx.clone())?;

    // Create pending_dir AFTER watcher is running.
    // Clients use pending_dir existence as the "ready" signal.
    let pending_dir = root.join(PENDING_DIR);
    fs::create_dir_all(&pending_dir)?;

    let signals = DaemonSignals::new();
    let signals_clone = signals.clone();

    let thread = thread::spawn(move || {
        let _lock = lock;
        let _cleanup = SocketCleanup(socket_path.clone());
        let _watcher = watcher;

        info!(socket = %socket_path.display(), "daemon listening");

        run_daemon(
            listener,
            fs_events,
            wake_rx,
            wake_tx,
            &agents_dir,
            &pending_dir,
            &config.into(),
            &signals_clone,
        )
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
    run_with_config(root, DaemonConfig::default())
}

/// Run the agent pool daemon with custom configuration (blocking, never returns on success).
///
/// # Errors
///
/// Returns an error if the lock can't be acquired or an I/O error occurs.
pub fn run_with_config(root: impl AsRef<Path>, config: DaemonConfig) -> io::Result<Infallible> {
    fs::create_dir_all(root.as_ref())?;
    // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
    // so FSEvent paths match our stored paths.
    let root = fs::canonicalize(root.as_ref())?;

    let lock_path = root.join(LOCK_FILE);
    let _lock = acquire_lock(&lock_path)?;

    // TODO: Add --clean flag to clear stale agent/pending directories on restart.
    // Currently, leftover directories from previous runs are picked up by the scan
    // and treated as live agents, which causes tasks to be assigned to non-existent agents.

    let agents_dir = root.join(AGENTS_DIR);
    fs::create_dir_all(&agents_dir)?;

    let socket_path = root.join(SOCKET_NAME);
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }

    let _cleanup = SocketCleanup(socket_path.clone());

    // Create wake channel for event-driven I/O
    let (wake_tx, wake_rx) = mpsc::channel();

    let listener = create_socket_listener(&socket_path)?;
    let (watcher, fs_events) = create_fs_watcher(&root, wake_tx.clone())?;
    let _watcher = watcher;

    // Create pending_dir AFTER watcher is running.
    // Clients use pending_dir existence as the "ready" signal.
    let pending_dir = root.join(PENDING_DIR);
    fs::create_dir_all(&pending_dir)?;

    info!(socket = %socket_path.display(), "daemon listening");

    let signals = DaemonSignals::new();
    let io_config = config.into();
    match run_daemon(
        listener,
        fs_events,
        wake_rx,
        wake_tx,
        &agents_dir,
        &pending_dir,
        &io_config,
        &signals,
    ) {
        Ok(()) => unreachable!("event loop returned without shutdown signal"),
        Err(e) => Err(e),
    }
}

// =============================================================================
// Main Daemon Loop
// =============================================================================

/// The main daemon function that orchestrates core and I/O.
fn run_daemon(
    listener: Listener,
    fs_events: mpsc::Receiver<notify::Event>,
    wake_rx: mpsc::Receiver<()>,
    wake_tx: mpsc::Sender<()>,
    agents_dir: &Path,
    pending_dir: &Path,
    io_config: &IoConfig,
    signals: &DaemonSignals,
) -> io::Result<()> {
    // Create channels between event loop and I/O
    let (events_tx, events_rx) = mpsc::channel::<Event>();
    let (effects_tx, effects_rx) = mpsc::channel::<Effect>();

    // Create socket channel and spawn accept thread
    let (socket_tx, socket_rx) = mpsc::channel();
    let _socket_thread = spawn_socket_accept_thread(listener, socket_tx, wake_tx.clone());

    // I/O state
    let mut agent_map = AgentMap::new();
    let mut external_task_map = ExternalTaskMap::new();
    let mut task_id_allocator = TaskIdAllocator::new();
    // Track agents with pending responses to deduplicate FSWatcher events
    let mut pending_responses: HashSet<AgentId> = HashSet::new();

    // Track kicked agent paths to reject re-registration attempts
    let mut kicked_paths: HashSet<PathBuf> = HashSet::new();

    // Clone signals for the event loop thread
    let event_loop_signals = signals.clone();

    // Spawn event loop in a separate thread
    let event_loop_handle = thread::spawn(move || {
        run_event_loop_with_shutdown(events_rx, effects_tx, wake_tx, event_loop_signals)
    });

    // Do initial scan of existing agents (kicked_paths is empty at startup)
    scan_agents(agents_dir, &events_tx, &mut agent_map, &kicked_paths, &mut task_id_allocator, io_config)?;

    // Run the I/O loop
    let result = io_loop(
        wake_rx,
        fs_events,
        socket_rx,
        effects_rx,
        &events_tx,
        &mut agent_map,
        &mut external_task_map,
        &mut task_id_allocator,
        &mut pending_responses,
        &mut kicked_paths,
        agents_dir,
        pending_dir,
        io_config,
        signals,
    );

    // Wait for event loop to finish (it will exit when it sees shutdown signal)
    let final_state = event_loop_handle
        .join()
        .map_err(|_| io::Error::other("event loop thread panicked"))?;

    debug!(
        agents = final_state.agent_count(),
        pending = final_state.pending_count(),
        "daemon shutdown complete"
    );

    result
}

/// Run the event loop with shutdown signal checking.
///
/// This wraps the pure event loop to add shutdown signal checking,
/// since timer threads keep the events channel alive.
#[allow(clippy::needless_pass_by_value)] // We take ownership intentionally - runs in spawned thread
fn run_event_loop_with_shutdown(
    events_rx: mpsc::Receiver<Event>,
    effects_tx: mpsc::Sender<Effect>,
    wake_tx: mpsc::Sender<()>,
    signals: DaemonSignals,
) -> super::core::PoolState {
    use super::core::{PoolState, step};

    let mut state = PoolState::new();

    loop {
        // Check shutdown signal
        if signals.is_shutdown_triggered() {
            debug!("event loop: shutdown signal received");
            break;
        }

        // Use recv_timeout to periodically check shutdown signal
        match events_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                trace!(?event, "received event");
                let (new_state, effects) = step(state, event);
                state = new_state;

                for effect in effects {
                    trace!(?effect, "emitting effect");
                    let _ = effects_tx.send(effect);
                    let _ = wake_tx.send(()); // Wake main loop
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Continue to check shutdown signal
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                debug!("event loop: channel disconnected");
                break;
            }
        }
    }

    debug!(
        pending = state.pending_count(),
        agents = state.agent_count(),
        "event loop stopped"
    );

    state
}

/// The I/O loop that handles filesystem events, socket connections, and effects.
///
/// Uses the wake channel pattern: blocks until any event source has work,
/// then drains all sources non-blocking. Only wakes on actual events.
#[allow(clippy::too_many_arguments)]
fn io_loop(
    wake_rx: mpsc::Receiver<()>,
    fs_events: mpsc::Receiver<notify::Event>,
    socket_rx: mpsc::Receiver<(String, Stream)>,
    effects_rx: mpsc::Receiver<Effect>,
    events_tx: &mpsc::Sender<Event>,
    agent_map: &mut AgentMap,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    pending_responses: &mut HashSet<AgentId>,
    kicked_paths: &mut HashSet<PathBuf>,
    agents_dir: &Path,
    pending_dir: &Path,
    io_config: &IoConfig,
    signals: &DaemonSignals,
) -> io::Result<()> {
    debug!("io_loop starting, agents_dir={:?}, pending_dir={:?}", agents_dir, pending_dir);

    loop {
        // Block until woken or timeout (for shutdown check)
        match wake_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(()) => {} // Woken by an event source
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Check shutdown on timeout
                if signals.is_shutdown_triggered() {
                    info!("shutdown requested");
                    return Ok(());
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // All senders dropped, shutdown
                info!("wake channel disconnected, shutting down");
                return Ok(());
            }
        }

        // Check shutdown after being woken
        if signals.is_shutdown_triggered() {
            info!("shutdown requested");
            return Ok(());
        }

        // Drain socket submissions (non-blocking)
        if !signals.is_paused() {
            while let Ok((raw, stream)) = socket_rx.try_recv() {
                let content = match resolve_payload(&raw) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(error = %e, "failed to resolve socket payload");
                        continue;
                    }
                };

                let external_id = external_task_map.register_socket(
                    stream,
                    ExternalTaskData {
                        content,
                        timeout: io_config.default_task_timeout,
                    },
                );
                info!(external_task_id = external_id.0, "socket task submitted");
                let _ = events_tx.send(Event::TaskSubmitted {
                    task_id: TaskId::External(external_id),
                });
            }
        }

        // Drain filesystem events (non-blocking)
        while let Ok(event) = fs_events.try_recv() {
            trace!(kind = ?event.kind, "fs event");
            handle_fs_event(
                &event,
                events_tx,
                agent_map,
                external_task_map,
                task_id_allocator,
                pending_responses,
                kicked_paths,
                agents_dir,
                pending_dir,
                io_config,
            );
        }

        // Drain effects from event loop (non-blocking)
        while let Ok(effect) = effects_rx.try_recv() {
            trace!(?effect, "executing effect");
            // Clear pending response tracking when TaskCompleted cleans up the response file
            if let Effect::TaskCompleted { agent_id, .. } = &effect {
                pending_responses.remove(agent_id);
            }
            execute_effect(
                effect,
                agent_map,
                external_task_map,
                task_id_allocator,
                kicked_paths,
                events_tx,
                io_config,
            )?;
        }
    }
}

// =============================================================================
// Filesystem Event Handling
// =============================================================================

/// Handle a filesystem event, converting it to core events.
#[allow(clippy::too_many_arguments)]
fn handle_fs_event(
    event: &notify::Event,
    events_tx: &mpsc::Sender<Event>,
    agent_map: &mut AgentMap,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    pending_responses: &mut HashSet<AgentId>,
    kicked_paths: &mut HashSet<PathBuf>,
    agents_dir: &Path,
    pending_dir: &Path,
    io_config: &IoConfig,
) {
    trace!(kind = ?event.kind, paths = ?event.paths, "fs event");

    for path in &event.paths {
        let Some(category) = path_category::categorize(path, agents_dir, pending_dir) else {
            trace!(?path, "path did not match any category");
            continue;
        };
        debug!(?path, ?category, "categorized path");

        match category {
            PathCategory::AgentDir { name } => {
                let agent_path = agents_dir.join(&name);
                handle_agent_dir(&agent_path, events_tx, agent_map, kicked_paths, task_id_allocator, io_config);
            }
            PathCategory::AgentResponse { name } => {
                let agent_path = agents_dir.join(&name);
                handle_agent_response(&agent_path, path, events_tx, agent_map, pending_responses, kicked_paths, task_id_allocator, io_config);
            }
            PathCategory::PendingDir { uuid } => {
                let submission_dir = pending_dir.join(&uuid);
                let task_path = submission_dir.join(TASK_FILE);
                if task_path.exists() {
                    register_pending_task(&submission_dir, events_tx, external_task_map, task_id_allocator, io_config);
                }
            }
            PathCategory::PendingTask { uuid } => {
                let submission_dir = pending_dir.join(&uuid);
                if path.exists() {
                    register_pending_task(&submission_dir, events_tx, external_task_map, task_id_allocator, io_config);
                }
            }
        }
    }
}

/// Handle an agent directory event (creation or deletion).
fn handle_agent_dir(
    agent_path: &Path,
    events_tx: &mpsc::Sender<Event>,
    agent_map: &mut AgentMap,
    kicked_paths: &mut HashSet<PathBuf>,
    task_id_allocator: &mut TaskIdAllocator,
    io_config: &IoConfig,
) {
    if !agent_path.exists() {
        // Directory deleted - clean up tracking
        kicked_paths.remove(agent_path);
        if let Some(agent_id) = agent_map.get_id_by_path(agent_path) {
            let _ = events_tx.send(Event::AgentDeregistered { agent_id });
        }
    } else if agent_path.is_dir()
        && !kicked_paths.contains(agent_path)
        && !is_kicked_agent(agent_path)
    {
        // Only register if not kicked (in-memory or via task.json)
        if let Some(agent_id) = agent_map.register_directory(agent_path.to_path_buf(), ()) {
            let heartbeat_task_id = if io_config.immediate_heartbeat_enabled {
                Some(task_id_allocator.allocate_heartbeat())
            } else {
                None
            };
            let _ = events_tx.send(Event::AgentRegistered {
                agent_id,
                heartbeat_task_id,
            });
        }
    }
}

/// Handle an agent response file event.
fn handle_agent_response(
    agent_path: &Path,
    response_path: &Path,
    events_tx: &mpsc::Sender<Event>,
    agent_map: &mut AgentMap,
    pending_responses: &mut HashSet<AgentId>,
    kicked_paths: &HashSet<PathBuf>,
    task_id_allocator: &mut TaskIdAllocator,
    io_config: &IoConfig,
) {
    if !response_path.exists() || kicked_paths.contains(agent_path) {
        return;
    }

    // Register agent if not already known (response arrived before we saw the directory)
    if agent_map.get_id_by_path(agent_path).is_none() {
        if let Some(agent_id) = agent_map.register_directory(agent_path.to_path_buf(), ()) {
            let heartbeat_task_id = if io_config.immediate_heartbeat_enabled {
                Some(task_id_allocator.allocate_heartbeat())
            } else {
                None
            };
            let _ = events_tx.send(Event::AgentRegistered {
                agent_id,
                heartbeat_task_id,
            });
        }
    }

    // Send response event (deduplicated)
    if let Some(agent_id) = agent_map.get_id_by_path(agent_path) {
        if pending_responses.insert(agent_id) {
            let _ = events_tx.send(Event::AgentResponded { agent_id });
        } else {
            trace!(agent_id = agent_id.0, "skipping duplicate AgentResponded");
        }
    }
}

/// Register a pending task from the filesystem.
fn register_pending_task(
    submission_dir: &Path,
    events_tx: &mpsc::Sender<Event>,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    io_config: &IoConfig,
) {
    let task_path = submission_dir.join(TASK_FILE);
    let response_path = submission_dir.join(crate::constants::RESPONSE_FILE);

    // Already registered?
    if external_task_map.get_id_by_path(submission_dir).is_some() {
        return;
    }

    // Already completed? (response.json exists)
    if response_path.exists() {
        return;
    }

    // Read and resolve payload
    let raw = match fs::read_to_string(&task_path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!(path = %task_path.display(), error = %e, "failed to read pending task");
            return;
        }
    };

    let content = match resolve_payload(&raw) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %task_path.display(), error = %e, "failed to resolve payload");
            return;
        }
    };

    // Register the task
    let external_id = task_id_allocator.allocate_external();
    if external_task_map.register(
        external_id,
        submission_dir.to_path_buf(),
        ExternalTaskData {
            content,
            timeout: io_config.default_task_timeout,
        },
    ) {
        info!(external_task_id = external_id.0, "file-based task registered");
        let _ = events_tx.send(Event::TaskSubmitted {
            task_id: TaskId::External(external_id),
        });
    }
}

// =============================================================================
// Periodic Scans
// =============================================================================

/// Scan the agents directory and register any new agents.
fn scan_agents(
    agents_dir: &Path,
    events_tx: &mpsc::Sender<Event>,
    agent_map: &mut AgentMap,
    kicked_paths: &HashSet<PathBuf>,
    task_id_allocator: &mut TaskIdAllocator,
    io_config: &IoConfig,
) -> io::Result<()> {
    if !agents_dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(agents_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let agent_path = entry.path();
        // Skip kicked agents (in-memory tracking)
        if kicked_paths.contains(&agent_path) {
            continue;
        }
        // Skip agents with Kicked task.json (survives daemon restart)
        if is_kicked_agent(&agent_path) {
            trace!(?agent_path, "skipping kicked agent directory");
            continue;
        }
        if let Some(agent_id) = agent_map.register_directory(agent_path, ()) {
            info!(agent_id = agent_id.0, "agent registered");
            let heartbeat_task_id = if io_config.immediate_heartbeat_enabled {
                Some(task_id_allocator.allocate_heartbeat())
            } else {
                None
            };
            let _ = events_tx.send(Event::AgentRegistered {
                agent_id,
                heartbeat_task_id,
            });
        }
    }

    Ok(())
}

/// Check if an agent directory contains a Kicked task.json.
///
/// This handles the case where the daemon restarts and finds old kicked
/// agent directories from a previous run.
fn is_kicked_agent(agent_path: &Path) -> bool {
    let task_path = agent_path.join(TASK_FILE);
    if let Ok(content) = fs::read_to_string(&task_path) {
        // Quick check for "Kicked" in the JSON
        if content.contains("\"kind\":\"Kicked\"") || content.contains("\"kind\": \"Kicked\"") {
            return true;
        }
    }
    false
}

// =============================================================================
// Socket Handling
// =============================================================================

/// Spawn a thread that accepts socket connections and forwards them to the main loop.
fn spawn_socket_accept_thread(
    listener: Listener,
    socket_tx: mpsc::Sender<(String, Stream)>,
    wake_tx: mpsc::Sender<()>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            match accept_socket_task(&listener) {
                Ok(Some((raw, stream))) => {
                    if socket_tx.send((raw, stream)).is_err() {
                        // Receiver dropped, shutdown
                        break;
                    }
                    let _ = wake_tx.send(());
                }
                Ok(None) => {
                    // Non-blocking, no connection waiting
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    warn!("Socket accept error: {}", e);
                    break;
                }
            }
        }
    })
}

/// Accept a task from the socket listener (non-blocking).
///
/// Returns the task content and the directory path for the response.
/// Accept a socket task and return the content and stream.
///
/// The stream is kept alive so we can send the response back later.
fn accept_socket_task(listener: &Listener) -> io::Result<Option<(String, Stream)>> {
    use std::io::{BufRead, BufReader, Read};

    match listener.accept() {
        Ok(stream) => {
            // We need to read from stream but also keep it for sending response.
            // BufReader borrows, so we use a reference and read into it.
            let mut reader = BufReader::new(&stream);

            let mut len_line = String::new();
            reader.read_line(&mut len_line)?;

            let len: usize = match len_line.trim().parse() {
                Ok(n) => n,
                Err(e) => {
                    warn!("invalid length prefix: {}", e);
                    return Ok(None);
                }
            };

            let mut content = vec![0u8; len];
            reader.read_exact(&mut content)?;

            let content = match String::from_utf8(content) {
                Ok(s) => s,
                Err(e) => {
                    warn!("invalid UTF-8 in socket submission: {}", e);
                    return Ok(None);
                }
            };

            // Drop reader to release borrow, then return owned stream
            drop(reader);
            Ok(Some((content, stream)))
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(io::Error::new(
            e.kind(),
            format!("socket accept failed: {e}"),
        )),
    }
}

// =============================================================================
// Payload Resolution
// =============================================================================

/// Resolve a payload to its content.
///
/// For inline payloads, returns the content directly.
/// For file references, reads the file and returns its contents.
fn resolve_payload(raw: &str) -> io::Result<String> {
    let payload: Payload = serde_json::from_str(raw)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    match payload {
        Payload::Inline { content } => Ok(content),
        Payload::FileReference { path } => fs::read_to_string(path),
    }
}

// =============================================================================
// Setup Helpers
// =============================================================================

fn create_socket_listener(socket_path: &Path) -> io::Result<Listener> {
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

fn create_fs_watcher(
    root: &Path,
    wake_tx: mpsc::Sender<()>,
) -> io::Result<(RecommendedWatcher, mpsc::Receiver<notify::Event>)> {
    let (tx, rx) = mpsc::channel();

    let config = notify::Config::default().with_poll_interval(Duration::from_millis(100));

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
                let _ = wake_tx.send(()); // Wake main loop
            }
        },
        config,
    )
    .map_err(io::Error::other)?;

    // Watch root recursively to catch both agents/ and pending/ events
    watcher
        .watch(root, RecursiveMode::Recursive)
        .map_err(io::Error::other)?;

    Ok((watcher, rx))
}

struct SocketCleanup(PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::core::{ExternalTaskId, TaskId};
    use super::*;
    use tempfile::TempDir;

    /// Helper to create external task IDs in tests.
    fn ext(id: u32) -> TaskId {
        TaskId::External(ExternalTaskId(id))
    }

    #[test]
    fn daemon_config_converts_to_io_config() {
        let daemon_config = DaemonConfig {
            idle_agent_timeout: Duration::from_secs(120),
            default_task_timeout: Duration::from_secs(600),
            immediate_heartbeat_enabled: false,
            periodic_heartbeat_enabled: false,
        };

        let io_config: IoConfig = daemon_config.into();

        assert_eq!(io_config.idle_agent_timeout, Duration::from_secs(120));
        assert_eq!(io_config.default_task_timeout, Duration::from_secs(600));
        assert!(!io_config.immediate_heartbeat_enabled);
        assert!(!io_config.periodic_heartbeat_enabled);
    }

    #[test]
    fn scan_agents_registers_existing_directories() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        // Create some agent directories
        fs::create_dir(agents_dir.join("agent-1")).unwrap();
        fs::create_dir(agents_dir.join("agent-2")).unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        let kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        scan_agents(&agents_dir, &events_tx, &mut agent_map, &kicked_paths, &mut task_id_allocator, &io_config).unwrap();

        // Should have received two AgentRegistered events (with heartbeat task IDs)
        let mut events = vec![];
        while let Ok(event) = events_rx.try_recv() {
            events.push(event);
        }

        assert_eq!(events.len(), 2);
        for event in events {
            assert!(matches!(event, Event::AgentRegistered { heartbeat_task_id: Some(_), .. }));
        }
    }

    // =========================================================================
    // handle_agent_dir tests
    // =========================================================================

    #[test]
    fn handle_agent_dir_registers_new_directory() {
        let tmp = TempDir::new().unwrap();
        let agent_path = tmp.path().join("agent-1");
        fs::create_dir(&agent_path).unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        let mut kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        handle_agent_dir(&agent_path, &events_tx, &mut agent_map, &mut kicked_paths, &mut task_id_allocator, &io_config);

        let event = events_rx.try_recv().unwrap();
        assert!(matches!(event, Event::AgentRegistered { agent_id, heartbeat_task_id: Some(_) } if agent_id == AgentId(0)));
        assert!(agent_map.get_id_by_path(&agent_path).is_some());
    }

    #[test]
    fn handle_agent_dir_ignores_already_registered() {
        let tmp = TempDir::new().unwrap();
        let agent_path = tmp.path().join("agent-1");
        fs::create_dir(&agent_path).unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        let mut kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        // Register once
        handle_agent_dir(&agent_path, &events_tx, &mut agent_map, &mut kicked_paths, &mut task_id_allocator, &io_config);
        let _ = events_rx.try_recv().unwrap();

        // Second call should not emit event
        handle_agent_dir(&agent_path, &events_tx, &mut agent_map, &mut kicked_paths, &mut task_id_allocator, &io_config);
        assert!(events_rx.try_recv().is_err());
    }

    #[test]
    fn handle_agent_dir_ignores_kicked_agent() {
        let tmp = TempDir::new().unwrap();
        let agent_path = tmp.path().join("agent-1");
        fs::create_dir(&agent_path).unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        let mut kicked_paths = HashSet::new();
        kicked_paths.insert(agent_path.clone());
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        handle_agent_dir(&agent_path, &events_tx, &mut agent_map, &mut kicked_paths, &mut task_id_allocator, &io_config);

        assert!(events_rx.try_recv().is_err());
        assert!(agent_map.get_id_by_path(&agent_path).is_none());
    }

    #[test]
    fn handle_agent_dir_deregisters_deleted_directory() {
        let tmp = TempDir::new().unwrap();
        let agent_path = tmp.path().join("agent-1");
        fs::create_dir(&agent_path).unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        let mut kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        // Register first
        handle_agent_dir(&agent_path, &events_tx, &mut agent_map, &mut kicked_paths, &mut task_id_allocator, &io_config);
        let _ = events_rx.try_recv().unwrap();

        // Delete and handle again
        fs::remove_dir(&agent_path).unwrap();
        handle_agent_dir(&agent_path, &events_tx, &mut agent_map, &mut kicked_paths, &mut task_id_allocator, &io_config);

        let event = events_rx.try_recv().unwrap();
        assert!(matches!(event, Event::AgentDeregistered { agent_id } if agent_id == AgentId(0)));
    }

    #[test]
    fn handle_agent_dir_clears_kicked_on_delete() {
        let tmp = TempDir::new().unwrap();
        let agent_path = tmp.path().join("agent-1");
        // Don't create the directory - simulating deletion

        let (events_tx, _events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        let mut kicked_paths = HashSet::new();
        kicked_paths.insert(agent_path.clone());
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        handle_agent_dir(&agent_path, &events_tx, &mut agent_map, &mut kicked_paths, &mut task_id_allocator, &io_config);

        assert!(!kicked_paths.contains(&agent_path));
    }

    #[test]
    fn handle_agent_dir_ignores_file_not_directory() {
        let tmp = TempDir::new().unwrap();
        let agent_path = tmp.path().join("agent-1");
        fs::write(&agent_path, "not a directory").unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        let mut kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        handle_agent_dir(&agent_path, &events_tx, &mut agent_map, &mut kicked_paths, &mut task_id_allocator, &io_config);

        assert!(events_rx.try_recv().is_err());
    }

    // =========================================================================
    // handle_agent_response tests
    // =========================================================================

    #[test]
    fn handle_agent_response_sends_event_for_known_agent() {
        let tmp = TempDir::new().unwrap();
        let agent_path = tmp.path().join("agent-1");
        fs::create_dir(&agent_path).unwrap();
        let response_path = agent_path.join("response.json");
        fs::write(&response_path, "{}").unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        agent_map.register_directory(agent_path.clone(), ()).unwrap();
        let mut pending_responses = HashSet::new();
        let kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        handle_agent_response(&agent_path, &response_path, &events_tx, &mut agent_map, &mut pending_responses, &kicked_paths, &mut task_id_allocator, &io_config);

        let event = events_rx.try_recv().unwrap();
        assert!(matches!(event, Event::AgentResponded { agent_id } if agent_id == AgentId(0)));
    }

    #[test]
    fn handle_agent_response_registers_unknown_agent() {
        let tmp = TempDir::new().unwrap();
        let agent_path = tmp.path().join("agent-1");
        fs::create_dir(&agent_path).unwrap();
        let response_path = agent_path.join("response.json");
        fs::write(&response_path, "{}").unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        let mut pending_responses = HashSet::new();
        let kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        handle_agent_response(&agent_path, &response_path, &events_tx, &mut agent_map, &mut pending_responses, &kicked_paths, &mut task_id_allocator, &io_config);

        // Should get both registration and response events
        let events: Vec<_> = std::iter::from_fn(|| events_rx.try_recv().ok()).collect();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], Event::AgentRegistered { .. }));
        assert!(matches!(events[1], Event::AgentResponded { .. }));
    }

    #[test]
    fn handle_agent_response_deduplicates_events() {
        let tmp = TempDir::new().unwrap();
        let agent_path = tmp.path().join("agent-1");
        fs::create_dir(&agent_path).unwrap();
        let response_path = agent_path.join("response.json");
        fs::write(&response_path, "{}").unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        agent_map.register_directory(agent_path.clone(), ()).unwrap();
        let mut pending_responses = HashSet::new();
        let kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        // Call twice
        handle_agent_response(&agent_path, &response_path, &events_tx, &mut agent_map, &mut pending_responses, &kicked_paths, &mut task_id_allocator, &io_config);
        handle_agent_response(&agent_path, &response_path, &events_tx, &mut agent_map, &mut pending_responses, &kicked_paths, &mut task_id_allocator, &io_config);

        // Should only get one event
        let events: Vec<_> = std::iter::from_fn(|| events_rx.try_recv().ok()).collect();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn handle_agent_response_ignores_kicked_agent() {
        let tmp = TempDir::new().unwrap();
        let agent_path = tmp.path().join("agent-1");
        fs::create_dir(&agent_path).unwrap();
        let response_path = agent_path.join("response.json");
        fs::write(&response_path, "{}").unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        let mut pending_responses = HashSet::new();
        let mut kicked_paths = HashSet::new();
        kicked_paths.insert(agent_path.clone());
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        handle_agent_response(&agent_path, &response_path, &events_tx, &mut agent_map, &mut pending_responses, &kicked_paths, &mut task_id_allocator, &io_config);

        assert!(events_rx.try_recv().is_err());
    }

    #[test]
    fn handle_agent_response_ignores_missing_file() {
        let tmp = TempDir::new().unwrap();
        let agent_path = tmp.path().join("agent-1");
        fs::create_dir(&agent_path).unwrap();
        let response_path = agent_path.join("response.json");
        // Don't create the file

        let (events_tx, events_rx) = mpsc::channel();
        let mut agent_map = AgentMap::new();
        agent_map.register_directory(agent_path.clone(), ()).unwrap();
        let mut pending_responses = HashSet::new();
        let kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        handle_agent_response(&agent_path, &response_path, &events_tx, &mut agent_map, &mut pending_responses, &kicked_paths, &mut task_id_allocator, &io_config);

        assert!(events_rx.try_recv().is_err());
    }

    // =========================================================================
    // register_pending_task tests
    // =========================================================================

    #[test]
    fn register_pending_task_registers_new_task() {
        let tmp = TempDir::new().unwrap();
        let submission_dir = tmp.path().join("task-1");
        fs::create_dir(&submission_dir).unwrap();
        fs::write(submission_dir.join(TASK_FILE), r#"{"kind": "Inline", "content": "test task"}"#).unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut external_task_map = ExternalTaskMap::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        register_pending_task(&submission_dir, &events_tx, &mut external_task_map, &mut task_id_allocator, &io_config);

        let event = events_rx.try_recv().unwrap();
        assert!(matches!(event, Event::TaskSubmitted { task_id } if task_id == ext(0)));
        assert!(external_task_map.get_id_by_path(&submission_dir).is_some());
    }

    #[test]
    fn register_pending_task_ignores_already_registered() {
        let tmp = TempDir::new().unwrap();
        let submission_dir = tmp.path().join("task-1");
        fs::create_dir(&submission_dir).unwrap();
        fs::write(submission_dir.join(TASK_FILE), r#"{"kind": "Inline", "content": "test task"}"#).unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut external_task_map = ExternalTaskMap::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        // Register once
        register_pending_task(&submission_dir, &events_tx, &mut external_task_map, &mut task_id_allocator, &io_config);
        let _ = events_rx.try_recv().unwrap();

        // Second call should not emit event
        register_pending_task(&submission_dir, &events_tx, &mut external_task_map, &mut task_id_allocator, &io_config);
        assert!(events_rx.try_recv().is_err());
    }

    #[test]
    fn register_pending_task_ignores_completed_task() {
        let tmp = TempDir::new().unwrap();
        let submission_dir = tmp.path().join("task-1");
        fs::create_dir(&submission_dir).unwrap();
        fs::write(submission_dir.join(TASK_FILE), r#"{"kind": "Inline", "content": "test task"}"#).unwrap();
        fs::write(submission_dir.join(crate::constants::RESPONSE_FILE), r#"{"done": true}"#).unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut external_task_map = ExternalTaskMap::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        register_pending_task(&submission_dir, &events_tx, &mut external_task_map, &mut task_id_allocator, &io_config);

        assert!(events_rx.try_recv().is_err());
    }

    #[test]
    fn register_pending_task_ignores_missing_task_file() {
        let tmp = TempDir::new().unwrap();
        let submission_dir = tmp.path().join("task-1");
        fs::create_dir(&submission_dir).unwrap();
        // Don't create task.json

        let (events_tx, events_rx) = mpsc::channel();
        let mut external_task_map = ExternalTaskMap::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        register_pending_task(&submission_dir, &events_tx, &mut external_task_map, &mut task_id_allocator, &io_config);

        assert!(events_rx.try_recv().is_err());
    }

    // =========================================================================
    // Event loop tests (events → step → effects)
    // =========================================================================

    use super::super::core::{AgentId, Effect, PoolState, step};
    use std::thread;
    use tracing::{debug, trace};

    /// Run the event loop until the events channel closes.
    fn run_event_loop(
        events_rx: mpsc::Receiver<Event>,
        effects_tx: mpsc::Sender<Effect>,
    ) -> PoolState {
        run_event_loop_with_state(PoolState::new(), events_rx, effects_tx)
    }

    /// Run the event loop with an initial state.
    fn run_event_loop_with_state(
        mut state: PoolState,
        events_rx: mpsc::Receiver<Event>,
        effects_tx: mpsc::Sender<Effect>,
    ) -> PoolState {
        debug!("event loop starting");

        while let Ok(event) = events_rx.recv() {
            trace!(?event, "received event");

            let (new_state, effects) = step(state, event);
            state = new_state;

            for effect in effects {
                trace!(?effect, "emitting effect");
                let _ = effects_tx.send(effect);
            }
        }

        debug!(
            pending = state.pending_count(),
            agents = state.agent_count(),
            "event loop stopped"
        );

        state
    }

    #[test]
    fn event_loop_processes_events_and_emits_effects() {
        let (events_tx, events_rx) = mpsc::channel();
        let (effects_tx, effects_rx) = mpsc::channel();

        let handle = thread::spawn(move || run_event_loop(events_rx, effects_tx));

        events_tx
            .send(Event::AgentRegistered {
                agent_id: AgentId(1),
                heartbeat_task_id: None,
            })
            .unwrap();

        let effect = effects_rx.recv().unwrap();
        assert!(matches!(effect, Effect::AgentIdled { .. }));

        events_tx
            .send(Event::TaskSubmitted {
                task_id: ext(42),
            })
            .unwrap();

        let effect = effects_rx.recv().unwrap();
        assert!(matches!(
            effect,
            Effect::TaskAssigned { task_id, .. } if task_id == ext(42)
        ));

        drop(events_tx);

        let final_state = handle.join().unwrap();
        assert_eq!(final_state.agent_count(), 1);
        assert_eq!(final_state.pending_count(), 0);
    }

    #[test]
    fn event_loop_handles_channel_close_gracefully() {
        let (events_tx, events_rx) = mpsc::channel();
        let (effects_tx, effects_rx) = mpsc::channel();

        let handle = thread::spawn(move || run_event_loop(events_rx, effects_tx));

        drop(effects_rx);

        events_tx
            .send(Event::AgentRegistered {
                agent_id: AgentId(1),
                heartbeat_task_id: None,
            })
            .unwrap();

        drop(events_tx);

        let final_state = handle.join().unwrap();
        assert_eq!(final_state.agent_count(), 1);
    }

    #[test]
    fn event_loop_uses_initial_state() {
        let (events_tx, events_rx) = mpsc::channel();
        let (effects_tx, effects_rx) = mpsc::channel();

        let initial_state = PoolState::new();

        let handle =
            thread::spawn(move || run_event_loop_with_state(initial_state, events_rx, effects_tx));

        events_tx
            .send(Event::AgentRegistered {
                agent_id: AgentId(1),
                heartbeat_task_id: None,
            })
            .unwrap();

        let _ = effects_rx.recv().unwrap();

        drop(events_tx);
        let final_state = handle.join().unwrap();
        assert_eq!(final_state.agent_count(), 1);
    }
}
