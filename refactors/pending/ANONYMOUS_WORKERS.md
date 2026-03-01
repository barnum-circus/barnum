# Anonymous Worker Model

**Status:** Ready for implementation

## Overview

Simplify agent protocol from "named agents with persistent directories" to "anonymous workers with flat files." This eliminates the inotify race condition on Linux and simplifies the protocol.

## Goals

1. **Eliminate per-agent directories** - Use flat files like submissions already do
2. **Simplify registration** - Worker creates one file, waits for response
3. **Remove agent identity** - Workers are anonymous, names are debug-only metadata
4. **Unify patterns** - Agents and submissions use identical flat-file protocol

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
    ├── <uuid>.request.json   # Client writes request
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

### Agent Registration Flow (Before)

1. Agent creates directory: `agents/<name>/`
2. Daemon sees `FolderCreated` event → `PathCategory::AgentDir`
3. Daemon registers agent, writes `task.json` with heartbeat
4. Agent writes `response.json`
5. Daemon sees `FileWritten` event → `PathCategory::AgentResponse`

**Race condition:** On Linux, inotify may fire `FolderCreated` before the directory is visible to `readdir()`.

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
    ├── <uuid>.request.json   # Client writes request
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

---

## Implementation Plan

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
        // Agent directory - meaningful on folder creation or removal
        1 if is_folder_created(event_kind) || is_folder_removed(event_kind) => {
            Some(PathCategory::AgentDir { name })
        }
        // Agent response - only meaningful when write is complete
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
const READY_SUFFIX: &str = ".ready.json";
const RESPONSE_SUFFIX: &str = ".response.json";

fn categorize_under_agents(
    path: &Path,
    event_kind: EventKind,
    agents_dir: &Path,
) -> Option<PathCategory> {
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
    if let Some(id) = filename.strip_suffix(RESPONSE_SUFFIX) {
        return Some(PathCategory::WorkerResponse { id: id.to_string() });
    }
    None
}
```

#### 1.3: Update tests

Update all `PathCategory` tests to use flat file patterns instead of nested directories.

---

### Task 2: Update IO Layer

**File:** `crates/agent_pool/src/daemon/io.rs`

#### 2.1: Change AgentId to WorkerId

```rust
// Before
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct AgentId(pub(super) u32);

pub(super) type AgentMap = TransportMap<AgentId>;

// After
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct WorkerId(pub(super) u32);

pub(super) type WorkerMap = TransportMap<WorkerId>;
```

**Note:** `WorkerId` maps to the UUID from the file path, not an auto-incrementing ID.

#### 2.2: Track workers by UUID string

The current `TransportMap` uses auto-incrementing IDs. For the new model, we track by UUID:

```rust
// Before: TransportMap with auto-incrementing IDs
pub(super) struct TransportMap<Id: TransportId> {
    entries: HashMap<Id, (Transport, Id::Data)>,
    path_to_id: HashMap<PathBuf, Id>,
    next_id: u32,
}

// After: WorkerMap keyed by UUID string
pub(super) struct WorkerMap {
    /// Maps worker UUID to transport and state
    workers: HashMap<String, WorkerEntry>,
}

struct WorkerEntry {
    /// Path to the agents/ directory (not individual files)
    agents_dir: PathBuf,
    /// Current state
    state: WorkerState,
}

enum WorkerState {
    /// Worker has written ready file, waiting for task assignment
    Ready,
    /// Worker is processing a task
    Busy { task_id: TaskId },
}
```

**Avoiding impossible states:** The `WorkerState` enum ensures a worker can only be in valid states.

---

### Task 3: Update Core State Machine

**File:** `crates/agent_pool/src/daemon/core.rs`

#### 3.1: Rename AgentId to WorkerId

```rust
// Before
pub(super) struct AgentId(pub(super) u32);

pub(super) enum AgentStatus {
    Idle,
    Busy { task_id: TaskId },
}

// After
pub(super) struct WorkerId(pub(super) u32);

pub(super) enum WorkerStatus {
    Idle,
    Busy { task_id: TaskId },
}
```

#### 3.2: Update Events

```rust
// Before
pub(super) enum Event {
    AgentRegistered { agent_id: AgentId, heartbeat_task_id: Option<TaskId> },
    AgentResponded { agent_id: AgentId },
    AgentDeregistered { agent_id: AgentId },
    TaskSubmitted { task_id: TaskId },
    TaskTimeout { epoch: Epoch },
    IdleTimeout { epoch: Epoch },
    Shutdown,
}

// After
pub(super) enum Event {
    WorkerReady { worker_id: WorkerId, heartbeat_task_id: Option<TaskId> },
    WorkerResponded { worker_id: WorkerId },
    TaskSubmitted { task_id: TaskId },
    TaskTimeout { epoch: Epoch },
    IdleTimeout { epoch: Epoch },
    Shutdown,
}
```

**Key change:** Remove `AgentDeregistered`. Workers don't deregister - they just stop calling `get_task`. Stale ready files are cleaned up by the daemon on timeout.

#### 3.3: Update Effects

```rust
// Before
pub(super) enum Effect {
    TaskAssigned { agent_id: AgentId, task_id: TaskId },
    TaskCompleted { agent_id: AgentId, task_id: TaskId, response: String },
    AgentIdled { epoch: Epoch },
    AgentKicked { agent_id: AgentId },
    StartTimer { kind: TimerKind, epoch: Epoch, duration: Duration },
}

