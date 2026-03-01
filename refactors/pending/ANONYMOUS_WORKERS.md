# Anonymous Worker Model

**Status:** Ready for implementation

## Overview

Simplify agent protocol from "named agents with persistent directories" to "anonymous workers with flat files." This eliminates the inotify race condition on Linux and simplifies the protocol.

## Goals

1. **Eliminate per-agent directories** - Use flat files like submissions already do
2. **Simplify registration** - Worker creates one file, waits for response
3. **Remove agent identity** - Workers are anonymous, names are debug-only metadata
4. **Unify patterns** - Agents and submissions use identical flat-file protocol
5. **Simplify executor** - Use `VerifiedWatcher` directly, eliminate custom watcher code

## Current Architecture

### Directory Structure (Before)

```
<pool>/
├── agents/
│   ├── claude-1/           # Per-agent directory
│   │   ├── task.json       # Daemon writes task here
│   │   └── response.json   # Agent writes response here
│   └── claude-2/
│       ├── task.json
│       └── response.json
└── submissions/
    ├── <uuid>.request.json   # Submitter writes request
    └── <uuid>.response.json  # Daemon writes response
```

### PathCategory (Before)

**File:** `crates/agent_pool/src/daemon/path_category.rs`

```rust
pub(super) enum PathCategory {
    /// Agent directory: `agents/<name>/`
    AgentDir { name: String },
    /// Agent response file: `agents/<name>/response.json`
    AgentResponse { name: String },
    /// Submission request file: `submissions/<id>.request.json`
    SubmissionRequest { id: String },
}
```

**Problems:**
- `AgentDir` requires folder creation/deletion events (inotify race on Linux)
- `AgentResponse` is nested two levels deep
- Asymmetric: submissions are flat, agents are nested

### Current worker.rs

**File:** `crates/agent_pool/src/worker.rs`

Contains:
- `is_file_write_event()` - platform-specific event detection (DUPLICATED in `daemon/path_category.rs` as `is_write_complete()`)
- `AgentEvent` enum
- `create_watcher()` - creates notify watcher
- `verify_watcher_sync()` - canary verification
- `is_task_ready()` - checks `task.exists() && !response.exists()`
- `wait_for_task()`, `wait_for_task_with_timeout()`

**After anonymous workers:** Most of this becomes unnecessary because:
- `VerifiedWatcher` handles watcher creation + canary verification
- Simple `task.exists()` check (no response file ambiguity with fresh UUIDs)
- Platform-specific code consolidated in one place

---

## Proposed Architecture

### Directory Structure (After)

```
<pool>/
├── agents/
│   ├── <uuid>.ready.json     # Worker writes (signals availability)
│   ├── <uuid>.task.json      # Daemon writes (assigns task)
│   └── <uuid>.response.json  # Worker writes (task result)
└── submissions/
    ├── <uuid>.request.json   # Submitter writes request
    └── <uuid>.response.json  # Daemon writes response
```

Both agents and submissions use flat files with UUID-based naming.

### PathCategory (After)

**File:** `crates/agent_pool/src/daemon/path_category.rs`

```rust
pub(super) enum PathCategory {
    /// Worker ready file: `agents/<id>.ready.json`
    WorkerReady { id: String },
    /// Worker response file: `agents/<id>.response.json`
    WorkerResponse { id: String },
    /// Submission request file: `submissions/<id>.request.json`
    SubmissionRequest { id: String },
}
```

**Changes:**
- Remove `AgentDir` (no more folder events)
- Rename `AgentResponse` → `WorkerResponse`
- Add `WorkerReady` for flat file registration

### Worker Registration Flow (After)

1. Worker generates UUID, writes `agents/<uuid>.ready.json`
2. Daemon sees `FileWritten` event → `PathCategory::WorkerReady`
3. Daemon assigns task, writes `agents/<uuid>.task.json`, deletes ready file (RAII guard)
4. Worker reads task, processes, writes `agents/<uuid>.response.json`
5. Daemon sees `FileWritten` event → `PathCategory::WorkerResponse`
6. Daemon reads response, deletes task file (RAII guard), deletes response file
7. Worker generates new UUID, repeats from step 1

**No race condition:** All events are file writes, which are reliable on both Linux and macOS.

