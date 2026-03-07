//! I/O layer
//!
//! This module handles all I/O operations:
//! - Socket communication (accepting connections, sending responses)
//! - Filesystem operations (reading/writing task and response files)
//! - Timer management (starting timeout timers)
//! - Event parsing (converting FS events to our Event enum)
//!
//! The I/O layer maps abstract IDs from core to concrete transports and content.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::Sender;
use interprocess::local_socket::Stream;
use tracing::{debug, warn};

use crate::Transport;
use crate::constants::{REQUEST_SUFFIX, RESPONSE_FILE, RESPONSE_SUFFIX, TASK_FILE};
use crate::response::{NotProcessedReason, Response};

use super::core::{Effect, Event, SubmissionId, TaskId, WorkerId};

// =============================================================================
// Stop Notifier
// =============================================================================

/// Thread-safe stop notifier with interruptible wait.
///
/// Timer threads use `wait_timeout` to sleep with the ability to wake up
/// immediately when `stop` is called. This prevents timer threads from
/// continuing to run after the pool starts stopping.
#[derive(Debug, Default)]
pub(super) struct StopNotifier {
    /// Flag indicating stop is in progress.
    flag: AtomicBool,
    /// Mutex for the condvar (condvar requires a mutex guard).
    mutex: Mutex<()>,
    /// Condvar to wake up waiting threads.
    condvar: Condvar,
}

impl StopNotifier {
    /// Create a new stop notifier.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wait for the given duration or until stop is signaled.
    ///
    /// Returns `true` if stop was signaled (either before or during the wait).
    /// Returns `false` if the timeout elapsed without stop.
    #[expect(clippy::significant_drop_tightening)] // Guard must be held for condvar wait
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        if self.flag.load(Ordering::Relaxed) {
            return true;
        }

        #[expect(clippy::expect_used)] // Mutex poisoning indicates a bug
        let guard = self.mutex.lock().expect("mutex poisoned");
        #[expect(clippy::expect_used)] // Condvar wait can't fail if mutex isn't poisoned
        let (_guard, timeout_result) = self
            .condvar
            .wait_timeout(guard, timeout)
            .expect("condvar wait failed");

        // Check again after waking (might have been notified)
        if self.flag.load(Ordering::Relaxed) {
            return true;
        }

        // Timeout elapsed without stop
        timeout_result.timed_out()
    }

    /// Signal stop and wake all waiting threads.
    pub fn stop(&self) {
        self.flag.store(true, Ordering::Relaxed);
        self.condvar.notify_all();
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// I/O configuration.
#[derive(Debug, Clone)]
pub(super) struct IoConfig {
    /// How long an idle worker can wait before receiving a heartbeat.
    pub idle_timeout: Duration,
    /// Default timeout for tasks (used when submission doesn't specify one).
    pub default_task_timeout: Duration,
    /// Whether to send periodic heartbeats to idle workers.
    pub heartbeat_enabled: bool,
}

impl Default for IoConfig {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(180),
            default_task_timeout: Duration::from_secs(300),
            heartbeat_enabled: true,
        }
    }
}

// =============================================================================
// Transport ID Trait
// =============================================================================

/// Trait for IDs that can be used with `TransportMap`.
pub(super) trait TransportId:
    Copy + Eq + std::hash::Hash + std::fmt::Debug + From<u32>
{
    /// Data stored alongside the transport for this ID type.
    type Data: std::fmt::Debug;
}

impl TransportId for WorkerId {
    type Data = ();
}

// =============================================================================
// ID Allocators
// =============================================================================

/// Allocates submission IDs.
#[derive(Debug, Default)]
pub(super) struct IdAllocator {
    next_submission_id: u32,
}

impl IdAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a submission ID.
    #[expect(clippy::missing_const_for_fn)] // const fn with &mut self not stable
    pub fn allocate_submission(&mut self) -> SubmissionId {
        let id = SubmissionId(self.next_submission_id);
        self.next_submission_id += 1;
        id
    }
}

/// Data stored per submission (external task).
#[derive(Debug)]
pub(super) struct SubmissionData {
    /// The task content to send to the worker.
    pub content: String,
    /// How long the worker has to complete this task.
    pub timeout: Duration,
}

impl TransportId for SubmissionId {
    type Data = SubmissionData;
}

