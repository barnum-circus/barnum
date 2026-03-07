//! Daemon - Wires together the core state machine and I/O.
//!
//! This module creates the event and effect channels, spawns the event loop,
//! and runs the I/O layer that handles filesystem events, socket connections,
//! and effect execution.

use std::convert::Infallible;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{self as channel, Receiver, Sender, select};
use interprocess::local_socket::{
    GenericFilePath, Listener, ListenerNonblockingMode, ListenerOptions, Stream, prelude::*,
};
use tracing::{debug, info, trace, warn};

use std::collections::HashSet;

use crate::constants::{
    AGENTS_DIR, LOCK_FILE, REQUEST_SUFFIX, SCRATCH_DIR, SOCKET_NAME, STATUS_FILE, STATUS_READY,
    SUBMISSIONS_DIR,
};
use crate::lock::{LockGuard, acquire_lock};
use crate::submit::Payload;
use crate::verified_watcher::VerifiedWatcher;

use super::core::{Effect, Event, WorkerId};
use super::io::{
    IdAllocator, IoConfig, StopNotifier, SubmissionData, SubmissionMap, WorkerMap, execute_effect,
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

    // Clean agents directory (subdirectories for legacy, flat files for anonymous workers)
    if agents_dir.exists()
        && let Ok(entries) = fs::read_dir(&agents_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            } else if path.is_file() {
                let _ = fs::remove_file(&path);
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
    /// How long an idle worker can wait before receiving a heartbeat.
    pub idle_timeout: Duration,
    /// Default timeout for tasks.
    pub default_task_timeout: Duration,
    /// Whether to send periodic heartbeats to idle workers.
    pub heartbeat_enabled: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(180),
            default_task_timeout: Duration::from_secs(300),
            heartbeat_enabled: true,
        }
    }
}