### Simplified worker.rs (After)

With anonymous workers + `VerifiedWatcher`, the executor becomes trivial:

```rust
//! Task execution utilities.

use crate::fs::VerifiedWatcher;
use crate::constants::AGENTS_DIR;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

/// Wait for a task assignment.
///
/// Writes a ready file, waits for task file to appear, returns task content.
pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    timeout: Option<Duration>,
) -> io::Result<(String, String)> {  // Returns (uuid, task_content)
    let agents_dir = pool_root.join(AGENTS_DIR);
    let uuid = Uuid::new_v4().to_string();

    let ready_path = agents_dir.join(format!("{uuid}.ready.json"));
    let task_path = agents_dir.join(format!("{uuid}.task.json"));
    let canary_path = agents_dir.join(format!("{uuid}.canary"));

    // Write ready file with optional metadata
    let metadata = match name {
        Some(n) => format!(r#"{{"name":"{}"}}"#, n),
        None => "{}".to_string(),
    };
    fs::write(&ready_path, &metadata)?;

    // Wait for task using VerifiedWatcher
    let mut watcher = VerifiedWatcher::new(&agents_dir, canary_path)?;
    watcher.wait_for(&task_path, timeout)?;

    let task = fs::read_to_string(&task_path)?;
    Ok((uuid, task))
}
```

This significantly simplifies the executor code.

---

## Implementation Plan

### Task 0: Consolidate Platform-Specific Code

**Problem:** `is_file_write_event()` in `worker.rs` duplicates `is_write_complete()` in `daemon/path_category.rs`.

**File:** `crates/agent_pool/src/fs.rs` (add to existing file)

```rust
/// Check if event kind indicates a file write is complete.
///
/// Platform-specific behavior:
/// - **Linux inotify**: Only `Close(Write)` guarantees data is flushed
/// - **macOS FSEvents**: `Create(File)` and `Modify(Data)` are accepted
#[cfg(target_os = "linux")]
pub const fn is_write_complete(kind: notify::EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode, ModifyKind};
    matches!(
        kind,
        notify::EventKind::Access(AccessKind::Close(AccessMode::Write))
            | notify::EventKind::Modify(ModifyKind::Name(_))
    )
}

#[cfg(target_os = "macos")]
pub const fn is_write_complete(kind: notify::EventKind) -> bool {
    use notify::event::{CreateKind, ModifyKind};
    matches!(
        kind,
        notify::EventKind::Create(CreateKind::File)
            | notify::EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_))
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub const fn is_write_complete(kind: notify::EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind};
    matches!(
        kind,
        notify::EventKind::Access(AccessKind::Close(AccessMode::Write))
            | notify::EventKind::Create(CreateKind::File)
            | notify::EventKind::Modify(ModifyKind::Data(_))
            | notify::EventKind::Modify(ModifyKind::Name(_))
    )
}
```

Then update:
- `daemon/path_category.rs` - use `crate::fs::is_write_complete`
- `worker.rs` - use `crate::fs::is_write_complete`

---

### Task 1: Update PathCategory

**File:** `crates/agent_pool/src/daemon/path_category.rs`

#### 1.1: Change enum variants

```rust
// Before
pub(super) enum PathCategory {
    AgentDir { name: String },
    AgentResponse { name: String },
    SubmissionRequest { id: String },
}

// After
pub(super) enum PathCategory {
    WorkerReady { id: String },
    WorkerResponse { id: String },
    SubmissionRequest { id: String },
}
```

#### 1.2: Update categorize_under_agents

