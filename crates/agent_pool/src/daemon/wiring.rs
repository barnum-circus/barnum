//! Daemon - Wires together the core state machine and I/O.
//!
//! This module creates the event and effect channels, spawns the event loop,
//! and runs the I/O layer that handles filesystem events, socket connections,
//! and effect execution.

use std::convert::Infallible;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use interprocess::local_socket::{
    GenericFilePath, Listener, ListenerNonblockingMode, ListenerOptions, prelude::*,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, info, trace, warn};

use std::collections::HashSet;

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
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            idle_agent_timeout: Duration::from_secs(60),
            default_task_timeout: Duration::from_secs(300),
        }
    }
}

impl From<DaemonConfig> for IoConfig {
    fn from(config: DaemonConfig) -> Self {
        IoConfig {
            idle_agent_timeout: config.idle_agent_timeout,
            default_task_timeout: config.default_task_timeout,
        }
    }
}

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
    let root = root.as_ref().to_path_buf();

    fs::create_dir_all(&root)?;

    let lock_path = root.join(LOCK_FILE);
    let lock = acquire_lock(&lock_path)?;

    let agents_dir = root.join(AGENTS_DIR);
    fs::create_dir_all(&agents_dir)?;

    let pending_dir = root.join(PENDING_DIR);
    fs::create_dir_all(&pending_dir)?;

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

        info!(socket = %socket_path.display(), "daemon listening");

        run_daemon(
            &listener,
            &fs_events,
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
    let root = root.as_ref().to_path_buf();

    fs::create_dir_all(&root)?;

    let lock_path = root.join(LOCK_FILE);
    let _lock = acquire_lock(&lock_path)?;

    let agents_dir = root.join(AGENTS_DIR);
    fs::create_dir_all(&agents_dir)?;

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

    info!(socket = %socket_path.display(), "daemon listening");

    let signals = DaemonSignals::new();
    let io_config = config.into();
    match run_daemon(
        &listener,
        &fs_events,
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
    listener: &Listener,
    fs_events: &mpsc::Receiver<notify::Event>,
    agents_dir: &Path,
    pending_dir: &Path,
    io_config: &IoConfig,
    signals: &DaemonSignals,
) -> io::Result<()> {
    // Create channels between event loop and I/O
    let (events_tx, events_rx) = mpsc::channel::<Event>();
    let (effects_tx, effects_rx) = mpsc::channel::<Effect>();

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
        run_event_loop_with_shutdown(events_rx, effects_tx, event_loop_signals)
    });

    // Do initial scan of existing agents (kicked_paths is empty at startup)
    scan_agents(agents_dir, &events_tx, &mut agent_map, &kicked_paths)?;

    // Run the I/O loop
    let result = io_loop(
        listener,
        fs_events,
        &events_tx,
        &effects_rx,
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
#[allow(clippy::too_many_arguments)]
fn io_loop(
    listener: &Listener,
    fs_events: &mpsc::Receiver<notify::Event>,
    events_tx: &mpsc::Sender<Event>,
    effects_rx: &mpsc::Receiver<Effect>,
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
    let poll_timeout = Duration::from_millis(100);
    let scan_interval = Duration::from_millis(500);
    let mut last_scan = Instant::now();

    debug!("io_loop starting, agents_dir={:?}, pending_dir={:?}", agents_dir, pending_dir);

    loop {
        if signals.is_shutdown_triggered() {
            info!("shutdown requested");
            return Ok(());
        }

        // Check for socket-based task submissions (non-blocking)
        if !signals.is_paused()
            && let Some((content, respond_to)) = accept_task(listener)?
        {
            let external_id = task_id_allocator.allocate_external();
            if external_task_map
                .register(external_id, respond_to, ExternalTaskData {
                    content,
                    timeout: io_config.default_task_timeout,
                })
            {
                debug!("socket task submitted: {:?}", external_id);
                let _ = events_tx.send(Event::TaskSubmitted {
                    task_id: TaskId::External(external_id),
                });
            }
        }

        // Process filesystem events (non-blocking drain)
        match fs_events.recv_timeout(poll_timeout) {
            Ok(event) => {
                trace!(kind = ?event.kind, "fs event received");
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
                // Drain any additional queued events
                while let Ok(event) = fs_events.try_recv() {
                    trace!(kind = ?event.kind, "fs event (drained)");
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
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("fs event channel disconnected"));
            }
        }

        // Process effects from event loop (non-blocking drain)
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

        // Periodic scans for reliability
        if last_scan.elapsed() >= scan_interval {
            scan_agents(agents_dir, events_tx, agent_map, kicked_paths)?;
            scan_pending(pending_dir, events_tx, external_task_map, task_id_allocator, io_config)?;
            last_scan = Instant::now();
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
                if !agent_path.exists() {
                    // Directory deleted - clean up tracking
                    kicked_paths.remove(&agent_path);
                    if let Some(agent_id) = agent_map.get_id_by_path(&agent_path) {
                        let _ = events_tx.send(Event::AgentDeregistered { agent_id });
                    }
                } else if agent_path.is_dir() && !kicked_paths.contains(&agent_path) {
                    // Only register if not kicked
                    if let Some(agent_id) = agent_map.register_directory(agent_path, ()) {
                        // No heartbeat - let core try pending queue first
                        let _ = events_tx.send(Event::AgentRegistered {
                            agent_id,
                            heartbeat_task_id: None,
                        });
                    }
                }
            }
            PathCategory::AgentResponse { name } => {
                let agent_path = agents_dir.join(&name);
                if path.exists() && !kicked_paths.contains(&agent_path) {
                    // Only process responses from non-kicked agents
                    if agent_map.get_id_by_path(&agent_path).is_none() {
                        if let Some(agent_id) = agent_map.register_directory(agent_path.clone(), ()) {
                            // No heartbeat - let core try pending queue first
                            let _ = events_tx.send(Event::AgentRegistered {
                                agent_id,
                                heartbeat_task_id: None,
                            });
                        }
                    }
                    if let Some(agent_id) = agent_map.get_id_by_path(&agent_path) {
                        // Deduplicate: FSWatcher often sends multiple events for the same file
                        if pending_responses.insert(agent_id) {
                            let _ = events_tx.send(Event::AgentResponded { agent_id });
                        } else {
                            trace!(agent_id = agent_id.0, "skipping duplicate AgentResponded");
                        }
                    }
                }
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

    // Read task content
    let content = match fs::read_to_string(&task_path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!(path = %task_path.display(), error = %e, "failed to read pending task");
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
        // Skip kicked agents
        if kicked_paths.contains(&agent_path) {
            continue;
        }
        if let Some(agent_id) = agent_map.register_directory(agent_path, ()) {
            debug!(agent_id = agent_id.0, "agent registered via scan");
            // No heartbeat - let core try pending queue first
            let _ = events_tx.send(Event::AgentRegistered {
                agent_id,
                heartbeat_task_id: None,
            });
        }
    }

    Ok(())
}

/// Scan the pending directory for new tasks.
fn scan_pending(
    pending_dir: &Path,
    events_tx: &mpsc::Sender<Event>,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    io_config: &IoConfig,
) -> io::Result<()> {
    if !pending_dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(pending_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let submission_dir = entry.path();
        let task_path = submission_dir.join(TASK_FILE);

        // Only process if task.json exists
        if task_path.exists() {
            register_pending_task(&submission_dir, events_tx, external_task_map, task_id_allocator, io_config);
        }
    }

    Ok(())
}

// =============================================================================
// Socket Handling
// =============================================================================

/// Accept a task from the socket listener (non-blocking).
///
/// Returns the task content and the directory path for the response.
fn accept_task(listener: &Listener) -> io::Result<Option<(String, PathBuf)>> {
    use std::io::{BufRead, BufReader, Read};

    match listener.accept() {
        Ok(stream) => {
            let mut reader = BufReader::new(&stream);

            let mut len_line = String::new();
            reader.read_line(&mut len_line)?;

            let len: usize = match len_line.trim().parse() {
                Ok(n) => n,
                Err(_) => return Ok(None),
            };

            let mut content = vec![0u8; len];
            reader.read_exact(&mut content)?;

            let Ok(_content) = String::from_utf8(content) else {
                return Ok(None);
            };

            // Create a temporary directory for this socket-based submission
            // We'll write the response there when complete
            let _submission_id = uuid::Uuid::new_v4().to_string();
            // Store the stream handle somehow... this is tricky.
            // For now, we'll need to refactor to handle socket responses differently.
            // TODO: Handle socket-based responses properly

            // TODO: Handle socket-based submissions properly
            warn!("socket-based submissions not yet supported, task ignored");
            drop(stream);
            Ok(None)
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(io::Error::new(
            e.kind(),
            format!("socket accept failed: {e}"),
        )),
    }
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

fn create_watcher(
    agents_dir: &Path,
) -> io::Result<(RecommendedWatcher, mpsc::Receiver<notify::Event>)> {
    let (tx, rx) = mpsc::channel();

    let config = notify::Config::default().with_poll_interval(Duration::from_millis(100));

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        config,
    )
    .map_err(io::Error::other)?;

    // Only watch agents_dir - pending tasks are handled via periodic scan_pending
    // (This matches v1 behavior and is more reliable on macOS)
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
        };

        let io_config: IoConfig = daemon_config.into();

        assert_eq!(io_config.idle_agent_timeout, Duration::from_secs(120));
        assert_eq!(io_config.default_task_timeout, Duration::from_secs(600));
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

        scan_agents(&agents_dir, &events_tx, &mut agent_map, &kicked_paths).unwrap();

        // Should have received two AgentRegistered events
        let mut events = vec![];
        while let Ok(event) = events_rx.try_recv() {
            events.push(event);
        }

        assert_eq!(events.len(), 2);
        for event in events {
            assert!(matches!(event, Event::AgentRegistered { .. }));
        }
    }

    #[test]
    fn scan_pending_registers_tasks() {
        let tmp = TempDir::new().unwrap();
        let pending_dir = tmp.path().join("pending");
        fs::create_dir_all(&pending_dir).unwrap();

        // Create a pending task
        let task_dir = pending_dir.join("task-1");
        fs::create_dir_all(&task_dir).unwrap();
        fs::write(task_dir.join(TASK_FILE), r#"{"test": true}"#).unwrap();

        let (events_tx, events_rx) = mpsc::channel();
        let mut external_task_map = ExternalTaskMap::new();
        let mut task_id_allocator = TaskIdAllocator::new();
        let io_config = IoConfig::default();

        scan_pending(&pending_dir, &events_tx, &mut external_task_map, &mut task_id_allocator, &io_config).unwrap();

        // Should have received one TaskSubmitted event
        let event = events_rx.try_recv().unwrap();
        assert!(matches!(event, Event::TaskSubmitted { task_id } if task_id == ext(0)));
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