impl From<DaemonConfig> for IoConfig {
    fn from(config: DaemonConfig) -> Self {
        IoConfig {
            idle_timeout: config.idle_timeout,
            default_task_timeout: config.default_task_timeout,
            heartbeat_enabled: config.heartbeat_enabled,
        }
    }
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
    let root_ref = root.as_ref();
    fs::create_dir_all(root_ref).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "[E050] failed to create pool directory {}: {e}",
                root_ref.display()
            ),
        )
    })?;
    // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
    // so FSEvent paths match our stored paths.
    let root = fs::canonicalize(root_ref).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "[E051] failed to canonicalize pool root {}: {e}",
                root_ref.display()
            ),
        )
    })?;

    // Clean up stale state from previous runs (crashed daemon, etc.)
    cleanup_pool_state(&root);

    let lock_path = root.join(LOCK_FILE);
    let socket_path = root.join(SOCKET_NAME);
    let agents_dir = root.join(AGENTS_DIR);
    let submissions_dir = root.join(SUBMISSIONS_DIR);
    let scratch_dir = root.join(SCRATCH_DIR);

    // Clean up stale socket if it exists
    if socket_path.exists() {
        fs::remove_file(&socket_path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "[E052] failed to remove stale socket {}: {e}",
                    socket_path.display()
                ),
            )
        })?;
    }

    // Create directories first (needed for watcher canary)
    fs::create_dir_all(&submissions_dir).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "[E053] failed to create submissions dir {}: {e}",
                submissions_dir.display()
            ),
        )
    })?;
    fs::create_dir_all(&agents_dir).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "[E054] failed to create agents dir {}: {e}",
                agents_dir.display()
            ),
        )
    })?;
    fs::create_dir_all(&scratch_dir).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "[E055] failed to create scratch dir {}: {e}",
                scratch_dir.display()
            ),
        )
    })?;

    // Clean up pool state on exit (SIGTERM or panic)
    let _pool_cleanup = PoolStateCleanup(root.clone());

    // Use VerifiedWatcher with canary verification for agents/ and submissions/
    let verified_watcher =
        VerifiedWatcher::new(&root, &[agents_dir.clone(), submissions_dir.clone()])?;
    let (_fs_watcher, fs_rx) = verified_watcher.into_receiver(Duration::from_secs(5))?;

    // Acquire lock and create socket listener
    let (_lock, listener) = acquire_lock_and_socket(&lock_path, &socket_path)?;

    let _cleanup = SocketCleanup(socket_path.clone());

    // Write status file to signal daemon is ready
    let status_path = root.join(STATUS_FILE);
    fs::write(&status_path, STATUS_READY).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "[E056] failed to write status file {}: {e}",
                status_path.display()
            ),
        )
    })?;

    info!(socket = %socket_path.display(), "daemon listening");

    let io_config = config.into();
    match run_daemon(
        listener,
        fs_rx,
        &root,
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
fn run_daemon(
    listener: Listener,
    fs_rx: Receiver<notify::Event>,
    root: &Path,
    agents_dir: &Path,
    submissions_dir: &Path,
    io_config: &IoConfig,
) -> io::Result<()> {
    // Create channel for core events (from I/O to event loop)
    let (events_tx, events_rx) = channel::unbounded::<Event>();

    // Create channel for effects (from event loop to I/O)
    let (effect_tx, effect_rx) = channel::unbounded::<Effect>();

    // Create channel for socket connections
    let (socket_tx, socket_rx) = channel::unbounded::<(String, Stream)>();

    // Stop notifier - signals timer threads to exit immediately
    let stop_notifier = Arc::new(StopNotifier::new());

    // Spawn socket accept thread
    let _socket_thread = spawn_socket_accept_thread(listener, socket_tx);

    // I/O state
    let mut worker_map = WorkerMap::new();
    let mut submission_map = SubmissionMap::new();
    let mut id_allocator = IdAllocator::new();
    // Track workers with pending responses to deduplicate FSWatcher events
    let mut pending_responses: HashSet<WorkerId> = HashSet::new();

    // Track kicked worker paths to reject re-registration attempts
    let mut kicked_paths: HashSet<PathBuf> = HashSet::new();

    // Spawn event loop in a separate thread
    let event_loop_handle = thread::spawn(move || run_event_loop(events_rx, effect_tx));

    // Run the I/O loop with select! on all channels
    let result = io_loop(
        fs_rx,
        socket_rx,
        effect_rx,
        &events_tx,
        &mut worker_map,
        &mut submission_map,
        &mut id_allocator,
        &mut pending_responses,
        &mut kicked_paths,
        root,
        agents_dir,
        submissions_dir,
        io_config,
        &stop_notifier,
    );

    // Signal stop to wake up all timer threads immediately
    stop_notifier.stop();

    // Wait for event loop to finish (it exits when channel closes)
    let final_state = event_loop_handle
        .join()
        .map_err(|_| io::Error::other("[E030] event loop thread panicked"))?;

    debug!(
        workers = final_state.worker_count(),
        pending = final_state.pending_count(),
        "daemon stop complete"
    );

    result
}

/// Run the core event loop.
///
/// Receives core events, runs the state machine, and sends effects
/// to the effect channel. Exits when the events channel closes.
#[expect(clippy::needless_pass_by_value)] // Runs in spawned thread
fn run_event_loop(events_rx: Receiver<Event>, effect_tx: Sender<Effect>) -> super::core::PoolState {
    use super::core::{PoolState, step};

    let mut state = PoolState::new();

    // Block on recv until channel is closed
    while let Ok(event) = events_rx.recv() {
        info!(?event, "received event");
        let (new_state, effects) = step(state, event);
        state = new_state;

        for effect in effects {
            info!(?effect, "emitting effect");
            if effect_tx.send(effect).is_err() {
                // I/O loop is gone, exit
                debug!("event loop: effect channel closed");
                break;
            }
        }
    }

    debug!(
        pending = state.pending_count(),
        workers = state.worker_count(),
        "event loop stopped"
    );

    state
}

/// The I/O loop that handles events from multiple channels using select!.
///
/// Uses `crossbeam_channel::select!` to wait on filesystem, socket, and effect
/// channels simultaneously. Exits on stop signal or when all channels close.
#[expect(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn io_loop(
    fs_rx: Receiver<notify::Event>,
    socket_rx: Receiver<(String, Stream)>,
    effect_rx: Receiver<Effect>,
    events_tx: &Sender<Event>,
    worker_map: &mut WorkerMap,
    submission_map: &mut SubmissionMap,
    id_allocator: &mut IdAllocator,
    pending_responses: &mut HashSet<WorkerId>,
    kicked_paths: &mut HashSet<PathBuf>,
    root: &Path,
    agents_dir: &Path,
    submissions_dir: &Path,
    io_config: &IoConfig,
    stop_notifier: &Arc<StopNotifier>,
) -> io::Result<()> {
    debug!(
        "io_loop starting, agents_dir={:?}, submissions_dir={:?}",
        agents_dir, submissions_dir
    );

    loop {
        select! {
            recv(fs_rx) -> event => {
                let Ok(event) = event else {
                    debug!("fs channel closed");
                    break;
                };
                debug!(kind = ?event.kind, paths = ?event.paths, "io_loop: fs event");
                let should_continue = handle_fs_event(
                    &event,
                    events_tx,
                    worker_map,
                    submission_map,
                    id_allocator,
                    pending_responses,
                    kicked_paths,
                    root,
                    agents_dir,
                    submissions_dir,
                    io_config,
                );
                if !should_continue {
                    info!("stop signal received, shutting down");
                    break;
                }
            }
            recv(socket_rx) -> conn => {
                let Ok((raw, stream)) = conn else {
                    debug!("socket channel closed");
                    break;
                };
                let content = match resolve_payload(&raw) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(error = %e, "failed to resolve socket payload");
                        continue;
                    }
                };

                let submission_id = id_allocator.allocate_submission();
                submission_map.register_socket(
                    stream,
                    SubmissionData {
                        content,
                        timeout: io_config.default_task_timeout,
                    },
                );
                let _ = events_tx.send(Event::TaskSubmitted { submission_id });
            }
            recv(effect_rx) -> effect => {
                let Ok(effect) = effect else {
                    debug!("effect channel closed");
                    break;
                };
                debug!(?effect, "executing effect");
                // Clear pending response tracking when TaskCompleted cleans up the response file
                if let Effect::TaskCompleted { worker_id, .. } = &effect {
                    pending_responses.remove(worker_id);
                }
                execute_effect(
                    effect,
                    worker_map,
                    submission_map,
                    kicked_paths,
                    events_tx,
                    io_config,
                    stop_notifier,
                )?;
            }
        }
    }

    Ok(())
}

