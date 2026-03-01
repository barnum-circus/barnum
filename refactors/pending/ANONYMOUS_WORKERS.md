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

### Current worker.rs (324 lines)

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
3. Daemon assigns task, writes `agents/<uuid>.task.json`
4. Worker reads task, processes, writes `agents/<uuid>.response.json`
5. Daemon sees `FileWritten` event → `PathCategory::WorkerResponse`
6. Daemon cleans up all three files
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

This replaces ~300 lines with ~30 lines.

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

#### 3.1: Rename AgentId to WorkerId

```rust
// Before
pub(super) struct AgentId(pub(super) u32);
pub(super) enum AgentStatus { Idle, Busy { task_id: TaskId } }
pub(super) struct AgentState { ... }

// After
pub(super) struct WorkerId(pub(super) u32);
pub(super) enum WorkerStatus { Idle, Busy { task_id: TaskId } }
pub(super) struct WorkerState { ... }
```

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
    WorkerReady { worker_id: WorkerId, heartbeat_task_id: Option<TaskId> },
    WorkerResponded { worker_id: WorkerId },
    WorkerTimedOut { epoch: Epoch },
    AssignTaskToWorkerIfEpochMatches { epoch: Epoch, task_id: TaskId },
}
```

**Key changes:**
- Rename `AgentRegistered` → `WorkerReady`
- Rename `AgentResponded` → `WorkerResponded`
- Rename `AgentTimedOut` → `WorkerTimedOut`
- Rename `AssignTaskToAgentIfEpochMatches` → `AssignTaskToWorkerIfEpochMatches`
- Remove `AgentDeregistered` - workers don't deregister, stale files cleaned up on timeout

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
    TaskAssigned { task_id: TaskId, epoch: Epoch },
    WorkerWaiting { epoch: Epoch },
    TaskCompleted { worker_id: WorkerId, task_id: TaskId },
    TaskFailed { task_id: TaskId },
    WorkerRemoved { worker_id: WorkerId },
}
```

**Key changes:**
- Rename `AgentIdled` → `WorkerWaiting`
- Rename `AgentRemoved` → `WorkerRemoved`

**Semantic change:** In the named-agent model, `AgentIdled` was emitted in two cases:
1. Agent registers but no task available
2. Agent completes a task but no new task available (returns to idle)

In the anonymous worker model, only case 1 exists - workers don't "return to idle" because they get a fresh UUID each cycle. Hence the rename to `WorkerWaiting` which better describes the semantics: "worker is waiting for first task."

**Note:** There is no `StartTimer` effect - timers are started implicitly by the IO layer when it handles `TaskAssigned` or `WorkerWaiting`.

---

### Task 4: Update IO Layer

**File:** `crates/agent_pool/src/daemon/io.rs`

This is the most complex task. The IO layer maps abstract IDs to concrete transports and performs actual I/O. With anonymous workers, the key changes are:

1. **Type aliases** - Rename `AgentMap` → `WorkerMap`
2. **Import rename** - `AgentId` → `WorkerId` (from core.rs)
3. **Path registration** - Register flat files (`<uuid>.ready.json`) instead of directories
4. **File operations** - Write to `<uuid>.task.json` instead of `<dir>/task.json`
5. **Cleanup tracking** - Track cleaned-up UUIDs instead of directory paths

#### 4.1: Update imports and type alias

```rust
// Before
use super::core::{AgentId, Effect, Epoch, Event, ExternalTaskId, HeartbeatId, TaskId};

pub(super) type AgentMap = TransportMap<AgentId>;

// After
use super::core::{WorkerId, Effect, Epoch, Event, ExternalTaskId, HeartbeatId, TaskId};

pub(super) type WorkerMap = TransportMap<WorkerId>;
```

#### 4.2: Update TransportId impl

```rust
// Before
impl TransportId for AgentId {
    type Data = ();
}

// After
impl TransportId for WorkerId {
    /// Worker metadata from ready.json (e.g., debug name)
    type Data = WorkerData;
}

/// Data stored per worker, parsed from ready.json.
#[derive(Debug, Clone, Default)]
pub(super) struct WorkerData {
    /// Debug-only name from ready.json (optional)
    pub name: Option<String>,
}
```

