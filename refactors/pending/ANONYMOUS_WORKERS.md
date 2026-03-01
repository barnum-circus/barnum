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

#### 3.1: Core Data Structures

```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
struct WorkerId(u32);

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
struct SubmissionId(u32);

enum TaskId {
    External(SubmissionId),
    Heartbeat,
}

/// Either tasks are waiting for workers, or workers are waiting for tasks, or neither.
/// Never both - any arrival triggers immediate matching.
///
/// Invariant: VecDeque is always non-empty in Tasks/Workers variants.
/// When the last item is removed, transition to None.
enum Waiting {
    None,
    Tasks(VecDeque<SubmissionId>),
    Workers(VecDeque<WorkerId>),
}

struct PoolState {
    waiting: Waiting,
    busy_workers: HashMap<WorkerId, TaskId>,
}
```

The `Waiting` enum makes the invariant impossible to violate at the type level.

#### 3.2: Events and Effects

```rust
enum Event {
    TaskSubmitted { submission_id: SubmissionId },
    TaskWithdrawn { submission_id: SubmissionId },
    WorkerReady { worker_id: WorkerId },
    WorkerResponded { worker_id: WorkerId },
    WorkerTimedOut { worker_id: WorkerId },
    AssignHeartbeatIfIdle { worker_id: WorkerId },
}

enum Effect {
    TaskAssigned { worker_id: WorkerId, task_id: TaskId },
    WorkerWaiting { worker_id: WorkerId },
    TaskCompleted { worker_id: WorkerId, task_id: TaskId },
    TaskFailed { submission_id: SubmissionId },
    WorkerRemoved { worker_id: WorkerId },
}
```

All u32 IDs. IO layer handles UUID ↔ ID mapping.

#### 3.4: Event Handlers

**Event sources and race analysis:**

| Source | Events | Can race with |
|--------|--------|---------------|
| FS (watcher) | WorkerReady, WorkerResponded | Timers only |
| Timers | WorkerTimedOut, AssignHeartbeatIfIdle | Everything |
| Submissions | TaskSubmitted, TaskWithdrawn | TaskWithdrawn (same submission) |

**FS events are sequenced** by the IO layer. **Timer events need defensive handling** (worker might be gone or in different state).

```rust
fn handle_worker_ready(state: &mut PoolState, worker_id: WorkerId) -> Vec<Effect> {
    // PANIC: IO layer guarantees WorkerReady is sent exactly once per worker.
    // A duplicate would be a bug in IO's UUID→ID mapping.
    assert!(!state.busy_workers.contains_key(&worker_id));

    match &mut state.waiting {
        Waiting::Tasks(submission_ids) => {
            let submission_id = submission_ids.pop_front().expect("Tasks variant with empty queue");
            if submission_ids.is_empty() {
                state.waiting = Waiting::None;
            }
            state.busy_workers.insert(worker_id, TaskId::External(submission_id));
            vec![Effect::TaskAssigned { worker_id, task_id: TaskId::External(submission_id) }]
        }
        Waiting::Workers(worker_ids) => {
            // PANIC: Same worker appearing twice in waiting queue is a bug.
            assert!(!worker_ids.contains(&worker_id));
            worker_ids.push_back(worker_id);
            vec![Effect::WorkerWaiting { worker_id }]
        }
        Waiting::None => {
            state.waiting = Waiting::Workers(VecDeque::from([worker_id]));
            vec![Effect::WorkerWaiting { worker_id }]
        }
    }
}

fn handle_task_submitted(state: &mut PoolState, submission_id: SubmissionId) -> Vec<Effect> {
    // No defensive checks needed - submissions are independent of each other
    // and IO allocates fresh IDs.
    match &mut state.waiting {
        Waiting::Workers(worker_ids) => {
            let worker_id = worker_ids.pop_front().expect("Workers variant with empty queue");
            if worker_ids.is_empty() {
                state.waiting = Waiting::None;
            }
            state.busy_workers.insert(worker_id, TaskId::External(submission_id));
            vec![Effect::TaskAssigned { worker_id, task_id: TaskId::External(submission_id) }]
        }
        Waiting::Tasks(submission_ids) => {
            submission_ids.push_back(submission_id);
            vec![]
        }
        Waiting::None => {
            state.waiting = Waiting::Tasks(VecDeque::from([submission_id]));
            vec![]
        }
    }
}

fn handle_worker_responded(state: &mut PoolState, worker_id: WorkerId) -> Vec<Effect> {
    // DEFENSIVE: Worker might not be in busy_workers if:
    // - Timeout fired first and already removed the worker
    // IO guarantees we only get WorkerResponded for workers that were Assigned,
    // but timers can race and remove the worker before we process the response.
    let Some(task_id) = state.busy_workers.remove(&worker_id) else {
        return vec![];
    };
    vec![Effect::TaskCompleted { worker_id, task_id }]
}

fn handle_heartbeat_if_idle(state: &mut PoolState, worker_id: WorkerId) -> Vec<Effect> {
    // DEFENSIVE: Timer event - worker state may have changed since timer was scheduled.
    // Worker might be:
    // - Gone (timed out, or completed a task and re-registered with new ID)
    // - Busy (got a real task before heartbeat timer fired)
    let Waiting::Workers(worker_ids) = &mut state.waiting else {
        return vec![]; // No idle workers at all
    };
    let Some(pos) = worker_ids.iter().position(|id| *id == worker_id) else {
        return vec![]; // This specific worker not idle (busy or gone)
    };
    worker_ids.remove(pos);
    if worker_ids.is_empty() {
        state.waiting = Waiting::None;
    }
    state.busy_workers.insert(worker_id, TaskId::Heartbeat);
    vec![Effect::TaskAssigned { worker_id, task_id: TaskId::Heartbeat }]
}

fn handle_worker_timeout(state: &mut PoolState, worker_id: WorkerId) -> Vec<Effect> {
    // DEFENSIVE: Timer event - worker might have already responded.
    // WorkerResponded could have been processed first, removing the worker.
    let Some(task_id) = state.busy_workers.remove(&worker_id) else {
        return vec![];
    };
    let mut effects = vec![Effect::WorkerRemoved { worker_id }];
    if let TaskId::External(submission_id) = task_id {
        effects.push(Effect::TaskFailed { submission_id });
    }
    effects
}
```

