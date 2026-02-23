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
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tracing::{debug, trace, warn};

use crate::constants::{RESPONSE_FILE, TASK_FILE};

use super::core::{AgentId, Effect, Epoch, Event, TaskId};

// =============================================================================
// Configuration
// =============================================================================

/// I/O configuration.
#[derive(Debug, Clone)]
pub(super) struct IoConfig {
    /// How long an idle agent can wait before being deregistered.
    /// Agents that are still alive will re-register by calling `get_task` again.
    pub idle_agent_timeout: Duration,
    /// Default timeout for tasks (used when submission doesn't specify one).
    pub default_task_timeout: Duration,
}

impl Default for IoConfig {
    fn default() -> Self {
        Self {
            idle_agent_timeout: Duration::from_secs(60),
            default_task_timeout: Duration::from_secs(300),
        }
    }
}

// =============================================================================
// Transport
// =============================================================================

/// Communication transport for agents and submissions.
#[derive(Debug)]
pub(super) enum Transport {
    /// Filesystem-based transport using a directory.
    Directory(PathBuf),
    // TODO: Socket(Stream) - for socket-based submissions
}

impl Transport {
    /// Read content from a file in this transport.
    pub fn read(&self, filename: &str) -> io::Result<String> {
        match self {
            Transport::Directory(path) => fs::read_to_string(path.join(filename)),
        }
    }

    /// Write content to a file in this transport.
    pub fn write(&self, filename: &str, content: &str) -> io::Result<()> {
        match self {
            Transport::Directory(path) => fs::write(path.join(filename), content),
        }
    }

    /// Get the path for directory-based transports.
    pub fn path(&self) -> &Path {
        match self {
            Transport::Directory(path) => path,
        }
    }
}

// =============================================================================
// Transport ID Trait
// =============================================================================

/// Trait for IDs that can be used with TransportMap.
pub(super) trait TransportId: Copy + Eq + std::hash::Hash + std::fmt::Debug + From<u32> {
    /// Data stored alongside the transport for this ID type.
    type Data: std::fmt::Debug;
}

impl TransportId for AgentId {
    type Data = ();
}

// =============================================================================
// Task ID Types (I/O Layer)
// =============================================================================

/// External task ID - a real submission with a respond-to transport.
///
/// Wraps `core::TaskId` to distinguish external tasks from heartbeats at the type level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExternalTaskId(pub(super) TaskId);

impl ExternalTaskId {
    /// Get the underlying core task ID.
    pub fn core_id(self) -> TaskId {
        self.0
    }
}

/// Heartbeat ID - a synthetic task with no transport.
///
/// Wraps `core::TaskId` to distinguish heartbeats from external tasks at the type level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct HeartbeatId(pub(super) TaskId);

impl HeartbeatId {
    /// Get the underlying core task ID.
    pub fn core_id(self) -> TaskId {
        self.0
    }
}

/// Allocates task IDs for core, tracking whether each is external or heartbeat.
#[derive(Debug, Default)]
pub(super) struct TaskIdAllocator {
    next_id: u32,
}

impl TaskIdAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate an ID for an external task.
    pub fn allocate_external(&mut self) -> ExternalTaskId {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        ExternalTaskId(id)
    }

    /// Allocate an ID for a heartbeat.
    pub fn allocate_heartbeat(&mut self) -> HeartbeatId {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        HeartbeatId(id)
    }
}

/// Data stored per external task submission.
#[derive(Debug)]
pub(super) struct ExternalTaskData {
    /// The task content to send to the agent.
    pub content: String,
    /// How long the agent has to complete this task.
    pub timeout: Duration,
}

impl TransportId for ExternalTaskId {
    type Data = ExternalTaskData;
}

impl From<u32> for ExternalTaskId {
    fn from(id: u32) -> Self {
        ExternalTaskId(TaskId(id))
    }
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