// =============================================================================
// Transport Map
// =============================================================================

/// Generic map from IDs to transports and associated data.
///
/// **Invariant:** If `entries[id]` exists and is `Transport::Directory(path)`,
/// then `path_to_id[path] == id`. Maintained by `register_directory` and `remove`.
#[derive(Debug)]
pub(super) struct TransportMap<Id: TransportId> {
    entries: HashMap<Id, (Transport, Id::Data)>,
    path_to_id: HashMap<PathBuf, Id>,
    next_id: u32,
}

impl<Id: TransportId> Default for TransportMap<Id> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            path_to_id: HashMap::new(),
            next_id: 0,
        }
    }
}

impl<Id: TransportId> TransportMap<Id> {
    /// Create a new empty transport map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate an ID without registering an entry.
    pub fn allocate_id(&mut self) -> Id {
        let id = Id::from(self.next_id);
        self.next_id += 1;
        id
    }

    /// Register a pre-allocated ID with a directory-based transport.
    ///
    /// Returns `true` if registered, `false` if the path is already registered.
    pub fn register(&mut self, id: Id, path: PathBuf, data: Id::Data) -> bool {
        use std::collections::hash_map::Entry;

        let Entry::Vacant(entry) = self.path_to_id.entry(path.clone()) else {
            return false; // Duplicate FS event
        };
        entry.insert(id);
        self.entries.insert(id, (Transport::Directory(path), data));
        true
    }

    /// Register a socket-based transport with associated data.
    ///
    /// Socket transports don't have paths, so no duplicate checking is done.
    pub fn register_socket(&mut self, stream: Stream, data: Id::Data) -> Id {
        let id = self.allocate_id();
        self.entries.insert(id, (Transport::Socket(stream), data));
        id
    }

    /// Register a flat file transport with associated data.
    ///
    /// The `key_path` is used for deduplication (e.g., the ready file path).
    /// The transport stores `dir` and `uuid` for constructing file paths.
    ///
    /// Returns `None` if the `key_path` is already registered (duplicate FS event).
    pub fn register_flat_file(
        &mut self,
        key_path: PathBuf,
        dir: PathBuf,
        uuid: String,
        data: Id::Data,
    ) -> Option<Id> {
        // Check for duplicate first
        if self.path_to_id.contains_key(&key_path) {
            return None;
        }
        let id = self.allocate_id();
        self.path_to_id.insert(key_path, id);
        self.entries
            .insert(id, (Transport::FlatFile { dir, uuid }, data));
        Some(id)
    }

    /// Get the transport for an ID.
    #[must_use]
    pub fn get_transport(&self, id: Id) -> Option<&Transport> {
        self.entries.get(&id).map(|(ch, _)| ch)
    }

    /// Get the data for an ID.
    #[must_use]
    pub fn get_data(&self, id: Id) -> Option<&Id::Data> {
        self.entries.get(&id).map(|(_, data)| data)
    }

    /// Look up an ID by path.
    #[must_use]
    pub fn get_id_by_path(&self, path: &Path) -> Option<Id> {
        self.path_to_id.get(path).copied()
    }

    /// Remove an entry and return its transport and data.
    pub fn remove(&mut self, id: Id) -> Option<(Transport, Id::Data)> {
        let entry = self.entries.remove(&id)?;
        // Only directory transports have path_to_id entries
        if let Transport::Directory(ref path) = entry.0 {
            self.path_to_id.remove(path);
        }
        Some(entry)
    }

    /// Write content to a file in the transport for the given ID.
    pub fn write_to(&self, id: Id, filename: &str, content: &str) -> io::Result<()> {
        let transport = self.get_transport(id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("[E009] transport not found for id {id:?}"),
            )
        })?;
        transport.write(filename, content)
    }

    /// Read content from a file in the transport for the given ID.
    pub fn read_from(&self, id: Id, filename: &str) -> io::Result<String> {
        let transport = self.get_transport(id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("[E010] transport not found for id {id:?}"),
            )
        })?;
        transport.read(filename)
    }

    /// Get the path for the given ID (for directory-based transports).
    pub fn get_path(&self, id: Id) -> Option<&Path> {
        self.get_transport(id).and_then(Transport::path)
    }
}