```rust
// Before
fn categorize_under_agents(
    path: &Path,
    event_kind: EventKind,
    agents_dir: &Path,
) -> Option<PathCategory> {
    let relative = path.strip_prefix(agents_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    let name = components[0].as_os_str().to_str()?.to_string();

    match components.len() {
        1 if is_folder_created(event_kind) || is_folder_removed(event_kind) => {
            Some(PathCategory::AgentDir { name })
        }
        2 if is_write_complete(event_kind) => {
            let filename = components[1].as_os_str().to_str()?;
            if filename == RESPONSE_FILE {
                Some(PathCategory::AgentResponse { name })
            } else {
                None
            }
        }
        _ => None,
    }
}

// After
fn categorize_under_agents(
    path: &Path,
    event_kind: EventKind,
    agents_dir: &Path,
) -> Option<PathCategory> {
    use crate::constants::{READY_SUFFIX, WORKER_RESPONSE_SUFFIX};

    // Only process when write is complete (same as submissions)
    if !is_write_complete(event_kind) {
        return None;
    }

    let relative = path.strip_prefix(agents_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    // Must be exactly one component (flat file)
    if components.len() != 1 {
        return None;
    }

    let filename = components[0].as_os_str().to_str()?;

    if let Some(id) = filename.strip_suffix(READY_SUFFIX) {
        return Some(PathCategory::WorkerReady { id: id.to_string() });
    }
    if let Some(id) = filename.strip_suffix(WORKER_RESPONSE_SUFFIX) {
        return Some(PathCategory::WorkerResponse { id: id.to_string() });
    }
    None
}
```

#### 1.3: Remove folder event helpers

Delete `is_folder_created()` and `is_folder_removed()` - no longer needed.

#### 1.4: Update tests

Update all `PathCategory` tests to use flat file patterns instead of nested directories.

---

### Task 2: Update Constants

**File:** `crates/agent_pool/src/constants.rs`

```rust
// Before
pub const TASK_FILE: &str = "task.json";
pub const RESPONSE_FILE: &str = "response.json";

// After - add suffixes for flat worker files
pub const READY_SUFFIX: &str = ".ready.json";
pub const TASK_SUFFIX: &str = ".task.json";
pub const WORKER_RESPONSE_SUFFIX: &str = ".response.json";

// Keep TASK_FILE and RESPONSE_FILE for now (remove in later cleanup)
// Or remove if no longer used after all changes
```

---

### Task 3: Update Core State Machine

**File:** `crates/agent_pool/src/daemon/core.rs`

Core is the pure state machine - it deals only with IDs and state transitions, no files. The anonymous worker model simplifies core because workers are **one-shot**: they're removed after completing a task, not returned to idle.

#### What stays the same in core

- `WorkerStatus` is still `Idle` or `Busy { task_id }` - a worker either has a task or doesn't
- Epoch-based timeout validation works the same way
- Task queue (pending tasks) works the same way

#### What changes in core

1. **Renames**: Agent → Worker throughout
2. **Behavioral change**: After task completion, worker is **removed** (not returned to idle)
3. **Remove `AgentDeregistered`**: Workers don't deregister, they just get removed on timeout or completion

#### IO Layer Guarantees (enables panics in core)

The IO layer maintains a strict state machine per UUID:
```
ready.json created → task.json created → response.json created → cleanup
```

**Guarantees IO provides to core:**
1. **UUIDs never reused** - each worker gets a fresh UUID
2. **WorkerReady sent exactly once per worker_id** - never duplicates
3. **WorkerResponded only sent for Assigned workers** - IO checks its own state before sending
4. **State transitions are sequential** - Ready → Assigned, never backwards

Because of these guarantees, core can **panic on violations** rather than handling them defensively. If core sees an invalid event, it's a bug in IO or core, not external input.

#### 3.1: Complete Core Data Structures

```rust
// =============================================================================
// ID Types
// =============================================================================

/// External task ID - a real submission from a client.
pub(super) struct ExternalTaskId(pub(super) u32);

/// Heartbeat ID - a synthetic task to validate worker liveness.
pub(super) struct HeartbeatId(pub(super) u32);

/// Task identifier - either an external submission or a heartbeat.
pub(super) enum TaskId {
    External(ExternalTaskId),
    Heartbeat(HeartbeatId),
}

/// Worker identity is just the UUID string. No separate ID type needed.
pub(super) type Uuid = String;

// =============================================================================
// Pool State
// =============================================================================

/// The complete state of the worker pool.
///
/// Workers are either waiting (no task) or busy (has task).
pub(super) struct PoolState {
    pending_tasks: VecDeque<TaskId>,
    waiting_workers: VecDeque<Uuid>,
    busy_workers: HashMap<Uuid, TaskId>,
}
```

No epochs, no WorkerId. UUID is the identity. State is implicit in which collection the worker is in.

