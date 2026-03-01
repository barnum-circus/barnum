# File Reorganization

**Status:** Analysis complete, awaiting approval

## Phase 1: Current File Inventory

### Root Level Files

| File | Functionality |
|------|--------------|
| `agent.rs` | **Task executor utilities** - `create_watcher`, `verify_watcher_sync`, `is_task_ready`, `wait_for_task`, `wait_for_task_with_timeout`, `AgentEvent` enum, platform-specific `is_file_write_event` |
| `constants.rs` | **Protocol constants** - `AGENTS_DIR`, `SUBMISSIONS_DIR`, `SCRATCH_DIR`, `LOCK_FILE`, `SOCKET_NAME`, `TASK_FILE`, `RESPONSE_FILE`, `REQUEST_SUFFIX`, `RESPONSE_SUFFIX`, `STATUS_FILE` |
| `fs_util.rs` | **Filesystem utilities** - `atomic_write`, `atomic_write_str`, `VerifiedWatcher`, `CanaryGuard`, `WatcherState` |
| `lib.rs` | **Re-exports** - Public API surface |
| `lock.rs` | **Process lock management** - `LockGuard`, `acquire_lock`, `is_daemon_running`, `is_process_running` |
| `pool.rs` | **Pool management** - `default_pool_root`, `generate_id`, `id_to_path`, `list_pools`, `resolve_pool`, `cleanup_stopped`, `PoolInfo` |
| `response.rs` | **Response protocol** - `Response` enum (Processed/NotProcessed), `ResponseKind`, `NotProcessedReason` |
| `transport.rs` | **Transport abstraction** - `Transport` enum (Directory/Socket), `read`, `write`, `path` |
| `types.rs` | **Domain types** - `AgentName`, `PoolId` newtypes |

### client/ Folder

| File | Functionality |
|------|--------------|
| `mod.rs` | Re-exports + `wait_for_pool_ready` function |
| `payload.rs` | `Payload` enum (Inline/FileReference) for task content |
| `stop.rs` | `stop` function - sends SIGTERM to daemon |
| `submit.rs` | Socket-based task submission |
| `submit_file.rs` | File-based task submission + `cleanup_submission` |

### daemon/ Folder

| File | Functionality |
|------|--------------|
| `mod.rs` | Re-exports |
| `core.rs` | Pure state machine - `AgentId`, `TaskId`, `Epoch`, `AgentState`, `Event`, `Effect`, `step()` |
| `io.rs` | I/O layer - `TransportMap`, `AgentMap`, `TaskIdAllocator`, `ExternalTaskData`, `IoConfig` |
| `path_category.rs` | FS event categorization - `PathCategory` enum, `categorize()`, `is_write_complete()` |
| `wiring.rs` | Thread management - `run`, `spawn`, `DaemonConfig`, `DaemonHandle`, main loops |

---

## Phase 2: Proposed Reorganization

### Conceptual Model

The user identifies three actors:
1. **Daemon** - the server that orchestrates task distribution
2. **Executor** - the thing that executes tasks (currently called "agent")
3. **Submitter** - the thing that submits tasks (currently called "client")

Plus shared utilities used by multiple actors.

### Naming Changes

| Current | Proposed | Rationale |
|---------|----------|-----------|
| `agent.rs` | `executor/` | "Executor" describes what it does (executes tasks) rather than what it is |
| `client/` | `submit/` | "Submit" is the action; "client" is too generic |
| `AgentName` | Keep | Still refers to the human-readable name |

### Proposed Structure

```
src/
├── lib.rs                  # Public API re-exports
├── constants.rs            # Protocol constants (shared)
├── types.rs                # Domain newtypes: AgentName, PoolId (shared)
│
├── common/                 # Shared utilities
│   ├── mod.rs
│   ├── fs.rs               # atomic_write, VerifiedWatcher
│   ├── lock.rs             # LockGuard, acquire_lock, is_daemon_running
│   ├── pool.rs             # Pool ID management
│   ├── response.rs         # Response protocol types
│   └── transport.rs        # Transport abstraction
│
├── daemon/                 # The daemon (unchanged internally)
│   ├── mod.rs
│   ├── core.rs
│   ├── io.rs
│   ├── path_category.rs
│   └── wiring.rs
│
├── submit/                 # Task submission (renamed from client/)
│   ├── mod.rs              # Re-exports + wait_for_pool_ready
│   ├── payload.rs          # Payload types
│   ├── socket.rs           # Socket-based submission (was submit.rs)
│   ├── file.rs             # File-based submission (was submit_file.rs)
│   └── stop.rs             # Stop daemon
│
└── executor/               # Task execution (was agent.rs)
    ├── mod.rs              # Re-exports
    ├── watcher.rs          # create_watcher, verify_watcher_sync, AgentEvent
    └── wait.rs             # is_task_ready, wait_for_task, wait_for_task_with_timeout
```