// =============================================================================
// Filesystem Event Handling
// =============================================================================

/// Handle a filesystem event, converting it to core events.
///
/// Returns `true` to continue processing, `false` to signal shutdown.
#[expect(clippy::too_many_arguments)]
fn handle_fs_event(
    event: &notify::Event,
    events_tx: &Sender<Event>,
    worker_map: &mut WorkerMap,
    submission_map: &mut SubmissionMap,
    id_allocator: &mut IdAllocator,
    pending_responses: &mut HashSet<WorkerId>,
    kicked_paths: &HashSet<PathBuf>,
    root: &Path,
    agents_dir: &Path,
    submissions_dir: &Path,
    io_config: &IoConfig,
) -> bool {
    for path in &event.paths {
        let category =
            path_category::categorize(path, event.kind, root, agents_dir, submissions_dir);
        trace!(?path, ?category, kind = ?event.kind, "categorize result");
        let Some(category) = category else {
            continue;
        };

        match category {
            PathCategory::WorkerReady { id } => {
                handle_worker_ready_file(
                    &id,
                    path,
                    agents_dir,
                    events_tx,
                    worker_map,
                    kicked_paths,
                );
            }
            PathCategory::WorkerResponse { id } => {
                handle_worker_response_file(
                    &id,
                    path,
                    events_tx,
                    worker_map,
                    pending_responses,
                    kicked_paths,
                );
            }
            PathCategory::SubmissionRequest { id } => {
                // Skip if file doesn't exist (may have been cleaned up already)
                if !path.exists() {
                    trace!(id = %id, "SubmissionRequest: file doesn't exist, skipping");
                    continue;
                }
                register_submission(
                    &id,
                    submissions_dir,
                    events_tx,
                    submission_map,
                    id_allocator,
                    io_config,
                );
            }
            PathCategory::Stop => {
                // Stop signal received
                warn!(
                    path = %path.display(),
                    event_kind = ?event.kind,
                    "DAEMON: received Stop signal via status file"
                );
                return false;
            }
        }
    }
    true
}

/// Handle a worker ready file.
///
/// Worker writes `<uuid>.ready.json` to signal availability.
/// We register the worker and send `WorkerReady` event.
fn handle_worker_ready_file(
    uuid: &str,
    ready_path: &Path,
    agents_dir: &Path,
    events_tx: &Sender<Event>,
    worker_map: &mut WorkerMap,
    kicked_paths: &HashSet<PathBuf>,
) {
    // Skip if file doesn't exist (may have been cleaned up)
    if !ready_path.exists() {
        return;
    }

    // Skip if this UUID was kicked
    if kicked_paths.contains(ready_path) {
        trace!(uuid = %uuid, "WorkerReady: skipping kicked worker");
        return;
    }

    // Skip if already registered (duplicate event)
    if worker_map.get_id_by_path(ready_path).is_some() {
        trace!(uuid = %uuid, "WorkerReady: already registered");
        return;
    }

    // Register the worker with flat file transport
    if let Some(worker_id) = worker_map.register_flat_file(
        ready_path.to_path_buf(),
        agents_dir.to_path_buf(),
        uuid.to_string(),
        (),
    ) {
        debug!(uuid = %uuid, worker_id = worker_id.0, "WorkerReady: registered");
        let _ = events_tx.send(Event::WorkerReady { worker_id });
    }
}