No epochs. Status is implicit in which collection the worker is in.

**Key simplification:** `TaskCompleted` implies worker removal. It's a matching service - task and worker are paired, and when the match completes, both are removed.

#### Idle timeout explained

When a worker registers but no task is available:
1. Core emits `WorkerWaiting { worker }` effect
2. IO layer starts an idle timeout timer for this WorkerId
3. If timer fires, IO sends `AssignHeartbeatIfIdle { id }`
4. Core checks if worker is still in `waiting_workers` - if so, assigns heartbeat
5. Worker responds, gets removed, re-registers with new UUID (gets new WorkerId)

This keeps workers engaged. There's no "idle → removed" path - idle workers get heartbeats.

---

### Task 4: Update IO Layer

**File:** `crates/agent_pool/src/daemon/io.rs`

The IO layer maps UUIDs to IDs and handles file operations.

#### UUID Newtypes

```rust
/// Worker UUID - from ready.json filename. Not interchangeable with SubmissionUuid.
#[derive(Clone, Default, Eq, PartialEq, Hash)]
struct WorkerUuid(String);

/// Submission UUID - from request.json filename. Not interchangeable with WorkerUuid.
#[derive(Clone, Default, Eq, PartialEq, Hash)]
struct SubmissionUuid(String);
```

#### UUID → ID Mapping

```rust
struct IoState {
    // UUID → ID mappings (IO allocates IDs, core uses them)
    worker_uuids: HashMap<WorkerUuid, WorkerId>,
    submission_uuids: HashMap<SubmissionUuid, SubmissionId>,
    next_worker_id: u32,
    next_submission_id: u32,

    // File state (contains UUID for reverse lookup)
    workers: HashMap<WorkerId, IoWorkerState>,
}
```

No reverse map needed - when handling Effects, we look up `workers[worker_id]` and the `IoWorkerState` contains the UUID.

#### Handling FS Events

```rust
impl IoState {
    fn handle_ready_created(&mut self, worker_uuid: WorkerUuid) -> Option<Event> {
        if self.worker_uuids.contains_key(&worker_uuid) {
            return None;  // Duplicate
        }
        let worker_id = WorkerId(self.next_worker_id);
        self.next_worker_id += 1;
        self.worker_uuids.insert(worker_uuid.clone(), worker_id);

        let ready = WorkerReady::from_path(...)?;
        self.workers.insert(worker_id, IoWorkerState::Ready(ready));
        Some(Event::WorkerReady { worker_id })
    }

    fn handle_response_created(&mut self, worker_uuid: WorkerUuid) -> Option<Event> {
        let worker_id = self.worker_uuids.get(&worker_uuid)?;
        match self.workers.get(worker_id) {
            Some(IoWorkerState::Assigned(_)) => Some(Event::WorkerResponded { worker_id: *worker_id }),
            _ => None,
        }
    }
}
```

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
    pub worker_uuid: WorkerUuid,
    pub ready_path: PathBuf,
    pub data: WorkerData,
}