#### 3.2: Update Events

```rust
// Before
pub(super) enum Event {
    TaskSubmitted { task_id: TaskId },
    TaskWithdrawn { task_id: TaskId },
    AgentRegistered { agent_id: AgentId, heartbeat_task_id: Option<TaskId> },
    AgentDeregistered { agent_id: AgentId },
    AgentResponded { agent_id: AgentId },
    AgentTimedOut { epoch: Epoch },
    AssignTaskToAgentIfEpochMatches { epoch: Epoch, task_id: TaskId },
}

// After
pub(super) enum Event {
    TaskSubmitted { task_id: TaskId },
    TaskWithdrawn { task_id: TaskId },
    WorkerReady { uuid: Uuid },
    WorkerResponded { uuid: Uuid },
    WorkerTimedOut { uuid: Uuid },
    AssignHeartbeatIfIdle { uuid: Uuid },
}
```

**Changes:**
- Remove `AgentDeregistered` - workers don't deregister
- `WorkerTimedOut` takes `worker_id`, not epoch - check if worker is busy
- `AssignTaskToAgentIfEpochMatches` → `AssignHeartbeatIfIdle` - check if worker is idle

#### 3.3: Update Effects

```rust
// Before
pub(super) enum Effect {
    TaskAssigned { task_id: TaskId, epoch: Epoch },
    AgentIdled { epoch: Epoch },
    TaskCompleted { agent_id: AgentId, task_id: TaskId },
    TaskFailed { task_id: TaskId },
    AgentRemoved { agent_id: AgentId },
}

// After
pub(super) enum Effect {
    TaskAssigned { uuid: Uuid, task_id: TaskId },
    WorkerWaiting { uuid: Uuid },
    TaskCompleted { uuid: Uuid, task_id: TaskId },  // implies worker removal
    TaskFailed { task_id: TaskId },
    WorkerRemoved { uuid: Uuid },  // only for timeouts/kicks
}
```

**Changes:**
- No epochs anywhere - just worker_id
- `AgentIdled` → `WorkerWaiting` (waiting for first task)
- `TaskCompleted` implies worker removal (matching service)
- `WorkerRemoved` is only for task timeout

Note: There's no "idle timeout → remove" path. When a worker is idle too long, it gets a heartbeat task (becomes Busy). If it fails to respond to the heartbeat, that's a task timeout.

#### 3.4: Event Handlers

**Event sources and race analysis:**

| Source | Events | Can race with |
|--------|--------|---------------|
| FS (watcher) | WorkerReady, WorkerResponded | Timers only |
| Timers | WorkerTimedOut, AssignHeartbeatIfIdle | Everything |
| Submissions | TaskSubmitted, TaskWithdrawn | TaskWithdrawn (same submission) |

**FS events are sequenced** by the IO layer. **Timer events need defensive handling** (worker might be gone or in different state).

```rust
fn handle_worker_registered(mut state: PoolState, uuid: Uuid) -> (PoolState, Vec<Effect>) {
    assert!(
        !state.waiting_workers.contains(&uuid) && !state.busy_workers.contains_key(&uuid),
        "WorkerReady for existing worker {uuid}"
    );

    if let Some(task_id) = state.pending_tasks.pop_front() {
        state.busy_workers.insert(uuid.clone(), task_id);
        (state, vec![Effect::TaskAssigned { uuid, task_id }])
    } else {
        state.waiting_workers.push_back(uuid.clone());
        (state, vec![Effect::WorkerWaiting { uuid }])
    }
}

fn handle_worker_responded(mut state: PoolState, uuid: Uuid) -> (PoolState, Vec<Effect>) {
    let Some(task_id) = state.busy_workers.remove(&uuid) else {
        return (state, vec![]);  // Already removed by timeout
    };
    (state, vec![Effect::TaskCompleted { uuid, task_id }])
}

fn handle_assign_heartbeat_if_idle(mut state: PoolState, uuid: Uuid) -> (PoolState, Vec<Effect>) {
    let Some(pos) = state.waiting_workers.iter().position(|w| w == &uuid) else {
        return (state, vec![]);  // Not waiting anymore
    };
    state.waiting_workers.remove(pos);

    let task_id = TaskId::Heartbeat(HeartbeatId(/* allocate */));
    state.busy_workers.insert(uuid.clone(), task_id);
    (state, vec![Effect::TaskAssigned { uuid, task_id }])
}

fn handle_worker_timed_out(mut state: PoolState, uuid: Uuid) -> (PoolState, Vec<Effect>) {
    let Some(task_id) = state.busy_workers.remove(&uuid) else {
        return (state, vec![]);  // Already responded
    };
    (state, vec![Effect::TaskFailed { task_id }, Effect::WorkerRemoved { uuid }])
}
```