#### 4.3: Change path registration pattern

The current `AgentMap` registers **directories** (`agents/claude-1/`). The new `WorkerMap` registers **ready files** (`agents/<uuid>.ready.json`).

```rust
// Before (in wiring.rs, register_agent)
fn register_agent(
    name: &str,
    agents_dir: &Path,
    agent_map: &mut AgentMap,
    // ...
) -> Option<AgentId> {
    let agent_path = agents_dir.join(name);  // Directory path
    agent_map.register_directory(agent_path, ())
}

// After (in wiring.rs, handle_worker_ready)
fn handle_worker_ready(
    uuid: &str,
    agents_dir: &Path,
    worker_map: &mut WorkerMap,
    // ...
) -> Option<WorkerId> {
    let ready_path = agents_dir.join(format!("{uuid}.ready.json"));

    // Parse metadata from ready.json
    let metadata = fs::read_to_string(&ready_path).unwrap_or_default();
    let data: WorkerData = serde_json::from_str(&metadata).unwrap_or_default();

    worker_map.register_directory(ready_path, data)
}
```

#### 4.4: Change file write pattern in execute_effect

Currently, `execute_effect` writes to files **inside** the agent directory. With flat files, we write **sibling files** with the same UUID prefix.

```rust
// Before (Effect::TaskAssigned)
Effect::TaskAssigned { task_id, epoch } => {
    match task_id {
        TaskId::External(external_id) => {
            let task_data = external_task_map
                .get_data(external_id)
                .expect("TaskAssigned for unknown task - core bug");

            // Write to agent's directory
            agent_map
                .write_to(epoch.agent_id, TASK_FILE, &task_data.content)
                //       ^^^^^^^^^^^^^^^^  ^^^^^^^^^
                //       agent directory   "task.json"
                .expect("TaskAssigned for unknown agent - core bug");
            // ...
        }
        // ...
    }
}

// After (Effect::TaskAssigned)
Effect::TaskAssigned { task_id, epoch } => {
    match task_id {
        TaskId::External(external_id) => {
            let task_data = external_task_map
                .get_data(external_id)
                .expect("TaskAssigned for unknown task - core bug");

            // Get UUID from ready file path, write task file as sibling
            let ready_path = worker_map
                .get_path(epoch.worker_id)
                .expect("TaskAssigned for unknown worker - core bug");
            let task_path = ready_path.with_file_name(
                ready_path.file_stem()  // "<uuid>.ready" -> "<uuid>"
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.strip_suffix(".ready"))
                    .map(|uuid| format!("{uuid}.task.json"))
                    .expect("ready path should have .ready.json suffix")
            );
            fs::write(&task_path, &task_data.content)?;
            // ...
        }
        // ...
    }
}
```

#### 4.5: Add helper for UUID extraction

To avoid repeating the UUID extraction logic, add a helper:

```rust
/// Extract UUID from a worker's ready file path.
///
/// Given `agents/<uuid>.ready.json`, returns `<uuid>`.
fn extract_uuid(ready_path: &Path) -> Option<&str> {
    ready_path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(READY_SUFFIX))
}

/// Get the task file path for a worker.
fn task_path_for_worker(ready_path: &Path) -> PathBuf {
    let uuid = extract_uuid(ready_path).expect("ready path should have READY_SUFFIX");
    ready_path.with_file_name(format!("{uuid}{TASK_SUFFIX}"))
}

/// Get the response file path for a worker.
fn response_path_for_worker(ready_path: &Path) -> PathBuf {
    let uuid = extract_uuid(ready_path).expect("ready path should have READY_SUFFIX");
    ready_path.with_file_name(format!("{uuid}{WORKER_RESPONSE_SUFFIX}"))
}
```

#### 4.6: Update Effect::TaskCompleted

