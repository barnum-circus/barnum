//! Daemon - Wires together the core state machine and I/O.
//!
//! This module creates the event and effect channels, spawns the event loop,
//! and runs the I/O layer that handles filesystem events, socket connections,
//! and effect execution.

use std::convert::Infallible;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use interprocess::local_socket::{
    GenericFilePath, Listener, ListenerNonblockingMode, ListenerOptions, Stream, prelude::*,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, info, trace, warn};

use std::collections::HashSet;

use crate::client::Payload;
use crate::constants::{
    AGENTS_DIR, LOCK_FILE, PENDING_DIR, REQUEST_SUFFIX, SOCKET_NAME, STATUS_FILE, TASK_FILE,
};
use crate::lock::{LockGuard, acquire_lock};

use super::core::{AgentId, Effect, Event, TaskId};
use super::io::{
    AgentMap, ExternalTaskData, ExternalTaskMap, IoConfig, TaskIdAllocator, execute_effect,
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
// Unified Event Type
// =============================================================================

/// Unified event type for all I/O sources.
///
/// Instead of multiple channels plus a wake channel, all event sources send to a single
/// channel. The main loop blocks on `recv()`. Shutdown is signaled by `Shutdown` variant.
enum IoEvent {
    /// Filesystem event from notify watcher.
    Fs(notify::Event),
    /// Socket connection with task payload.
    Socket(String, Stream),
    /// Effect from the core event loop.
    Effect(Effect),
    /// Shutdown signal - exit the I/O loop.
    Shutdown,
}

// =============================================================================
// Public API
// =============================================================================

/// Handle to a running daemon, allowing graceful shutdown.
pub struct DaemonHandle {
    /// Dropping this sender closes the channel, signaling shutdown.
    _shutdown_tx: mpsc::Sender<IoEvent>,
    thread: Option<thread::JoinHandle<io::Result<()>>>,
}

impl DaemonHandle {
    /// Request graceful shutdown and wait for the daemon to stop.
    ///
    /// Sends a shutdown signal through the channel, which causes the daemon's
    /// I/O loop to exit. Then we join the thread.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon thread panicked or encountered an I/O error.
    pub fn shutdown(self) -> io::Result<()> {
        let Self {
            _shutdown_tx: shutdown_tx,
            thread,
        } = self;
        // Send explicit shutdown signal (can't rely on channel closing because
        // timer threads hold sender clones that keep the channel alive)
        let _ = shutdown_tx.send(IoEvent::Shutdown);

        if let Some(handle) = thread {
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

    // TODO: Add clean option to clear stale agent/pending directories on restart.
    // See run_with_config for details.

    let lock_path = root.join(LOCK_FILE);
    let socket_path = root.join(SOCKET_NAME);
    let agents_dir = root.join(AGENTS_DIR);
    let pending_dir = root.join(PENDING_DIR);

    // Clean up stale socket if it exists
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }

    // Create unified event channel - all sources send here, main loop receives
    let (io_tx, io_rx) = mpsc::channel();

    // Start watcher FIRST - before creating anything - so we can verify all FS events
    let fs_watcher = create_fs_watcher(&root, io_tx.clone())?;

    // Use a oneshot channel to signal readiness or early error
    let (ready_tx, ready_rx) = mpsc::sync_channel::<io::Result<()>>(0);

    // Clone sender for the daemon thread
    let daemon_io_tx = io_tx.clone();

    let thread = thread::spawn(move || {
        let _watcher = fs_watcher;

        // Create everything and verify all FS events are seen
        let (lock, listener) =
            match sync_and_setup(&lock_path, &socket_path, &pending_dir, &agents_dir, &io_rx) {
                Ok(result) => result,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return Err(io::Error::other("sync_and_setup failed"));
                }
            };

        let _lock = lock;
        let _cleanup = SocketCleanup(socket_path.clone());

        // Write status file to signal daemon is ready
        if let Err(e) = fs::write(root.join(STATUS_FILE), "ready") {
            let _ = ready_tx.send(Err(e));
            return Err(io::Error::other("failed to write status file"));
        }

        info!(socket = %socket_path.display(), "daemon listening");

        // Signal that we're ready
        let _ = ready_tx.send(Ok(()));

        run_daemon(
            listener,
            io_rx,
            daemon_io_tx,
            &agents_dir,
            &pending_dir,
            &config.into(),
        )
    });

    // Wait for daemon to signal readiness (blocking, no polling)
    // Propagate any early error
    ready_rx
        .recv()
        .map_err(|_| io::Error::other("daemon thread died during startup"))??;

    Ok(DaemonHandle {
        _shutdown_tx: io_tx,
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

    // TODO: Add --clean flag to clear stale agent/pending directories on restart.
    // Currently, leftover directories from previous runs are picked up by the scan
    // and treated as live agents, which causes tasks to be assigned to non-existent agents.

    let lock_path = root.join(LOCK_FILE);
    let socket_path = root.join(SOCKET_NAME);
    let agents_dir = root.join(AGENTS_DIR);
    let pending_dir = root.join(PENDING_DIR);

    // Clean up stale socket if it exists
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }

    // Create unified event channel
    let (io_tx, io_rx) = mpsc::channel();

    // Start watcher FIRST - before creating anything - so we can verify all FS events
    let _fs_watcher = create_fs_watcher(&root, io_tx.clone())?;

    // Now create everything and verify we see all the events
    let (_lock, listener) =
        sync_and_setup(&lock_path, &socket_path, &pending_dir, &agents_dir, &io_rx)?;

    let _cleanup = SocketCleanup(socket_path.clone());

    // Write status file to signal daemon is ready
    fs::write(root.join(STATUS_FILE), "ready")?;

    info!(socket = %socket_path.display(), "daemon listening");

    let io_config = config.into();
    match run_daemon(
        listener,
        io_rx,
        io_tx,
        &agents_dir,
        &pending_dir,
        &io_config,
    ) {
        Ok(()) => unreachable!("event loop returned without shutdown signal"),
        Err(e) => Err(e),
    }
}

// =============================================================================
// Main Daemon Loop
// =============================================================================

/// The main daemon function that orchestrates core and I/O.
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn run_daemon(
    listener: Listener,
    io_rx: mpsc::Receiver<IoEvent>,
    io_tx: mpsc::Sender<IoEvent>,
    agents_dir: &Path,
    pending_dir: &Path,
    io_config: &IoConfig,
) -> io::Result<()> {
    // Create channel for core events (from I/O to event loop)
    let (events_tx, events_rx) = mpsc::channel::<Event>();

    // Spawn socket accept thread - sends IoEvent::Socket to unified channel
    let socket_io_tx = io_tx.clone();
    let _socket_thread = spawn_socket_accept_thread(listener, socket_io_tx);

    // I/O state
    let mut agent_map = AgentMap::new();
    let mut external_task_map = ExternalTaskMap::new();
    let mut task_id_allocator = TaskIdAllocator::new();
    // Track agents with pending responses to deduplicate FSWatcher events
    let mut pending_responses: HashSet<AgentId> = HashSet::new();

    // Track kicked agent paths to reject re-registration attempts
    let mut kicked_paths: HashSet<PathBuf> = HashSet::new();

    // Track submission UUIDs we've already seen to deduplicate FS events
    let mut seen_submissions: HashSet<String> = HashSet::new();

    // Spawn event loop in a separate thread - sends IoEvent::Effect to unified channel
    let event_loop_handle = thread::spawn(move || run_event_loop(events_rx, io_tx));

    // Run the I/O loop - receives from unified channel
    let result = io_loop(
        io_rx,
        &events_tx,
        &mut agent_map,
        &mut external_task_map,
        &mut task_id_allocator,
        &mut pending_responses,
        &mut kicked_paths,
        &mut seen_submissions,
        agents_dir,
        pending_dir,
        io_config,
    );

    // Wait for event loop to finish (it exits when channel closes)
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

/// Run the core event loop.
///
/// Receives core events, runs the state machine, and sends effects
/// to the unified I/O channel. Exits when the events channel closes.
#[allow(clippy::needless_pass_by_value)] // We take ownership intentionally - runs in spawned thread
fn run_event_loop(
    events_rx: mpsc::Receiver<Event>,
    io_tx: mpsc::Sender<IoEvent>,
) -> super::core::PoolState {
    use super::core::{PoolState, step};

    let mut state = PoolState::new();

    // Block on recv - Shutdown event signals exit
    while let Ok(event) = events_rx.recv() {
        // Check for shutdown before processing
        if matches!(event, Event::Shutdown) {
            debug!("event loop: received shutdown signal");
            break;
        }

        trace!(?event, "received event");
        let (new_state, effects) = step(state, event);
        state = new_state;

        for effect in effects {
            trace!(?effect, "emitting effect");
            // Send effect to unified I/O channel
            if io_tx.send(IoEvent::Effect(effect)).is_err() {
                // I/O loop is gone, exit
                debug!("event loop: I/O channel closed");
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

/// The I/O loop that handles all events from the unified channel.
///
/// Blocks on `recv()` from the unified `IoEvent` channel. Channel closing signals shutdown.
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn io_loop(
    io_rx: mpsc::Receiver<IoEvent>,
    events_tx: &mpsc::Sender<Event>,
    agent_map: &mut AgentMap,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    pending_responses: &mut HashSet<AgentId>,
    kicked_paths: &mut HashSet<PathBuf>,
    seen_submissions: &mut HashSet<String>,
    agents_dir: &Path,
    pending_dir: &Path,
    io_config: &IoConfig,
) -> io::Result<()> {
    debug!(
        "io_loop starting, agents_dir={:?}, pending_dir={:?}",
        agents_dir, pending_dir
    );

    // Block on recv - Shutdown event signals exit
    while let Ok(io_event) = io_rx.recv() {
        match io_event {
            IoEvent::Fs(event) => {
                debug!(kind = ?event.kind, paths = ?event.paths, "io_loop: fs event");
                handle_fs_event(
                    &event,
                    events_tx,
                    agent_map,
                    external_task_map,
                    task_id_allocator,
                    pending_responses,
                    kicked_paths,
                    seen_submissions,
                    agents_dir,
                    pending_dir,
                    io_config,
                );
            }
            IoEvent::Socket(raw, stream) => {
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
            IoEvent::Effect(effect) => {
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
            IoEvent::Shutdown => {
                info!("shutdown signal received");
                // Signal event loop to exit (can't rely on channel closing because
                // timer threads hold events_tx clones)
                let _ = events_tx.send(Event::Shutdown);
                break;
            }
        }
    }

    info!("I/O loop exiting");
    Ok(())
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
    seen_submissions: &mut HashSet<String>,
    agents_dir: &Path,
    pending_dir: &Path,
    io_config: &IoConfig,
) {
    trace!(kind = ?event.kind, paths = ?event.paths, "fs event");

    for path in &event.paths {
        let Some(category) = path_category::categorize(path, event.kind, agents_dir, pending_dir)
        else {
            trace!(?path, "path did not match any category");
            continue;
        };
        debug!(?path, ?category, "categorized path");

        match category {
            PathCategory::AgentDir { name } => {
                let agent_path = agents_dir.join(&name);
                handle_agent_dir(
                    &agent_path,
                    events_tx,
                    agent_map,
                    kicked_paths,
                    task_id_allocator,
                    io_config,
                );
            }
            PathCategory::AgentResponse { name } => {
                let agent_path = agents_dir.join(&name);
                handle_agent_response(
                    &agent_path,
                    path,
                    events_tx,
                    agent_map,
                    pending_responses,
                    kicked_paths,
                    task_id_allocator,
                    io_config,
                );
            }
            PathCategory::SubmissionRequest { id } => {
                // Skip if file doesn't exist (may have been cleaned up already)
                if !path.exists() {
                    trace!(id = %id, "SubmissionRequest: file doesn't exist, skipping");
                    return;
                }
                register_submission(
                    &id,
                    pending_dir,
                    events_tx,
                    external_task_map,
                    task_id_allocator,
                    seen_submissions,
                    io_config,
                );
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
            // Remove from agent_map immediately to prevent races where core
            // assigns a task to an agent whose directory is already gone.
            agent_map.remove(agent_id);
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
#[allow(clippy::too_many_arguments)]
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
    if agent_map.get_id_by_path(agent_path).is_none()
        && let Some(agent_id) = agent_map.register_directory(agent_path.to_path_buf(), ())
    {
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

    // Send response event (deduplicated)
    if let Some(agent_id) = agent_map.get_id_by_path(agent_path) {
        if pending_responses.insert(agent_id) {
            let _ = events_tx.send(Event::AgentResponded { agent_id });
        } else {
            trace!(agent_id = agent_id.0, "skipping duplicate AgentResponded");
        }
    }
}

/// Register a submission from a request file.
fn register_submission(
    id: &str,
    pending_dir: &Path,
    events_tx: &mpsc::Sender<Event>,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    seen_submissions: &mut HashSet<String>,
    io_config: &IoConfig,
) {
    // Deduplicate by UUID - once we've seen a submission ID, ignore later events
    if !seen_submissions.insert(id.to_string()) {
        trace!(id = %id, "SubmissionRequest: already seen, skipping");
        return;
    }

    let request_path = pending_dir.join(format!("{id}{REQUEST_SUFFIX}"));

    // Read and resolve payload
    let raw = match fs::read_to_string(&request_path) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %request_path.display(), error = %e, "failed to read submission request");
            return;
        }
    };

    let content = match resolve_payload(&raw) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %request_path.display(), error = %e, "failed to resolve payload");
            return;
        }
    };

    // Register the submission
    let external_id = task_id_allocator.allocate_external();
    if external_task_map.register(
        external_id,
        request_path,
        ExternalTaskData {
            content,
            timeout: io_config.default_task_timeout,
        },
    ) {
        info!(
            external_task_id = external_id.0,
            "file-based task registered"
        );
        let _ = events_tx.send(Event::TaskSubmitted {
            task_id: TaskId::External(external_id),
        });
    }
}

// =============================================================================
// Periodic Scans
// =============================================================================

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

/// Spawn a thread that accepts socket connections and sends them to the unified channel.
fn spawn_socket_accept_thread(
    listener: Listener,
    io_tx: mpsc::Sender<IoEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            match accept_socket_task(&listener) {
                Ok(Some((raw, stream))) => {
                    if io_tx.send(IoEvent::Socket(raw, stream)).is_err() {
                        // Receiver dropped, shutdown
                        break;
                    }
                }
                Ok(None) => {
                    // Non-blocking, no connection waiting
                    std::thread::sleep(std::time::Duration::from_millis(10));
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
    let payload: Payload =
        serde_json::from_str(raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

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

fn create_fs_watcher(root: &Path, io_tx: mpsc::Sender<IoEvent>) -> io::Result<RecommendedWatcher> {
    let config =
        notify::Config::default().with_poll_interval(std::time::Duration::from_millis(100));

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                // Send all events - categorize() filters for meaningful ones
                let _ = io_tx.send(IoEvent::Fs(event));
            }
        },
        config,
    )
    .map_err(io::Error::other)?;

    // Watch root recursively to catch both agents/ and pending/ events
    watcher
        .watch(root, RecursiveMode::Recursive)
        .map_err(io::Error::other)?;

    Ok(watcher)
}

/// Set up the daemon and verify the watcher sees events for key directories.
///
/// This function:
/// 1. Creates the lock file and acquires the lock
/// 2. Creates the socket and starts listening
/// 3. Creates both directories (pending, agents)
/// 4. Writes canary files to verify watcher sees events in subdirectories
/// 5. Waits until we see events for the directories and canary files
/// 6. Cleans up canary files
///
/// We specifically verify events for `pending_dir`, `agents_dir`, and canary files
/// because these are where agents and submissions will interact. Other events
/// (lock file, socket, etc.) are allowlisted but not required.
///
/// Returns the lock guard and socket listener on success.
///
/// # Panics
///
/// Panics if an unexpected non-FS event is received.
#[allow(clippy::panic)] // Intentional: non-FS events during startup indicate a bug
fn sync_and_setup(
    lock_path: &Path,
    socket_path: &Path,
    pending_dir: &Path,
    agents_dir: &Path,
    io_rx: &mpsc::Receiver<IoEvent>,
) -> io::Result<(LockGuard, Listener)> {
    use std::collections::HashSet;

    const POLL_TIMEOUT: Duration = Duration::from_millis(100);
    const MAX_DURATION: Duration = Duration::from_secs(5);

    let pending_canary = pending_dir.join("canary");
    let agents_canary = agents_dir.join("canary");

    // Paths we MUST see events for (verifies watcher is working for key directories)
    // Only canary files - seeing these proves the watcher sees events IN those directories
    let mut required: HashSet<PathBuf> = HashSet::new();
    required.insert(pending_canary.clone());
    required.insert(agents_canary.clone());

    // Paths we allow events for but don't require
    let mut allowed: HashSet<PathBuf> = HashSet::new();
    allowed.insert(lock_path.to_path_buf());
    allowed.insert(socket_path.to_path_buf());
    allowed.insert(pending_dir.to_path_buf());
    allowed.insert(agents_dir.to_path_buf());

    // Track which required paths we've seen
    let mut seen: HashSet<PathBuf> = HashSet::new();

    debug!(
        "waiting for watcher sync, requiring {} events: {:?}",
        required.len(),
        required
    );

    // Create lock and socket first (before watcher matters)
    let lock = acquire_lock(lock_path)?;
    let listener = create_socket_listener(socket_path)?;

    // Create directories and canaries - watcher MUST see these
    fs::create_dir_all(pending_dir)?;
    fs::create_dir_all(agents_dir)?;
    fs::write(&pending_canary, "sync")?;
    fs::write(&agents_canary, "sync")?;

    let start = std::time::Instant::now();
    while start.elapsed() < MAX_DURATION {
        match io_rx.recv_timeout(POLL_TIMEOUT) {
            Ok(IoEvent::Fs(event)) => {
                for path in &event.paths {
                    if required.contains(path) {
                        seen.insert(path.clone());
                        debug!(
                            "sync: seen {}/{} - {} ({:?})",
                            seen.len(),
                            required.len(),
                            path.display(),
                            event.kind
                        );
                    } else if allowed.contains(path) {
                        // Allowlisted but not required - just log
                        debug!("sync: allowed {} ({:?})", path.display(), event.kind);
                    } else {
                        panic!(
                            "unexpected FS event during startup sync: {:?} for path {}\n\
                             Required: {:?}\n\
                             Allowed: {:?}",
                            event.kind,
                            path.display(),
                            required,
                            allowed
                        );
                    }
                }
                if seen.len() == required.len() {
                    debug!(
                        "watcher sync complete - all {} required events seen",
                        required.len()
                    );
                    let _ = fs::remove_file(&pending_canary);
                    let _ = fs::remove_file(&agents_canary);
                    return Ok((lock, listener));
                }
            }
            Ok(_) => panic!("unexpected non-FS event during startup sync"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // On timeout, rewrite canaries to trigger new events (in case we missed them)
                if !seen.contains(&pending_canary) {
                    fs::write(&pending_canary, "sync-retry")?;
                }
                if !seen.contains(&agents_canary) {
                    fs::write(&agents_canary, "sync-retry")?;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = fs::remove_file(&pending_canary);
                let _ = fs::remove_file(&agents_canary);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher channel disconnected",
                ));
            }
        }
    }

    // Log what we're missing for debugging
    let missing: Vec<_> = required.difference(&seen).collect();
    warn!("watcher sync timed out, missing events for: {:?}", missing);

    let _ = fs::remove_file(&pending_canary);
    let _ = fs::remove_file(&agents_canary);
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("watcher sync timed out, missing {} events", missing.len()),
    ))
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

        handle_agent_dir(
            &agent_path,
            &events_tx,
            &mut agent_map,
            &mut kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );

        let event = events_rx.try_recv().unwrap();
        assert!(
            matches!(event, Event::AgentRegistered { agent_id, heartbeat_task_id: Some(_) } if agent_id == AgentId(0))
        );
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
        handle_agent_dir(
            &agent_path,
            &events_tx,
            &mut agent_map,
            &mut kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );
        let _ = events_rx.try_recv().unwrap();

        // Second call should not emit event
        handle_agent_dir(
            &agent_path,
            &events_tx,
            &mut agent_map,
            &mut kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );
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

        handle_agent_dir(
            &agent_path,
            &events_tx,
            &mut agent_map,
            &mut kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );

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
        handle_agent_dir(
            &agent_path,
            &events_tx,
            &mut agent_map,
            &mut kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );
        let _ = events_rx.try_recv().unwrap();

        // Delete and handle again
        fs::remove_dir(&agent_path).unwrap();
        handle_agent_dir(
            &agent_path,
            &events_tx,
            &mut agent_map,
            &mut kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );

        let event = events_rx.try_recv().unwrap();
        assert!(matches!(event, Event::AgentDeregistered { agent_id } if agent_id == AgentId(0)));
        // Agent should be removed from agent_map immediately to prevent race conditions
        assert!(agent_map.get_id_by_path(&agent_path).is_none());
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

        handle_agent_dir(
            &agent_path,
            &events_tx,
            &mut agent_map,
            &mut kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );

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

        handle_agent_dir(
            &agent_path,
            &events_tx,
            &mut agent_map,
            &mut kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );

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
        agent_map
            .register_directory(agent_path.clone(), ())
            .unwrap();
        let mut pending_responses = HashSet::new();
        let kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        handle_agent_response(
            &agent_path,
            &response_path,
            &events_tx,
            &mut agent_map,
            &mut pending_responses,
            &kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );

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

        handle_agent_response(
            &agent_path,
            &response_path,
            &events_tx,
            &mut agent_map,
            &mut pending_responses,
            &kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );

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
        agent_map
            .register_directory(agent_path.clone(), ())
            .unwrap();
        let mut pending_responses = HashSet::new();
        let kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        // Call twice
        handle_agent_response(
            &agent_path,
            &response_path,
            &events_tx,
            &mut agent_map,
            &mut pending_responses,
            &kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );
        handle_agent_response(
            &agent_path,
            &response_path,
            &events_tx,
            &mut agent_map,
            &mut pending_responses,
            &kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );

        // Should only get one event
        assert_eq!(std::iter::from_fn(|| events_rx.try_recv().ok()).count(), 1);
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

        handle_agent_response(
            &agent_path,
            &response_path,
            &events_tx,
            &mut agent_map,
            &mut pending_responses,
            &kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );

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
        agent_map
            .register_directory(agent_path.clone(), ())
            .unwrap();
        let mut pending_responses = HashSet::new();
        let kicked_paths = HashSet::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        handle_agent_response(
            &agent_path,
            &response_path,
            &events_tx,
            &mut agent_map,
            &mut pending_responses,
            &kicked_paths,
            &mut task_id_allocator,
            &io_config,
        );

        assert!(events_rx.try_recv().is_err());
    }

    // =========================================================================
    // register_submission tests
    // =========================================================================

    #[test]
    fn register_submission_registers_new_task() {
        let tmp = TempDir::new().unwrap();
        let pending_dir = tmp.path();
        let id = "task-1";
        fs::write(
            pending_dir.join(format!("{id}.request.json")),
            r#"{"kind": "Inline", "content": "test task"}"#,
        )
        .unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut external_task_map = ExternalTaskMap::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let mut seen_submissions = HashSet::new();
        let io_config = IoConfig::default();

        register_submission(
            id,
            pending_dir,
            &events_tx,
            &mut external_task_map,
            &mut task_id_allocator,
            &mut seen_submissions,
            &io_config,
        );

        let event = events_rx.try_recv().unwrap();
        assert!(matches!(event, Event::TaskSubmitted { task_id } if task_id == ext(0)));
        let request_path = pending_dir.join(format!("{id}.request.json"));
        assert!(external_task_map.get_id_by_path(&request_path).is_some());
    }

    #[test]
    fn register_submission_ignores_already_seen() {
        let tmp = TempDir::new().unwrap();
        let pending_dir = tmp.path();
        let id = "task-1";
        fs::write(
            pending_dir.join(format!("{id}.request.json")),
            r#"{"kind": "Inline", "content": "test task"}"#,
        )
        .unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut external_task_map = ExternalTaskMap::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let mut seen_submissions = HashSet::new();
        let io_config = IoConfig::default();

        // Register once
        register_submission(
            id,
            pending_dir,
            &events_tx,
            &mut external_task_map,
            &mut task_id_allocator,
            &mut seen_submissions,
            &io_config,
        );
        let _ = events_rx.try_recv().unwrap();

        // Second call should not emit event (already in seen_submissions)
        register_submission(
            id,
            pending_dir,
            &events_tx,
            &mut external_task_map,
            &mut task_id_allocator,
            &mut seen_submissions,
            &io_config,
        );
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
    #[allow(clippy::needless_pass_by_value)]
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
            .send(Event::TaskSubmitted { task_id: ext(42) })
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