No epochs. Status is implicit in which collection the worker is in.

**Key simplification:** `TaskCompleted` implies worker removal. It's a matching service - task and worker are paired, and when the match completes, both are removed.

#### Idle timeout explained

When a worker registers but no task is available:
1. Core emits `WorkerWaiting { uuid }` effect
2. IO layer starts an idle timeout timer for this UUID
3. If timer fires, IO sends `AssignHeartbeatIfIdle { uuid }`
4. Core checks if UUID is still in `waiting_workers` - if so, assigns heartbeat
5. Worker responds, gets removed, re-registers with new UUID

This keeps workers engaged. There's no "idle → removed" path - idle workers get heartbeats.

---

### Task 4: Update IO Layer

**File:** `crates/agent_pool/src/daemon/io.rs`

The IO layer maps abstract IDs to concrete file paths and performs actual I/O. It maintains a per-UUID state machine that handles duplicate/delayed FS events gracefully.

#### UUID State Machine (Monotonic Transitions)

For each UUID, the IO layer tracks state and only transitions **upward**. This provides the guarantees that let core panic on violations.

```rust
// =============================================================================
// UUID State Tracking
// =============================================================================

/// State of a UUID in the filesystem protocol.
/// States are ordered - we only ever transition to higher states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(super) enum UuidState {
    #[default]
    Unknown = 0,    // Haven't seen this UUID yet
    Ready = 1,      // ready.json exists (worker registered)
    Assigned = 2,   // task.json written (by daemon)
    Responded = 3,  // response.json exists (worker responded)
    Removed = 4,    // Worker removed, files cleaned up
}

/// FS events we might receive for a UUID.
/// Multiple events can map to the same state (e.g., Create + Modify + Close for one write).
#[derive(Debug, Clone, Copy)]
pub(super) enum FsEventKind {
    ReadyCreated,
    ReadyDeleted,
    TaskCreated,
    TaskDeleted,
    ResponseCreated,
    ResponseDeleted,
}

impl FsEventKind {
    /// What state does this event indicate we should be in (at minimum)?
    fn target_state(self) -> UuidState {
        match self {
            Self::ReadyCreated => UuidState::Ready,
            Self::ReadyDeleted => UuidState::Assigned,  // Deleted after task.json written
            Self::TaskCreated => UuidState::Assigned,   // Primary trigger for Assigned
            Self::TaskDeleted => UuidState::Responded,  // Deleted after response read
            Self::ResponseCreated => UuidState::Responded,  // Primary trigger for Responded
            Self::ResponseDeleted => UuidState::Removed,
        }
    }
}

/// Tracker for a single UUID's state.
#[derive(Default)]
pub(super) struct UuidTracker {
    pub state: UuidState,
}
```

#### Handling FS Events (Monotonic Transitions)

```rust
impl IoState {
    /// Handle a filesystem event for a UUID.
    /// Only transitions to higher states. Duplicate/delayed events are ignored.
    fn handle_fs_event(&mut self, uuid: &str, event_kind: FsEventKind) -> Option<Event> {
        let target_state = event_kind.target_state();
        let tracker = self.uuid_trackers.entry(uuid.to_string()).or_default();

        if target_state <= tracker.state {
            return None;  // Duplicate or delayed event
        }

        let old_state = tracker.state;
        tracker.state = target_state;

        match (old_state, target_state) {
            (UuidState::Unknown, UuidState::Ready) => {
                Some(Event::WorkerReady { uuid: uuid.to_string() })
            }
            (UuidState::Assigned, UuidState::Responded) => {
                Some(Event::WorkerResponded { uuid: uuid.to_string() })
            }
            _ => None,  // Other transitions are initiated by us
        }
    }
}
```