// After
pub(super) enum Effect {
    TaskAssigned { worker_id: WorkerId, task_id: TaskId },
    TaskCompleted { worker_id: WorkerId, task_id: TaskId, response: String },
    WorkerIdled { epoch: Epoch },
    CleanupWorker { worker_id: WorkerId },  // Replaces AgentKicked
    StartTimer { kind: TimerKind, epoch: Epoch, duration: Duration },
}
```

**Key change:** `CleanupWorker` instead of `AgentKicked`. The daemon just removes the files, doesn't need to signal the worker.

---

### Task 4: Update Wiring

**File:** `crates/agent_pool/src/daemon/wiring.rs`

#### 4.1: Update event handlers

```rust
// Before
fn handle_fs_event(...) {
    match category {
        PathCategory::AgentDir { name } => {
            let agent_path = agents_dir.join(&name);
            handle_agent_dir(&agent_path, ...);
        }
        PathCategory::AgentResponse { name } => {
            let agent_path = agents_dir.join(&name);
            handle_agent_response(&agent_path, path, ...);
        }
        PathCategory::SubmissionRequest { id } => {
            register_submission(&id, ...);
        }
    }
}

// After
fn handle_fs_event(...) {
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
}
```

#### 4.2: Implement worker handlers

```rust
fn handle_worker_ready(
    worker_uuid: &str,
    agents_dir: &Path,
    events_tx: &mpsc::Sender<Event>,
    worker_map: &mut WorkerMap,
    task_id_allocator: &mut TaskIdAllocator,
    io_config: &IoConfig,
) {
    let ready_path = agents_dir.join(format!("{worker_uuid}.ready.json"));

    // Skip if file doesn't exist (may have been cleaned up)
    if !ready_path.exists() {
        return;
    }

    // Register worker and emit event
    if let Some(worker_id) = worker_map.register(worker_uuid.to_string(), agents_dir.to_path_buf()) {
        let heartbeat_task_id = if io_config.immediate_heartbeat_enabled {
            Some(task_id_allocator.allocate_heartbeat())
        } else {
            None
        };
        let _ = events_tx.send(Event::WorkerReady {
            worker_id,
            heartbeat_task_id,
        });
    }
}

fn handle_worker_response(
    worker_uuid: &str,
    agents_dir: &Path,
    events_tx: &mpsc::Sender<Event>,
    worker_map: &mut WorkerMap,
    pending_responses: &mut HashSet<WorkerId>,
) {
    let response_path = agents_dir.join(format!("{worker_uuid}.response.json"));

    if !response_path.exists() {
        return;
    }

    if let Some(worker_id) = worker_map.get_id_by_uuid(worker_uuid) {
        if pending_responses.insert(worker_id) {
            let _ = events_tx.send(Event::WorkerResponded { worker_id });
        }
    }
}
```

---

### Task 5: Update CLI Commands

**File:** `crates/agent_pool/src/bin/agent_pool.rs`

#### 5.1: Consolidate commands

```rust
// Before: Separate register and next_task commands
Commands::Register { pool, name } => { ... }
Commands::NextTask { pool, name, response_file } => { ... }
Commands::GetTask { pool, name } => { ... }

// After: Single get_task command
Commands::GetTask { pool, name } => {
    // 1. Generate UUID
    let uuid = Uuid::new_v4().to_string();
    let agents_dir = root.join(AGENTS_DIR);

    // 2. Write ready file with optional name metadata
    let ready_path = agents_dir.join(format!("{uuid}.ready.json"));
    let ready_content = json!({ "name": name });
    fs::write(&ready_path, ready_content.to_string())?;

    // 3. Wait for task file (using VerifiedWatcher)
    let task_path = agents_dir.join(format!("{uuid}.task.json"));
    let canary_path = agents_dir.join(format!("{uuid}.canary"));
    let mut watcher = VerifiedWatcher::new(&agents_dir, canary_path)?;
    watcher.wait_for(&task_path, None)?;  // None = wait forever

    // 4. Read and output task
    let task = fs::read_to_string(&task_path)?;
    println!("{task}");
}
```

#### 5.2: Remove deprecated commands

Remove `register`, `deregister_agent`, and consolidate into single `get_task` workflow.

---

### Task 6: Update Constants

**File:** `crates/agent_pool/src/constants.rs`

```rust
// Before
pub const TASK_FILE: &str = "task.json";
pub const RESPONSE_FILE: &str = "response.json";

// After (add suffixes for flat files)
pub const READY_SUFFIX: &str = ".ready.json";
pub const TASK_SUFFIX: &str = ".task.json";
pub const WORKER_RESPONSE_SUFFIX: &str = ".response.json";

// Keep for submissions (unchanged)
pub const REQUEST_SUFFIX: &str = ".request.json";
pub const RESPONSE_SUFFIX: &str = ".response.json";
```

---

### Task 7: Update Protocol Documentation

**File:** `crates/agent_pool/AGENT_PROTOCOL.md`

Document the new three-file protocol:

1. Worker writes `<uuid>.ready.json` with optional metadata (name)
2. Daemon writes `<uuid>.task.json` with task content
3. Worker writes `<uuid>.response.json` with result
4. Daemon cleans up all three files

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

2. **Cleanup timing** - When exactly should the daemon clean up the three files? After reading response, or let them age out?