// =============================================================================
// Type Aliases
// =============================================================================

/// Map of workers to their transports.
pub(super) type WorkerMap = TransportMap<WorkerId>;

/// Map of submissions to their transports and data.
pub(super) type SubmissionMap = TransportMap<SubmissionId>;

// =============================================================================
// SubmissionMap Extensions
// =============================================================================

impl SubmissionMap {
    /// Finish a submission: write response to transport and remove from map.
    ///
    /// Used for both success and failure - the response content determines the outcome.
    /// For directory transports, writes to response.json.
    /// For socket transports, sends length-prefixed response over the socket.
    pub fn finish(&mut self, id: SubmissionId, response: &str) -> io::Result<SubmissionData> {
        debug!(submission_id = id.0, "finish: completing submission");
        let (mut transport, data) = self.remove(id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("[E011] submission not found for id {}", id.0),
            )
        })?;

        match &mut transport {
            #[expect(clippy::expect_used)]
            // Internal invariant: request path must have REQUEST_SUFFIX
            Transport::Directory(path) => {
                // path is the request file; derive response path
                let response_path = path.with_file_name(
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .and_then(|n| n.strip_suffix(REQUEST_SUFFIX))
                        .map(|id| format!("{id}{RESPONSE_SUFFIX}"))
                        .expect("request path should have REQUEST_SUFFIX"),
                );
                debug!(
                    submission_id = id.0,
                    path = %response_path.display(),
                    "finish: writing response"
                );
                // Write atomically: write to temp file in same dir, then rename.
                // Temp file must be on same filesystem as target for rename to work.
                let temp_path = response_path
                    .parent()
                    .unwrap_or(path)
                    .join(format!(".response-{}.tmp", uuid::Uuid::new_v4()));
                fs::write(&temp_path, response).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!(
                            "[E012] failed to write response temp file {}: {e}",
                            temp_path.display()
                        ),
                    )
                })?;
                fs::rename(&temp_path, &response_path).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!(
                            "[E013] failed to rename response {} to {}: {e}",
                            temp_path.display(),
                            response_path.display()
                        ),
                    )
                })?;
            }
            Transport::Socket(stream) => {
                debug!(submission_id = id.0, "finish: sending socket response");
                writeln!(stream, "{}", response.len())?;
                stream.write_all(response.as_bytes())?;
                stream.flush()?;
            }
            Transport::FlatFile { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "FlatFile transport not supported for submissions",
                ));
            }
        }

        Ok(data)
    }
}

// =============================================================================
// Effect Execution
// =============================================================================