### Alternative: Flatter Structure

If `common/` feels like too much nesting, we could keep shared files at root:

```
src/
├── lib.rs
├── constants.rs            # Shared
├── types.rs                # Shared
├── fs.rs                   # Shared (was fs_util.rs)
├── lock.rs                 # Shared
├── pool.rs                 # Shared
├── response.rs             # Shared
├── transport.rs            # Shared
│
├── daemon/                 # Unchanged
│   └── ...
│
├── submit/                 # Renamed from client/
│   └── ...
│
└── executor/               # New folder from agent.rs
    └── ...
```

This keeps the flat structure for shared code but renames/reorganizes the actor-specific modules.

---

## Detailed File Mappings

### Option A: With common/ folder

| Current Path | New Path |
|--------------|----------|
| `agent.rs` | Split into `executor/watcher.rs` + `executor/wait.rs` |
| `client/mod.rs` | `submit/mod.rs` |
| `client/payload.rs` | `submit/payload.rs` |
| `client/stop.rs` | `submit/stop.rs` |
| `client/submit.rs` | `submit/socket.rs` |
| `client/submit_file.rs` | `submit/file.rs` |
| `constants.rs` | `constants.rs` (unchanged) |
| `daemon/*` | `daemon/*` (unchanged) |
| `fs_util.rs` | `common/fs.rs` |
| `lib.rs` | `lib.rs` (update re-exports) |
| `lock.rs` | `common/lock.rs` |
| `pool.rs` | `common/pool.rs` |
| `response.rs` | `common/response.rs` |
| `transport.rs` | `common/transport.rs` |
| `types.rs` | `types.rs` (unchanged) |

### Option B: Flatter (no common/ folder)

| Current Path | New Path |
|--------------|----------|
| `agent.rs` | Split into `executor/watcher.rs` + `executor/wait.rs` |
| `client/mod.rs` | `submit/mod.rs` |
| `client/payload.rs` | `submit/payload.rs` |
| `client/stop.rs` | `submit/stop.rs` |
| `client/submit.rs` | `submit/socket.rs` |
| `client/submit_file.rs` | `submit/file.rs` |
| `constants.rs` | `constants.rs` (unchanged) |
| `daemon/*` | `daemon/*` (unchanged) |
| `fs_util.rs` | `fs.rs` (rename only) |
| `lib.rs` | `lib.rs` (update re-exports) |
| `lock.rs` | `lock.rs` (unchanged) |
| `pool.rs` | `pool.rs` (unchanged) |
| `response.rs` | `response.rs` (unchanged) |
| `transport.rs` | `transport.rs` (unchanged) |
| `types.rs` | `types.rs` (unchanged) |

---

## Questions for Review

1. **common/ folder or flat?** - The `common/` folder groups shared utilities but adds nesting. Flat keeps things simpler but mixes shared and actor-specific at the same level.

2. **executor/ split?** - Currently `agent.rs` is ~325 lines. Split into `watcher.rs` (canary/watcher setup) and `wait.rs` (waiting for tasks), or keep as one file `executor.rs`?

3. **submit/ file naming** - `socket.rs` vs `submit_socket.rs`? The folder already provides context, but explicit names are clearer in imports.

4. **Transport location** - `Transport` is used by daemon, executor, and potentially submit. Is `common/` the right place, or root level?

---

## Implementation Plan

After approval:

1. Create new directory structure
2. Move files with `git mv`
3. Update `mod.rs` files
4. Update `lib.rs` re-exports
5. Fix all internal imports
6. Verify tests pass
7. Update any documentation references