**Key properties:**
1. **Monotonic**: State only increases, never decreases
2. **Idempotent**: Duplicate events for same state are ignored
3. **Tolerant**: Delayed events from lower states are ignored
4. **Events to core only on transitions**: WorkerReady sent exactly once (Unknown→Ready), WorkerResponded sent exactly once (Assigned→Responded)

This is what enables core to panic on violations - IO guarantees it will never send duplicate events or events for invalid states.

#### File Ownership (RAII Guards)

```rust
// =============================================================================
// Worker Metadata
// =============================================================================

/// Data parsed from ready.json. Debug-only, not used for dispatch.
#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct WorkerData {
    /// Optional name for debugging/logging
    pub name: Option<String>,
}

// =============================================================================
// Typestate for File Management (RAII Guards)
// =============================================================================

/// Worker just registered - owns ready file.
/// Drop deletes the ready file.
pub(super) struct WorkerReady {
    pub uuid: String,
    pub ready_path: PathBuf,
    pub data: WorkerData,
}

/// Worker has task assigned - owns task file.
/// Drop deletes the task file.
pub(super) struct WorkerAssigned {
    pub uuid: String,
    pub task_path: PathBuf,
}

impl WorkerReady {
    /// Create from a ready file path. Parses metadata from file.
    fn from_path(ready_path: PathBuf) -> io::Result<Self> {
        let uuid = ready_path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(READY_SUFFIX))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid ready path"))?
            .to_string();

        let content = fs::read_to_string(&ready_path).unwrap_or_default();
        let data: WorkerData = serde_json::from_str(&content).unwrap_or_default();

        Ok(Self { uuid, ready_path, data })
    }

    /// Assign a task. Deletes ready file, writes task file, returns new guard.
    fn assign_task(mut self, agents_dir: &Path, content: &str) -> io::Result<WorkerAssigned> {
        let task_path = agents_dir.join(format!("{}{TASK_SUFFIX}", self.uuid));
        fs::write(&task_path, content)?;

        // Take uuid before dropping self (can't partially move from Drop type)
        let uuid = std::mem::take(&mut self.uuid);
        // self drops here → ready file deleted (with empty uuid, but that's fine)

        Ok(WorkerAssigned { uuid, task_path })
    }
}

impl WorkerAssigned {
    /// Get the response file path for this worker.
    fn response_path(&self, agents_dir: &Path) -> PathBuf {
        agents_dir.join(format!("{}{WORKER_RESPONSE_SUFFIX}", self.uuid))
    }
}

impl Drop for WorkerReady {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.ready_path);
    }
}

impl Drop for WorkerAssigned {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.task_path);
    }
}

// =============================================================================
// Worker State (IO layer)
// =============================================================================

/// Current state of a worker in IO layer.
pub(super) enum IoWorkerState {
    Ready(WorkerReady),
    Assigned(WorkerAssigned),
}

/// IO layer just maps UUID → IoWorkerState. No separate ID needed.
pub(super) type WorkerMap = HashMap<Uuid, IoWorkerState>;
```

**Transitions:**
- `WorkerReady::assign_task(self, content)` → consumes self (deletes ready file), writes task file, returns `WorkerAssigned`
- `WorkerAssigned` dropped on completion → deletes task file, we manually delete response file

#### Effect handlers (IO layer)

```rust
fn handle_task_assigned(uuid: &Uuid, task_id: TaskId, worker_map: &mut WorkerMap) {
    let state = worker_map.remove(uuid).expect("TaskAssigned for unknown worker");
    let IoWorkerState::Ready(ready) = state else {
        panic!("TaskAssigned but worker not in Ready state");
    };

    let task_content = /* get task content based on task_id */;
    let assigned = ready.assign_task(&agents_dir, &task_content).expect("write task");
    worker_map.insert(uuid.clone(), IoWorkerState::Assigned(assigned));
}

fn handle_task_completed(uuid: &Uuid, worker_map: &mut WorkerMap) {
    let state = worker_map.remove(uuid).expect("TaskCompleted for unknown worker");
    // state drops → files cleaned up by RAII
}

fn handle_worker_removed(uuid: &Uuid, worker_map: &mut WorkerMap) {
    // Write kicked message, then remove
    let task_path = agents_dir.join(format!("{uuid}{TASK_SUFFIX}"));
    let _ = fs::write(&task_path, r#"{"kind":"Kicked"}"#);
    worker_map.remove(uuid);  // state drops → files cleaned up
}
```