    /// Register a directory-based transport with associated data.
    ///
    /// Returns `None` if the path is already registered (duplicate FS event).
    pub fn register_directory(&mut self, path: PathBuf, data: Id::Data) -> Option<Id> {
        let id = self.allocate_id();
        if self.register(id, path, data) {
            Some(id)
        } else {
            None
        }
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
        let Transport::Directory(ref path) = entry.0;
        self.path_to_id.remove(path);
        Some(entry)
    }

    /// Write content to a file in the transport for the given ID.
    pub fn write_to(&self, id: Id, filename: &str, content: &str) -> io::Result<()> {
        let transport = self
            .get_transport(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "id not found"))?;
        transport.write(filename, content)
    }

    /// Read content from a file in the transport for the given ID.
    pub fn read_from(&self, id: Id, filename: &str) -> io::Result<String> {
        let transport = self
            .get_transport(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "id not found"))?;
        transport.read(filename)
    }

    /// Get the path for the given ID (for directory-based transports).
    pub fn get_path(&self, id: Id) -> Option<&Path> {
        self.get_transport(id).map(Transport::path)
    }
}

// =============================================================================
// Type Aliases
// =============================================================================

/// Map of agents to their transports.
pub(super) type AgentMap = TransportMap<AgentId>;

/// Map of external tasks to their transports and data.
pub(super) type ExternalTaskMap = TransportMap<ExternalTaskId>;

// =============================================================================
// ExternalTaskMap Extensions
// =============================================================================

impl ExternalTaskMap {
    /// Finish a task: write response to transport and remove from map.
    ///
    /// Used for both success and failure - the response content determines the outcome.
    pub fn finish(&mut self, id: ExternalTaskId, response: &str) -> io::Result<ExternalTaskData> {
        let (transport, data) = self
            .remove(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "task not found"))?;
        transport.write(RESPONSE_FILE, response)?;
        Ok(data)
    }

    /// Look up an external task by its core task ID.
    pub fn get_by_core_id(&self, core_id: TaskId) -> Option<ExternalTaskId> {
        // ExternalTaskId wraps TaskId, so we can construct it and check if it exists
        let external_id = ExternalTaskId(core_id);
        if self.get_transport(external_id).is_some() {
            Some(external_id)
        } else {
            None
        }
    }
}

// =============================================================================
// Effect Execution
// =============================================================================