```rust
// Before
Effect::TaskCompleted { agent_id, task_id } => {
    let agent_path = agent_map
        .get_path(agent_id)
        .expect("TaskCompleted for unknown agent - core bug");

    match task_id {
        TaskId::Heartbeat(_) => {
            let _ = fs::remove_file(agent_path.join(TASK_FILE));
            let _ = fs::remove_file(agent_path.join(RESPONSE_FILE));
        }
        TaskId::External(external_id) => {
            let agent_output = agent_map
                .read_from(agent_id, RESPONSE_FILE)
                .expect("TaskCompleted for unknown agent - core bug");

            let _ = fs::remove_file(agent_path.join(TASK_FILE));
            let _ = fs::remove_file(agent_path.join(RESPONSE_FILE));
            // ...
        }
    }
}

// After
Effect::TaskCompleted { worker_id, task_id } => {
    let ready_path = worker_map
        .get_path(worker_id)
        .expect("TaskCompleted for unknown worker - core bug");
    let task_path = task_path_for_worker(ready_path);
    let response_path = response_path_for_worker(ready_path);

    match task_id {
        TaskId::Heartbeat(_) => {
            // Clean up all three files for this worker
            let _ = fs::remove_file(ready_path);
            let _ = fs::remove_file(&task_path);
            let _ = fs::remove_file(&response_path);
        }
        TaskId::External(external_id) => {
            let agent_output = fs::read_to_string(&response_path)
                .expect("TaskCompleted but response file missing - logic bug");

            // Clean up all three files for this worker
            let _ = fs::remove_file(ready_path);
            let _ = fs::remove_file(&task_path);
            let _ = fs::remove_file(&response_path);
            // ...
        }
    }
}
```

#### 4.7: Update Effect::AgentRemoved → Effect::WorkerRemoved

```rust
// Before
Effect::AgentRemoved { agent_id } => {
    let (transport, ()) = agent_map
        .remove(agent_id)
        .expect("AgentRemoved for unknown agent - core bug");

    // Write kicked message so agent knows it was removed
    let kicked_msg = serde_json::json!({
        "kind": "Kicked",
        "reason": "Timeout"
    });
    let _ = transport.write(TASK_FILE, &kicked_msg.to_string());

    // Track this path so we reject re-registration attempts
    if let Some(agent_path) = transport.path() {
        kicked_paths.insert(agent_path.to_path_buf());
    }
}

// After
Effect::WorkerRemoved { worker_id } => {
    let (transport, _data) = worker_map
        .remove(worker_id)
        .expect("WorkerRemoved for unknown worker - core bug");

    // Write kicked message to task file
    if let Some(ready_path) = transport.path() {
        let task_path = task_path_for_worker(ready_path);
        let kicked_msg = serde_json::json!({
            "kind": "Kicked",
            "reason": "Timeout"
        });
        let _ = fs::write(&task_path, kicked_msg.to_string());

        // Track this UUID so we reject stale events
        if let Some(uuid) = extract_uuid(ready_path) {
            removed_workers.insert(uuid.to_string());
        }
    }
}
```

#### 4.8: Update kicked_paths to removed_workers

```rust
// Before (in execute_effect signature)
pub(super) fn execute_effect(
    effect: Effect,
    agent_map: &mut AgentMap,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    kicked_paths: &mut HashSet<PathBuf>,  // Directory paths
    // ...
)

// After
pub(super) fn execute_effect(
    effect: Effect,
    worker_map: &mut WorkerMap,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    removed_workers: &mut HashSet<String>,  // UUIDs
    // ...
)
```

#### 4.9: Update IoConfig field name

```rust
// Before
pub(super) struct IoConfig {
    pub idle_agent_timeout: Duration,
    // ...
}

// After
pub(super) struct IoConfig {
    pub idle_worker_timeout: Duration,
    // ...
}
```

#### 4.10: Update tests

```rust
// Before
#[test]
fn agent_map_register_and_lookup() {
    let mut map = AgentMap::new();
    let path = PathBuf::from("/tmp/test/agents/agent-1");  // Directory

    let id = map.register_directory(path.clone(), ()).unwrap();
    assert_eq!(id, AgentId(0));
    // ...
}

// After
#[test]
fn worker_map_register_and_lookup() {
    let mut map = WorkerMap::new();
    let path = PathBuf::from("/tmp/test/agents/abc123.ready.json");  // Flat file

    let data = WorkerData { name: Some("test-worker".to_string()) };
    let id = map.register_directory(path.clone(), data).unwrap();
    assert_eq!(id, WorkerId(0));
    // ...
}
```