/// Worker has task assigned - owns task file.
/// Drop deletes the task file.
pub(super) struct WorkerAssigned {
    pub worker_uuid: WorkerUuid,
    pub task_path: PathBuf,
}

impl WorkerReady {
    /// Create from a ready file path. Parses metadata from file.
    fn from_path(ready_path: PathBuf) -> io::Result<Self> {
        let uuid_str = ready_path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(READY_SUFFIX))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid ready path"))?;
        let worker_uuid = WorkerUuid(uuid_str.to_string());

        let content = fs::read_to_string(&ready_path).unwrap_or_default();
        let data: WorkerData = serde_json::from_str(&content).unwrap_or_default();

        Ok(Self { worker_uuid, ready_path, data })
    }

    /// Assign a task. Deletes ready file, writes task file, returns new guard.
    fn assign_task(mut self, agents_dir: &Path, content: &str) -> io::Result<WorkerAssigned> {
        let task_path = agents_dir.join(format!("{}{TASK_SUFFIX}", self.worker_uuid.0));
        fs::write(&task_path, content)?;

        // Take worker_uuid before dropping self (can't partially move from Drop type)
        let worker_uuid = std::mem::take(&mut self.worker_uuid);
        // self drops here → ready file deleted

        Ok(WorkerAssigned { worker_uuid, task_path })
    }
}

impl WorkerAssigned {
    /// Get the response file path for this worker.
    fn response_path(&self, agents_dir: &Path) -> PathBuf {
        agents_dir.join(format!("{}{WORKER_RESPONSE_SUFFIX}", self.worker_uuid.0))
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

---

## Implementation Order (Temporal)

The tasks above are organized for logical readability, not implementation order. Here's the actual sequence with commit points marked.

### Phase 1: Independent Preparatory Work (each step is committable)

**Step 1: Task 0 - Consolidate `is_write_complete`**
- Move platform-specific code to `fs.rs`
- Update both callers to use shared function
- ✅ COMMIT: Tests pass, no behavior change

**Step 2: Task 2 - Add new constants**
- Add `READY_SUFFIX`, `TASK_SUFFIX`, `WORKER_RESPONSE_SUFFIX`
- Keep old constants for now (still used)
- ✅ COMMIT: Tests pass, constants are unused

### Phase 2: The Big Bang (single atomic change)

**Step 3: Task 1 + Task 3 + Task 4**

This is the irreducible core - these changes are interdependent and must happen together:

1. **Task 1**: Change `PathCategory` variants (`AgentDir`/`AgentResponse` → `WorkerReady`/`WorkerResponse`)
2. **Task 3**: Rewrite core with new types (`WorkerId`, `SubmissionId`, `Waiting` enum, new Event/Effect)
3. **Task 4**: Update IO layer to:
   - Handle new `PathCategory` variants
   - Implement UUID ↔ ID mapping
   - Wire up to new core Event/Effect types
   - Add RAII guards for file cleanup

⚠️ BROKEN STATE: These three must change together. The daemon won't compile until all three are done.

- ❌ Tests fail during this phase
- ✅ COMMIT: When daemon compiles and basic flow works

### Phase 3: Worker Side (committable after Phase 2)

**Step 4: Task 5 - Simplify worker.rs**
- Replace old registration flow with UUID-based flat files
- Remove `AgentEvent`, `create_watcher`, etc.
- ✅ COMMIT: Tests pass, workers use new protocol

**Step 5: Task 7 - Update CLI commands**
- Consolidate `register`/`get_task`/`next_task`
- ✅ COMMIT: Tests pass, CLI works

### Phase 4: Cleanup

**Step 6: Remove dead code**
- Delete old constants (`TASK_FILE`, `RESPONSE_FILE`)
- Delete `is_folder_created`, `is_folder_removed`
- ✅ COMMIT: Tests pass, code is cleaner

**Step 7: Task 8 - Update documentation**
- Update `AGENT_PROTOCOL.md` with new three-file protocol
- ✅ COMMIT: Docs match code

### Summary

| Phase | Steps | Duration | State |
|-------|-------|----------|-------|
| 1 | 1-2 | Incremental | ✅ Always green |
| 2 | 3 | Atomic | ❌ Red until complete |
| 3 | 4-5 | Incremental | ✅ Green after each |
| 4 | 6-7 | Incremental | ✅ Green after each |

Phase 2 is the only risky part. Everything else can be done incrementally with passing tests after each commit.