/// Execute an effect, performing the actual I/O.
///
/// # Arguments
///
/// * `effect` - The effect to execute
/// * `agent_map` - Map of agents to transports
/// * `external_task_map` - Map of external tasks to transports/data
/// * `heartbeat_ids` - Set of heartbeat IDs (synthetic tasks with no transport)
/// * `task_id_allocator` - Allocator for new task IDs
/// * `kicked_paths` - Set of agent paths that have been kicked (for rejection on re-register)
/// * `events_tx` - Channel to send timeout events
/// * `config` - I/O configuration
///
/// # Errors
///
/// Returns an error if I/O operations fail (writing task files, reading responses).
///
/// # Panics
///
/// Panics if the effect references an ID that doesn't exist. This indicates a
/// core bug, since core should only emit effects for IDs it knows about.
#[allow(clippy::expect_used)] // Internal invariants - effects reference valid IDs
pub(super) fn execute_effect(
    effect: Effect,
    agent_map: &mut AgentMap,
    external_task_map: &mut ExternalTaskMap,
    heartbeat_ids: &mut HashSet<HeartbeatId>,
    task_id_allocator: &mut TaskIdAllocator,
    kicked_paths: &mut HashSet<PathBuf>,
    events_tx: &mpsc::Sender<Event>,
    config: &IoConfig,
) -> io::Result<()> {
    match effect {
        Effect::TaskAssigned { task_id, epoch } => {
            // Check if this is an external task or heartbeat
            if let Some(external_id) = external_task_map.get_by_core_id(task_id) {
                // External task - write task content
                let task_data = external_task_map
                    .get_data(external_id)
                    .expect("TaskAssigned for unknown task - core bug");

                let content_value = serde_json::from_str::<serde_json::Value>(&task_data.content)
                    .unwrap_or_else(|_| serde_json::Value::String(task_data.content.clone()));
                let envelope = serde_json::json!({
                    "kind": "Task",
                    "content": content_value,
                });
                agent_map
                    .write_to(epoch.agent_id, TASK_FILE, &envelope.to_string())
                    .expect("TaskAssigned for unknown agent - core bug");

                debug!(
                    agent_id = epoch.agent_id.0,
                    task_id = task_id.0,
                    "dispatched task"
                );

                // Start timeout timer for task
                start_timeout_timer(events_tx.clone(), epoch, task_data.timeout);
            } else if heartbeat_ids.contains(&HeartbeatId(task_id)) {
                // Heartbeat - write heartbeat message
                let heartbeat = serde_json::json!({
                    "kind": "Heartbeat",
                });
                agent_map
                    .write_to(epoch.agent_id, TASK_FILE, &heartbeat.to_string())
                    .expect("TaskAssigned for unknown agent - core bug");

                debug!(
                    agent_id = epoch.agent_id.0,
                    task_id = task_id.0,
                    "dispatched heartbeat"
                );

                // Start timeout timer for heartbeat
                start_timeout_timer(events_tx.clone(), epoch, config.idle_agent_timeout);
            } else {
                panic!("TaskAssigned for unknown task {task_id:?} - core bug");
            }
        }
        Effect::AgentIdled { epoch } => {
            // Allocate a heartbeat ID
            let heartbeat_id = task_id_allocator.allocate_heartbeat();
            heartbeat_ids.insert(heartbeat_id);

            // Start idle timer - when it fires, core will assign the heartbeat
            start_idle_timer(
                events_tx.clone(),
                epoch,
                heartbeat_id.core_id(),
                config.idle_agent_timeout,
            );
            trace!(
                agent_id = epoch.agent_id.0,
                heartbeat_task_id = heartbeat_id.core_id().0,
                "started idle timer"
            );
        }
        Effect::TaskCompleted { agent_id, task_id } => {
            let agent_path = agent_map
                .get_path(agent_id)
                .expect("TaskCompleted for unknown agent - core bug");

            if heartbeat_ids.remove(&HeartbeatId(task_id)) {
                // Heartbeat completed - no submitter to notify, just clean up
                let _ = fs::remove_file(agent_path.join(TASK_FILE));
                let _ = fs::remove_file(agent_path.join(RESPONSE_FILE));

                debug!(agent_id = agent_id.0, task_id = task_id.0, "heartbeat completed");
            } else if let Some(external_id) = external_task_map.get_by_core_id(task_id) {
                // External task - read response first, then clean up
                let response = agent_map
                    .read_from(agent_id, RESPONSE_FILE)
                    .expect("TaskCompleted for unknown agent - core bug");

                // Clean up agent's task and response files so it can receive new tasks
                let _ = fs::remove_file(agent_path.join(TASK_FILE));
                let _ = fs::remove_file(agent_path.join(RESPONSE_FILE));

                // Send response to submitter
                external_task_map.finish(external_id, &response)?;

                debug!(agent_id = agent_id.0, task_id = task_id.0, "task completed");
            } else {
                panic!("TaskCompleted for unknown task {task_id:?} - core bug");
            }
        }
        Effect::TaskFailed { task_id } => {
            if heartbeat_ids.remove(&HeartbeatId(task_id)) {
                // Heartbeat timed out - no submitter to notify
                debug!(task_id = task_id.0, "heartbeat timed out");
            } else if let Some(external_id) = external_task_map.get_by_core_id(task_id) {
                // External task timed out - notify submitter
                let error = serde_json::json!({
                    "status": "NotProcessed",
                    "reason": "AgentTimeout"
                });
                external_task_map.finish(external_id, &error.to_string())?;

                warn!(task_id = task_id.0, "task failed (timeout)");
            } else {
                panic!("TaskFailed for unknown task {task_id:?} - core bug");
            }
        }
        Effect::AgentRemoved { agent_id } => {
            let (transport, ()) = agent_map
                .remove(agent_id)
                .expect("AgentRemoved for unknown agent - core bug");
            let agent_path = transport.path().to_path_buf();

            // Write kicked message so agent knows it was removed
            let kicked_msg = serde_json::json!({
                "kind": "Kicked",
                "reason": "Timeout"
            });
            let _ = transport.write(TASK_FILE, &kicked_msg.to_string());

            // Track this path so we reject re-registration attempts
            kicked_paths.insert(agent_path);

            debug!(agent_id = agent_id.0, "agent kicked");
        }
    }
    Ok(())
}