---

### Task 5: Update Wiring

**File:** `crates/agent_pool/src/daemon/wiring.rs`

#### 5.1: Update event handlers

```rust
// Before
match category {
    PathCategory::AgentDir { name } => { ... }
    PathCategory::AgentResponse { name } => { ... }
    PathCategory::SubmissionRequest { id } => { ... }
}

// After
match category {
    PathCategory::WorkerReady { id } => {
        handle_worker_ready(&id, agents_dir, ...);
    }
    PathCategory::WorkerResponse { id } => {
        handle_worker_response(&id, agents_dir, ...);
    }
    PathCategory::SubmissionRequest { id } => {
        register_submission(&id, ...);
    }
}
```

---

### Task 6: Simplify worker.rs

**File:** `crates/agent_pool/src/worker.rs`

Replace the current ~324 lines with a simple wrapper around `VerifiedWatcher`:

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

## Typestate Pattern for Worker Lifecycle

Use RAII guards that automatically clean up files when transitioning between states. Each state owns the files relevant to that state, and dropping the state deletes those files.

### Worker States

```rust
/// Worker just registered - holds ready file.
/// Dropping deletes the ready file.
struct WorkerReady {
    uuid: String,
    ready_path: PathBuf,
    data: WorkerData,
}

/// Worker has task assigned - ready file already cleaned.
/// Dropping deletes the task file.
struct WorkerAssigned {
    uuid: String,
    task_path: PathBuf,
}

/// Worker completed task - task file already cleaned.
/// Dropping deletes the response file.
struct WorkerComplete {
    uuid: String,
    response_path: PathBuf,
    response_content: String,
}
```

### State Transitions

```rust
impl WorkerReady {
    /// Assign a task to this worker.
    /// Consumes self (drops ready file), writes task file.
    fn assign_task(self, task_content: &str) -> io::Result<WorkerAssigned> {
        let task_path = self.ready_path.with_file_name(
            format!("{}{TASK_SUFFIX}", self.uuid)
        );
        fs::write(&task_path, task_content)?;

        // self drops here -> ready file deleted
        Ok(WorkerAssigned {
            uuid: self.uuid,
            task_path,
        })
    }
}

impl WorkerAssigned {
    /// Mark task as complete after reading response.
    /// Consumes self (drops task file), reads response file.
    fn complete(self, agents_dir: &Path) -> io::Result<WorkerComplete> {
        let response_path = agents_dir.join(
            format!("{}{WORKER_RESPONSE_SUFFIX}", self.uuid)
        );
        let response_content = fs::read_to_string(&response_path)?;

        // self drops here -> task file deleted
        Ok(WorkerComplete {
            uuid: self.uuid,
            response_path,
            response_content,
        })
    }
}

impl WorkerComplete {
    /// Finish processing and clean up.
    /// Returns the response content, drops self (deletes response file).
    fn finish(self) -> String {
        // self drops here -> response file deleted
        self.response_content
    }
}
```

### Drop Implementations

```rust
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

impl Drop for WorkerComplete {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.response_path);
    }
}
```

### Benefits

1. **Automatic cleanup** - Files are deleted when states transition, no manual cleanup needed
2. **Compiler-enforced correctness** - Can't forget to clean up, can't access wrong state's data
3. **Clear ownership** - Each state owns exactly the files it's responsible for
4. **Panic safety** - Even if code panics, Drop runs and cleans up files

### Worker State Enum for Storage

The IO layer stores workers in a map. Use an enum to hold the current state:

```rust
enum WorkerState {
    Ready(WorkerReady),
    Assigned(WorkerAssigned),
    // Complete is transient - processed immediately then dropped
}

type WorkerMap = HashMap<String, WorkerState>;  // Keyed by UUID
```

---

## Open Questions

1. **Should ready.json contain metadata?** - Currently proposed to include optional `name` field for debugging. Could also include capabilities, version, etc.

2. ~~**Cleanup timing**~~ - Resolved by typestate pattern. Files are cleaned up automatically on state transitions.