/// Execute an effect, performing the actual I/O.
///
/// # Errors
///
/// Returns an error if I/O operations fail (writing task files, reading responses).
///
/// # Panics
///
/// Panics if the effect references an ID that doesn't exist. This indicates a
/// core bug, since core should only emit effects for IDs it knows about.
#[expect(clippy::expect_used)] // Internal invariants - effects reference valid IDs
pub(super) fn execute_effect(
    effect: Effect,
    worker_map: &mut WorkerMap,
    submission_map: &mut SubmissionMap,
    kicked_paths: &mut HashSet<PathBuf>,
    events_tx: &Sender<Event>,
    config: &IoConfig,
    stop_notifier: &Arc<StopNotifier>,
) -> io::Result<()> {
    match effect {
        Effect::TaskAssigned { worker_id, task_id } => {
            match task_id {
                TaskId::External(submission_id) => {
                    let submission_data = submission_map
                        .get_data(submission_id)
                        .expect("TaskAssigned for unknown submission - core bug");

                    // Submission format is identical to worker format:
                    // {"kind": "Task", "task": {"instructions": "...", "data": {...}}}
                    // Pass through directly.
                    worker_map
                        .write_to(worker_id, TASK_FILE, &submission_data.content)
                        .expect("TaskAssigned for unknown worker - core bug");

                    start_task_timeout_timer(
                        events_tx.clone(),
                        worker_id,
                        submission_data.timeout,
                        Arc::clone(stop_notifier),
                    );
                }
                TaskId::Heartbeat => {
                    let heartbeat = serde_json::json!({
                        "kind": "Heartbeat",
                        "task": {
                            "instructions": "Respond with any valid JSON to confirm you're alive. The daemon discards your response - this exists to detect stuck workers.",
                            "data": null,
                        },
                    });
                    worker_map
                        .write_to(worker_id, TASK_FILE, &heartbeat.to_string())
                        .expect("TaskAssigned for unknown worker - core bug");

                    start_task_timeout_timer(
                        events_tx.clone(),
                        worker_id,
                        config.idle_timeout,
                        Arc::clone(stop_notifier),
                    );
                }
            }
        }
        Effect::WorkerWaiting { worker_id } => {
            if config.heartbeat_enabled {
                start_idle_timer(
                    events_tx.clone(),
                    worker_id,
                    config.idle_timeout,
                    Arc::clone(stop_notifier),
                );
            }
        }
        Effect::TaskCompleted { worker_id, task_id } => {
            let worker_path = worker_map
                .get_path(worker_id)
                .expect("TaskCompleted for unknown worker - core bug");

            match task_id {
                TaskId::Heartbeat => {
                    let _ = fs::remove_file(worker_path.join(TASK_FILE));
                    let _ = fs::remove_file(worker_path.join(RESPONSE_FILE));
                }
                TaskId::External(submission_id) => {
                    let worker_output = worker_map
                        .read_from(worker_id, RESPONSE_FILE)
                        .expect("TaskCompleted for unknown worker - core bug");

                    let _ = fs::remove_file(worker_path.join(TASK_FILE));
                    let _ = fs::remove_file(worker_path.join(RESPONSE_FILE));

                    // Wrap worker output in typed Response
                    let response = Response::processed(worker_output);
                    let response_json = serde_json::to_string(&response)
                        .expect("Response serialization cannot fail");
                    submission_map.finish(submission_id, &response_json)?;
                }
            }
        }
        Effect::TaskFailed { submission_id } => {
            let response = Response::not_processed(NotProcessedReason::Timeout);
            let response_json =
                serde_json::to_string(&response).expect("Response serialization cannot fail");
            submission_map.finish(submission_id, &response_json)?;

            warn!(submission_id = submission_id.0, "task failed (timeout)");
        }
        Effect::WorkerRemoved { worker_id } => {
            let (transport, ()) = worker_map
                .remove(worker_id)
                .expect("WorkerRemoved for unknown worker - core bug");

            // Write kicked message so worker knows it was removed
            let kicked_msg = serde_json::json!({
                "kind": "Kicked",
                "reason": "Timeout"
            });
            let _ = transport.write(TASK_FILE, &kicked_msg.to_string());

            // Track this path so we reject re-registration attempts
            if let Some(worker_path) = transport.path() {
                kicked_paths.insert(worker_path.to_path_buf());
            }
        }
    }
    Ok(())
}

/// Start a task timeout timer that sends `WorkerTimedOut` after the given duration.
///
/// The timer uses an interruptible wait. If stop is signaled during the wait,
/// the timer exits immediately without sending an event.
fn start_task_timeout_timer(
    events_tx: Sender<Event>,
    worker_id: WorkerId,
    timeout: Duration,
    stop_notifier: Arc<StopNotifier>,
) {
    thread::spawn(move || {
        // Wait for timeout or stop, whichever comes first
        let stopped = stop_notifier.wait_timeout(timeout);
        if stopped {
            debug!(
                worker_id = worker_id.0,
                "task timeout timer cancelled due to stop"
            );
        } else {
            // Timeout elapsed without stop - fire the event
            let _ = events_tx.send(Event::WorkerTimedOut { worker_id });
        }
    });
}

/// Start an idle timer that sends `AssignHeartbeatIfIdle` after the given duration.
///
/// The timer uses an interruptible wait. If stop is signaled during the wait,
/// the timer exits immediately without sending an event.
fn start_idle_timer(
    events_tx: Sender<Event>,
    worker_id: WorkerId,
    timeout: Duration,
    stop_notifier: Arc<StopNotifier>,
) {
    thread::spawn(move || {
        // Wait for timeout or stop, whichever comes first
        let stopped = stop_notifier.wait_timeout(timeout);
        if stopped {
            debug!(worker_id = worker_id.0, "idle timer cancelled due to stop");
        } else {
            // Timeout elapsed without stop - fire the event
            let _ = events_tx.send(Event::AssignHeartbeatIfIdle { worker_id });
        }
    });
}