/// Start a task/heartbeat timeout timer that sends `AgentTimedOut` after the given duration.
///
/// The timer is "fire and forget" - core ignores it if the epoch doesn't match.
fn start_timeout_timer(events_tx: mpsc::Sender<Event>, epoch: Epoch, timeout: Duration) {
    thread::spawn(move || {
        thread::sleep(timeout);
        let _ = events_tx.send(Event::AgentTimedOut { epoch });
    });
}

/// Start an idle timer that sends `AssignTaskToAgentIfEpochMatches` after the given duration.
///
/// When this fires, core will assign the heartbeat task to the agent if epoch still matches.
fn start_idle_timer(
    events_tx: mpsc::Sender<Event>,
    epoch: Epoch,
    heartbeat_task_id: TaskId,
    timeout: Duration,
) {
    thread::spawn(move || {
        thread::sleep(timeout);
        let _ = events_tx.send(Event::AssignTaskToAgentIfEpochMatches {
            epoch,
            task_id: heartbeat_task_id,
        });
    });
}

// =============================================================================
// Event Parsing
// =============================================================================

/// Category of a filesystem path.
#[derive(Debug)]
pub(super) enum PathCategory {
    /// Agent directory: `agents/<name>/`
    AgentDir {
        /// The agent's directory name.
        name: String,
    },
    /// Agent response file: `agents/<name>/response.json`
    AgentResponse {
        /// The agent's directory name.
        name: String,
    },
    /// Pending submission directory: `pending/<uuid>/`
    PendingDir {
        /// The submission's UUID.
        uuid: String,
    },
    /// Pending submission task file: `pending/<uuid>/task.json`
    PendingTask {
        /// The submission's UUID.
        uuid: String,
    },
}