#### Helper for paths

```rust
fn task_path(agents_dir: &Path, uuid: &str) -> PathBuf {
    agents_dir.join(format!("{uuid}{TASK_SUFFIX}"))
}

fn response_path(agents_dir: &Path, uuid: &str) -> PathBuf {
    agents_dir.join(format!("{uuid}{WORKER_RESPONSE_SUFFIX}"))
}

fn ready_path(agents_dir: &Path, uuid: &str) -> PathBuf {
    agents_dir.join(format!("{uuid}{READY_SUFFIX}"))
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(READY_SUFFIX))
}
```

---

### Task 5: Simplify worker.rs

**File:** `crates/agent_pool/src/worker.rs`

```rust
//! Task execution utilities for workers.

use crate::constants::{AGENTS_DIR, READY_SUFFIX, TASK_SUFFIX};
use crate::fs::VerifiedWatcher;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

/// Wait for a task assignment.
///
/// 1. Generates a UUID
/// 2. Writes `<uuid>.ready.json` to signal availability
/// 3. Waits for `<uuid>.task.json` using VerifiedWatcher
/// 4. Returns the UUID and task content
///
/// # Errors
///
/// Returns an error if file operations fail or timeout is exceeded.
pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    timeout: Option<Duration>,
) -> io::Result<(String, String)> {
    let agents_dir = pool_root.join(AGENTS_DIR);
    let uuid = Uuid::new_v4().to_string();

    let ready_path = agents_dir.join(format!("{uuid}{READY_SUFFIX}"));
    let task_path = agents_dir.join(format!("{uuid}{TASK_SUFFIX}"));
    let canary_path = agents_dir.join(format!("{uuid}.canary"));

    // Write ready file with optional metadata
    let metadata = match name {
        Some(n) => format!(r#"{{"name":"{}"}}"#, n),
        None => "{}".to_string(),
    };
    fs::write(&ready_path, &metadata)?;

    // Wait for task using VerifiedWatcher
    let mut watcher = VerifiedWatcher::new(&agents_dir, canary_path)?;
    watcher.wait_for(&task_path, timeout)?;

    let task = fs::read_to_string(&task_path)?;
    Ok((uuid, task))
}

/// Write a response for the given task.
pub fn write_response(pool_root: &Path, uuid: &str, response: &str) -> io::Result<()> {
    use crate::constants::WORKER_RESPONSE_SUFFIX;
    let agents_dir = pool_root.join(AGENTS_DIR);
    let response_path = agents_dir.join(format!("{uuid}{WORKER_RESPONSE_SUFFIX}"));
    fs::write(&response_path, response)
}
```

**Removed:**
- `AgentEvent` enum
- `create_watcher()`
- `verify_watcher_sync()`
- `is_task_ready()` (no longer needed - fresh UUID each cycle)
- `wait_for_task_with_timeout()` (merged into `wait_for_task`)
- Platform-specific `is_file_write_event()` (moved to `fs.rs`)

---

### Task 7: Update CLI Commands

**File:** `crates/agent_pool/src/bin/agent_pool.rs`

Consolidate `register`, `get_task`, `next_task` into single workflow using new executor functions.

---

### Task 8: Update Protocol Documentation

**File:** `crates/agent_pool/protocols/AGENT_PROTOCOL.md`

Document the new three-file protocol.

---

## Testing Considerations

1. **Unit tests for PathCategory** - Verify flat file patterns are recognized
2. **Integration tests** - Verify worker registration flow works end-to-end
3. **Race condition testing** - Verify no inotify race on Linux
4. **Timeout handling** - Verify stale ready files are cleaned up

## Migration

No migration needed - this is a breaking change to the protocol. All existing agents will need to update to the new protocol.

## Open Questions

1. **Should ready.json contain metadata?** - Currently proposed to include optional `name` field for debugging. Could also include capabilities, version, etc.