/// Handle a worker response file (anonymous workers protocol).
///
/// Worker writes `<uuid>.response.json` to signal task completion.
/// We send `WorkerResponded` event.
fn handle_worker_response_file(
    uuid: &str,
    response_path: &Path,
    events_tx: &Sender<Event>,
    worker_map: &WorkerMap,
    pending_responses: &mut HashSet<WorkerId>,
    kicked_paths: &HashSet<PathBuf>,
) {
    // Skip if file doesn't exist
    if !response_path.exists() {
        return;
    }

    // For anonymous workers, the ready file path is the key in worker_map.
    // Derive ready path from response path: <uuid>.response.json -> <uuid>.ready.json
    let Some(parent) = response_path.parent() else {
        return;
    };
    let ready_path = parent.join(format!("{uuid}.ready.json"));

    // Skip if kicked
    if kicked_paths.contains(&ready_path) {
        trace!(uuid = %uuid, "WorkerResponse: skipping kicked worker");
        return;
    }

    // Find the worker
    let Some(worker_id) = worker_map.get_id_by_path(&ready_path) else {
        // Worker not found - might have registered before we started watching
        // or response arrived before ready file was processed
        trace!(uuid = %uuid, "WorkerResponse: worker not found");
        return;
    };

    // Send response event (deduplicated)
    if pending_responses.insert(worker_id) {
        debug!(uuid = %uuid, worker_id = worker_id.0, "WorkerResponse: sending event");
        let _ = events_tx.send(Event::WorkerResponded { worker_id });
    } else {
        trace!(uuid = %uuid, "WorkerResponse: duplicate event");
    }
}

/// Register a submission from a request file.
fn register_submission(
    id: &str,
    submissions_dir: &Path,
    events_tx: &Sender<Event>,
    submission_map: &mut SubmissionMap,
    id_allocator: &mut IdAllocator,
    io_config: &IoConfig,
) {
    debug!(id = %id, "register_submission: called");
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
    let submission_id = id_allocator.allocate_submission();
    if submission_map.register(
        submission_id,
        request_path,
        SubmissionData {
            content,
            timeout: io_config.default_task_timeout,
        },
    ) {
        let _ = events_tx.send(Event::TaskSubmitted { submission_id });
    }
}

// =============================================================================
// Socket Handling
// =============================================================================

/// Spawn a thread that accepts socket connections and sends them to the socket channel.
fn spawn_socket_accept_thread(
    listener: Listener,
    socket_tx: Sender<(String, Stream)>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            match accept_socket_task(&listener) {
                Ok(Some((raw, stream))) => {
                    if socket_tx.send((raw, stream)).is_err() {
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
            format!("[E031] socket accept failed: {e}"),
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
    let payload: Payload = serde_json::from_str(raw).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("[E032] failed to parse payload JSON: {e}"),
        )
    })?;

    match payload {
        Payload::Inline { content } => Ok(content),
        Payload::FileReference { path } => fs::read_to_string(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("[E033] failed to read payload file {}: {e}", path.display()),
            )
        }),
    }
}

// =============================================================================
// Setup Helpers
// =============================================================================

fn create_socket_listener(socket_path: &Path) -> io::Result<Listener> {
    let name = socket_path.to_fs_name::<GenericFilePath>().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("[E034] invalid socket path {}: {e}", socket_path.display()),
        )
    })?;

    let listener = ListenerOptions::new()
        .name(name)
        .create_sync()
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::AddrInUse,
                format!(
                    "[E035] failed to create socket {}: {e}",
                    socket_path.display()
                ),
            )
        })?;

    listener
        .set_nonblocking(ListenerNonblockingMode::Accept)
        .map_err(|e| {
            io::Error::other(format!(
                "[E036] failed to set socket non-blocking {}: {e}",
                socket_path.display()
            ))
        })?;

    Ok(listener)
}

/// Acquire the daemon lock and create the socket listener.
fn acquire_lock_and_socket(
    lock_path: &Path,
    socket_path: &Path,
) -> io::Result<(LockGuard, Listener)> {
    let lock = acquire_lock(lock_path)?;
    let listener = create_socket_listener(socket_path)?;
    Ok((lock, listener))
}