/// Categorize a filesystem path relative to the pool root.
#[must_use]
pub(super) fn categorize_path(path: &Path, agents_dir: &Path, pending_dir: &Path) -> Option<PathCategory> {
    // Check if it's under agents/
    if let Ok(relative) = path.strip_prefix(agents_dir) {
        let components: Vec<_> = relative.components().collect();
        if components.is_empty() {
            return None;
        }
        let name = components[0].as_os_str().to_str()?.to_string();

        if components.len() == 1 {
            // agents/<name>/ directory itself
            return Some(PathCategory::AgentDir { name });
        } else if components.len() == 2 {
            let filename = components[1].as_os_str().to_str()?;
            if filename == RESPONSE_FILE {
                return Some(PathCategory::AgentResponse { name });
            }
        }
        return None;
    }

    // Check if it's under pending/
    if let Ok(relative) = path.strip_prefix(pending_dir) {
        let components: Vec<_> = relative.components().collect();
        if components.len() == 1 {
            // pending/<uuid>/ directory itself
            let uuid = components[0].as_os_str().to_str()?.to_string();
            return Some(PathCategory::PendingDir { uuid });
        } else if components.len() == 2 {
            let uuid = components[0].as_os_str().to_str()?.to_string();
            let filename = components[1].as_os_str().to_str()?;
            if filename == TASK_FILE {
                return Some(PathCategory::PendingTask { uuid });
            }
        }
    }

    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn agent_map_register_and_lookup() {
        let mut map = AgentMap::new();
        let path = PathBuf::from("/tmp/test/agents/agent-1");

        let id = map.register_directory(path.clone(), ()).unwrap();
        assert_eq!(id, AgentId(0));

        // Look up by ID
        assert!(map.get_transport(id).is_some());

        // Look up by path
        assert_eq!(map.get_id_by_path(&path), Some(id));

        // Duplicate registration returns None
        assert!(map.register_directory(path, ()).is_none());
    }

    #[test]
    fn agent_map_remove() {
        let mut map = AgentMap::new();
        let path = PathBuf::from("/tmp/test/agents/agent-1");

        let id = map.register_directory(path.clone(), ()).unwrap();
        let (transport, ()) = map.remove(id).unwrap();

        assert!(matches!(transport, Transport::Directory(_)));
        assert!(map.get_transport(id).is_none());
        assert!(map.get_id_by_path(&path).is_none());
    }

    #[test]
    fn external_task_map_register_and_finish() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("submission-1");
        fs::create_dir_all(&path).unwrap();

        let mut map = ExternalTaskMap::new();
        let id = map
            .register_directory(
                path.clone(),
                ExternalTaskData {
                    content: "test content".to_string(),
                    timeout: Duration::from_secs(60),
                },
            )
            .unwrap();

        assert_eq!(id, ExternalTaskId(TaskId(0)));
        assert_eq!(map.get_data(id).unwrap().content, "test content");

        // Finish the task
        map.finish(id, r#"{"result": "ok"}"#).unwrap();

        // Task should be removed
        assert!(map.get_data(id).is_none());

        // Response should be written
        let response = fs::read_to_string(path.join(RESPONSE_FILE)).unwrap();
        assert_eq!(response, r#"{"result": "ok"}"#);
    }

    #[test]
    fn categorize_path_agents() {
        let agents_dir = PathBuf::from("/pool/agents");
        let pending_dir = PathBuf::from("/pool/pending");

        // Agent directory
        let path = PathBuf::from("/pool/agents/claude-1");
        let cat = categorize_path(&path, &agents_dir, &pending_dir).unwrap();
        assert!(matches!(cat, PathCategory::AgentDir { name } if name == "claude-1"));

        // Agent response
        let path = PathBuf::from("/pool/agents/claude-1/response.json");
        let cat = categorize_path(&path, &agents_dir, &pending_dir).unwrap();
        assert!(matches!(cat, PathCategory::AgentResponse { name } if name == "claude-1"));

        // Agent task file (not categorized)
        let path = PathBuf::from("/pool/agents/claude-1/task.json");
        assert!(categorize_path(&path, &agents_dir, &pending_dir).is_none());
    }

    #[test]
    fn categorize_path_pending() {
        let agents_dir = PathBuf::from("/pool/agents");
        let pending_dir = PathBuf::from("/pool/pending");

        // Pending directory
        let path = PathBuf::from("/pool/pending/abc123");
        let cat = categorize_path(&path, &agents_dir, &pending_dir).unwrap();
        assert!(matches!(cat, PathCategory::PendingDir { uuid } if uuid == "abc123"));

        // Pending task
        let path = PathBuf::from("/pool/pending/abc123/task.json");
        let cat = categorize_path(&path, &agents_dir, &pending_dir).unwrap();
        assert!(matches!(cat, PathCategory::PendingTask { uuid } if uuid == "abc123"));

        // Pending response (not categorized - we write responses, not read them)
        let path = PathBuf::from("/pool/pending/abc123/response.json");
        assert!(categorize_path(&path, &agents_dir, &pending_dir).is_none());
    }

    #[test]
    fn categorize_path_unrelated() {
        let agents_dir = PathBuf::from("/pool/agents");
        let pending_dir = PathBuf::from("/pool/pending");

        let path = PathBuf::from("/other/path");
        assert!(categorize_path(&path, &agents_dir, &pending_dir).is_none());
    }
}
