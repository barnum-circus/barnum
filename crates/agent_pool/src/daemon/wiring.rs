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

use crate::constants::{
    AGENTS_DIR, LOCK_FILE, REQUEST_SUFFIX, SCRATCH_DIR, SOCKET_NAME, STATUS_FILE, SUBMISSIONS_DIR,
    TASK_FILE,
};
use crate::lock::{LockGuard, acquire_lock};
use crate::submit::Payload;

use super::core::{AgentId, Effect, Event, TaskId};
use super::io::{
    AgentMap, ExternalTaskData, ExternalTaskMap, IoConfig, TaskIdAllocator, execute_effect,
};
use super::path_category::{self, PathCategory};

// =============================================================================
// Pool State Cleanup
// =============================================================================

/// Clean up pool state files.
///
/// Removes:
/// - All files in submissions/ directory
/// - All directories in agents/ directory
/// - The status file
/// - Any canary files
///
/// This is called on startup (to clean up stale state from crashed daemons)
/// and on graceful shutdown.
fn cleanup_pool_state(root: &Path) {
    let status_file = root.join(STATUS_FILE);
    let submissions_dir = root.join(SUBMISSIONS_DIR);
    let agents_dir = root.join(AGENTS_DIR);

    // Remove status file
    if status_file.exists() {
        let _ = fs::remove_file(&status_file);
    }

    // Clean submissions directory (flat files)
    if submissions_dir.exists()
        && let Ok(entries) = fs::read_dir(&submissions_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let _ = fs::remove_file(&path);
            }
        }
    }

    // Clean agents directory (subdirectories)
    if agents_dir.exists()
        && let Ok(entries) = fs::read_dir(&agents_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }

    // Clean up any canary files in the root (daemon.canary, client_canary, UUID.canary, etc.)
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".canary"))
            {
                let _ = fs::remove_file(&path);
            }
        }
    }
    // Also clean up legacy canary files
    for filename in ["canary", "client_canary"] {
        let canary_path = root.join(filename);
        if canary_path.exists() {
            let _ = fs::remove_file(&canary_path);
        }
    }

    debug!("cleaned up pool state");
}

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
            idle_agent_timeout: Duration::from_secs(180),
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
}