struct SocketCleanup(PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Guard that deletes the entire pool folder when dropped.
struct PoolStateCleanup(PathBuf);

impl Drop for PoolStateCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sub(id: u32) -> SubmissionId {
        SubmissionId(id)
    }

    fn worker(id: u32) -> WorkerId {
        WorkerId(id)
    }

    #[test]
    fn daemon_config_converts_to_io_config() {
        let daemon_config = DaemonConfig {
            idle_timeout: Duration::from_secs(120),
            default_task_timeout: Duration::from_secs(600),
            heartbeat_enabled: false,
        };

        let io_config: IoConfig = daemon_config.into();

        assert_eq!(io_config.idle_timeout, Duration::from_secs(120));
        assert_eq!(io_config.default_task_timeout, Duration::from_secs(600));
        assert!(!io_config.heartbeat_enabled);
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

        let (events_tx, events_rx) = channel::unbounded();
        let mut submission_map = SubmissionMap::new();
        let mut id_allocator = IdAllocator::new();
        let io_config = IoConfig::default();

        register_submission(
            id,
            submissions_dir,
            &events_tx,
            &mut submission_map,
            &mut id_allocator,
            &io_config,
        );

        let event = events_rx.try_recv().unwrap();
        assert!(matches!(event, Event::TaskSubmitted { submission_id } if submission_id == sub(0)));
        let request_path = submissions_dir.join(format!("{id}.request.json"));
        assert!(submission_map.get_id_by_path(&request_path).is_some());
    }

    // =========================================================================
    // Event loop tests (events → step → effects)
    // =========================================================================

    use super::super::core::{Effect, PoolState, SubmissionId, TaskId, step};
    use std::thread;
    use tracing::{debug, info};

    /// Run the event loop until the events channel closes.
    fn run_test_event_loop(events_rx: Receiver<Event>, effects_tx: Sender<Effect>) -> PoolState {
        run_test_event_loop_with_state(PoolState::new(), events_rx, effects_tx)
    }

    /// Run the event loop with an initial state.
    #[expect(clippy::needless_pass_by_value)]
    fn run_test_event_loop_with_state(
        mut state: PoolState,
        events_rx: Receiver<Event>,
        effects_tx: Sender<Effect>,
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
            workers = state.worker_count(),
            "event loop stopped"
        );

        state
    }

    #[test]
    fn event_loop_processes_events_and_emits_effects() {
        let (events_tx, events_rx) = channel::unbounded();
        let (effects_tx, effects_rx) = channel::unbounded();

        let handle = thread::spawn(move || run_test_event_loop(events_rx, effects_tx));

        events_tx
            .send(Event::WorkerReady {
                worker_id: worker(1),
            })
            .unwrap();

        let effect = effects_rx.recv().unwrap();
        assert!(matches!(effect, Effect::WorkerWaiting { .. }));

        events_tx
            .send(Event::TaskSubmitted {
                submission_id: sub(42),
            })
            .unwrap();

        let effect = effects_rx.recv().unwrap();
        assert!(matches!(
            effect,
            Effect::TaskAssigned { task_id: TaskId::External(sub_id), .. } if sub_id == sub(42)
        ));

        drop(events_tx);

        let final_state = handle.join().unwrap();
        assert_eq!(final_state.worker_count(), 1);
        assert_eq!(final_state.pending_count(), 0);
    }

    #[test]
    fn event_loop_handles_channel_close_gracefully() {
        let (events_tx, events_rx) = channel::unbounded();
        let (effects_tx, effects_rx) = channel::unbounded();

        let handle = thread::spawn(move || run_test_event_loop(events_rx, effects_tx));

        drop(effects_rx);

        events_tx
            .send(Event::WorkerReady {
                worker_id: worker(1),
            })
            .unwrap();

        drop(events_tx);

        let final_state = handle.join().unwrap();
        assert_eq!(final_state.worker_count(), 1);
    }

    #[test]
    fn event_loop_uses_initial_state() {
        let (events_tx, events_rx) = channel::unbounded();
        let (effects_tx, effects_rx) = channel::unbounded();

        let initial_state = PoolState::new();

        let handle = thread::spawn(move || {
            run_test_event_loop_with_state(initial_state, events_rx, effects_tx)
        });

        events_tx
            .send(Event::WorkerReady {
                worker_id: worker(1),
            })
            .unwrap();

        let _ = effects_rx.recv().unwrap();

        drop(events_tx);
        let final_state = handle.join().unwrap();
        assert_eq!(final_state.worker_count(), 1);
    }
}