// =============================================================================
// Public API
// =============================================================================

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

    // Clean up stale state from previous runs (crashed daemon, etc.)
    cleanup_pool_state(&root);

    let lock_path = root.join(LOCK_FILE);
    let socket_path = root.join(SOCKET_NAME);
    let agents_dir = root.join(AGENTS_DIR);
    let submissions_dir = root.join(SUBMISSIONS_DIR);
    let scratch_dir = root.join(SCRATCH_DIR);

    // Clean up stale socket if it exists
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }

    // Create unified event channel
    let (io_tx, io_rx) = mpsc::channel();

    // Start watcher FIRST - before creating anything - so we can verify all FS events
    let _fs_watcher = create_fs_watcher(&root, io_tx.clone())?;
    // Clean up pool state on exit (SIGTERM or panic)
    let _pool_cleanup = PoolStateCleanup(root.clone());

    // Now create everything and verify we see all the events
    let (_lock, listener) = sync_and_setup(
        &root,
        &lock_path,
        &socket_path,
        &submissions_dir,
        &agents_dir,
        &scratch_dir,
        &io_rx,
    )?;

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
        &submissions_dir,
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
    submissions_dir: &Path,
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
        agents_dir,
        submissions_dir,
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

    // Block on recv until channel is closed
    while let Ok(event) = events_rx.recv() {
        info!(?event, "received event");
        let (new_state, effects) = step(state, event);
        state = new_state;

        for effect in effects {
            info!(?effect, "emitting effect");
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
    agents_dir: &Path,
    submissions_dir: &Path,
    io_config: &IoConfig,
) -> io::Result<()> {
    debug!(
        "io_loop starting, agents_dir={:?}, submissions_dir={:?}",
        agents_dir, submissions_dir
    );

    // Block on recv until channel is closed (process termination)
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
                    agents_dir,
                    submissions_dir,
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
                let _ = events_tx.send(Event::TaskSubmitted {
                    task_id: TaskId::External(external_id),
                });
            }
            IoEvent::Effect(effect) => {
                debug!(?effect, "executing effect");
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

    // Channel closed - all senders dropped (shouldn't happen in normal operation)
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
    agents_dir: &Path,
    submissions_dir: &Path,
    io_config: &IoConfig,
) {
    for path in &event.paths {
        let Some(category) =
            path_category::categorize(path, event.kind, agents_dir, submissions_dir)
        else {
            continue;
        };

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
                    submissions_dir,
                    events_tx,
                    external_task_map,
                    task_id_allocator,
                    io_config,
                );
            }
            // Anonymous workers protocol - not yet implemented
            PathCategory::WorkerReady { id } => {
                trace!(id = %id, "WorkerReady: anonymous workers not yet implemented");
            }
            PathCategory::WorkerResponse { id } => {
                trace!(id = %id, "WorkerResponse: anonymous workers not yet implemented");
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
    submissions_dir: &Path,
    events_tx: &mpsc::Sender<Event>,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    io_config: &IoConfig,
) {
    let request_path = submissions_dir.join(format!("{id}{REQUEST_SUFFIX}"));

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
            let raw_oneline = raw.replace('\n', "\\n").replace('\r', "");
            warn!(path = %request_path.display(), error = %e, raw = %raw_oneline, "failed to resolve payload");
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
    let Ok(content) = fs::read_to_string(&task_path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    json.get("kind").is_some_and(|k| k == "Kicked")
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

/// Set up the daemon and verify the watcher sees events.
///
/// This function:
/// 1. Creates the lock file and acquires the lock
/// 2. Creates the socket and starts listening
/// 3. Creates both directories (submissions, agents)
/// 4. Writes a canary file to verify watcher sees events
/// 5. Waits until we see ANY filesystem event (proves watcher is working)
/// 6. Cleans up canary file
///
/// # Key Assumption
///
/// Once we observe any filesystem event, the watcher is fully operational.
/// Filesystem watchers (`FSEvents` on macOS, inotify on Linux) don't "partially work".
/// The only failure mode is during initial setup - there's a brief window after
/// `watch()` returns where events might not be delivered yet. Once we receive ANY
/// event, we can trust the watcher is working for the entire directory tree.
///
/// Returns the lock guard and socket listener on success.
///
/// # Panics
///
/// Panics if an unexpected non-FS event is received.
#[allow(clippy::panic)]
fn sync_and_setup(
    root: &Path,
    lock_path: &Path,
    socket_path: &Path,
    submissions_dir: &Path,
    agents_dir: &Path,
    scratch_dir: &Path,
    io_rx: &mpsc::Receiver<IoEvent>,
) -> io::Result<(LockGuard, Listener)> {
    const POLL_TIMEOUT: Duration = Duration::from_millis(100);
    const MAX_DURATION: Duration = Duration::from_secs(5);

    let canary_path = root.join("daemon.canary");

    debug!("waiting for watcher sync via canary file");

    // Create lock and socket first (before watcher matters)
    let lock = acquire_lock(lock_path)?;
    let listener = create_socket_listener(socket_path)?;

    // Create directories
    fs::create_dir_all(submissions_dir)?;
    fs::create_dir_all(agents_dir)?;
    fs::create_dir_all(scratch_dir)?;

    // Write canary file to trigger an event
    fs::write(&canary_path, "0")?;

    let start = std::time::Instant::now();
    let mut retry_count = 0u32;
    while start.elapsed() < MAX_DURATION {
        match io_rx.recv_timeout(POLL_TIMEOUT) {
            Ok(IoEvent::Fs(event)) => {
                // Any filesystem event proves the watcher is working.
                // We don't care which path triggered it - the key insight is that
                // filesystem watchers don't "partially work".
                debug!(
                    "watcher sync complete - received event {:?} for {:?}",
                    event.kind, event.paths
                );
                let _ = fs::remove_file(&canary_path);
                return Ok((lock, listener));
            }
            Ok(_) => panic!("unexpected non-FS event during startup sync"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // On timeout, rewrite canary with incrementing value to trigger new event
                retry_count += 1;
                fs::write(&canary_path, retry_count.to_string())?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = fs::remove_file(&canary_path);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher channel disconnected",
                ));
            }
        }
    }

    warn!("watcher sync timed out - no events received");
    let _ = fs::remove_file(&canary_path);
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "watcher sync timed out - no events received",
    ))
}

struct SocketCleanup(PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Guard that cleans up pool state when dropped.
struct PoolStateCleanup(PathBuf);

impl Drop for PoolStateCleanup {
    fn drop(&mut self) {
        cleanup_pool_state(&self.0);
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
        let submissions_dir = tmp.path();
        let id = "task-1";
        fs::write(
            submissions_dir.join(format!("{id}.request.json")),
            r#"{"kind": "Inline", "content": "test task"}"#,
        )
        .unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut external_task_map = ExternalTaskMap::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        register_submission(
            id,
            submissions_dir,
            &events_tx,
            &mut external_task_map,
            &mut task_id_allocator,
            &io_config,
        );

        let event = events_rx.try_recv().unwrap();
        assert!(matches!(event, Event::TaskSubmitted { task_id } if task_id == ext(0)));
        let request_path = submissions_dir.join(format!("{id}.request.json"));
        assert!(external_task_map.get_id_by_path(&request_path).is_some());
    }

    // =========================================================================
    // Event loop tests (events → step → effects)
    // =========================================================================

    use super::super::core::{AgentId, Effect, PoolState, step};
    use std::thread;
    use tracing::{debug, info};

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
            info!(?event, "received event");

            let (new_state, effects) = step(state, event);
            state = new_state;

            for effect in effects {
                info!(?effect, "emitting effect");
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
