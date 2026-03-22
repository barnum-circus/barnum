# Extract RunState from TaskRunner

**Status:** Not started

**Depends on:** None

**Parent refactor:** `APPLY_PATTERN.md` (Phase 0a)

## Motivation

`TaskRunner` mixes task-tree state (`tasks`, `next_task_id`) with I/O concerns (`pool`, `state_log`, `tx`/`rx`). Extracting the task-tree state into its own struct creates a testable unit with no I/O dependencies and sets up the foundation for the apply pattern refactor.

## Current state

`TaskRunner` (`crates/barnum_config/src/runner/mod.rs:152`):

```rust
struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    tasks: BTreeMap<LogTaskId, TaskEntry>,       // moves to RunState
    pool: PoolConnection,
    max_concurrency: usize,
    in_flight: usize,
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,
    next_task_id: u32,                           // moves to RunState
    state_log: io::BufWriter<std::fs::File>,
}
```

`tasks` and `next_task_id` are accessed by every method that touches task state. They have no dependency on any of the other fields (pool, state_log, tx/rx, etc.).

## Changes

### 1. Create RunState struct

New struct in `crates/barnum_config/src/runner/mod.rs` (or a new `run_state.rs` file):

```rust
struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    next_task_id: u32,
}

impl RunState {
    fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            next_task_id: 0,
        }
    }

    fn next_id(&mut self) -> LogTaskId {
        let id = LogTaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }
}
```

### 2. Replace fields on TaskRunner

Before:

```rust
struct TaskRunner<'a> {
    // ...
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    next_task_id: u32,
    // ...
}
```

After:

```rust
struct TaskRunner<'a> {
    // ...
    state: RunState,
    // ...
}
```

### 3. Update all references

Every `self.tasks` becomes `self.state.tasks`. Every `self.next_task_id` reference becomes either `self.state.next_task_id` (direct access) or `self.state.next_id()` (for the allocation pattern).

Affected methods and their access patterns:

| Method | `self.tasks` accesses | `self.next_task_id` accesses |
|--------|----------------------|------------------------------|
| `queue_task` (line 411) | `insert` | `next_task_id()` → `self.state.next_id()` |
| `task_succeeded` (line 695) | `get`, `get_mut` | `self.next_task_id` (pre-compute child IDs) |
| `task_failed` (line 761) | `get`, `remove` | — |
| `remove_and_notify_parent` (line 636) | `remove` | — |
| `decrement_pending_children` (line 646) | `get_mut` | — |
| `schedule_finally` (line 580) | `get`, `insert` | `next_task_id()` → `self.state.next_id()` |
| `increment_pending_children` (line 620) | `get_mut` | — |
| `dispatch_all_pending` (line 831) | iteration | — |
| `process_result` / response handling | `get` | — |

The child ID pre-computation in `task_succeeded` (line 718):

```rust
// Before
let first_child_id = self.next_task_id;
let spawned_task_ids: Vec<LogTaskId> = (0..spawned.len())
    .map(|i| LogTaskId(first_child_id + i as u32))
    .collect();

// After
let first_child_id = self.state.next_task_id;
let spawned_task_ids: Vec<LogTaskId> = (0..spawned.len())
    .map(|i| LogTaskId(first_child_id + i as u32))
    .collect();
```

### 4. Update resume logic

The resume path reconstructs `tasks` and `next_task_id` from the NDJSON log. This code moves into `RunState` methods or into the coordinator that builds `RunState` from the log. Check the resume path in `run()` or wherever the log is replayed to update field access.

## What does NOT change

- No logic changes. Every method does exactly what it did before.
- No new methods on RunState beyond `new()` and `next_id()`. Methods like `remove_and_notify_parent` stay on TaskRunner for now (they do I/O).
- TaskEntry and TaskState are unchanged.
- The Iterator impl is unchanged.
- Log writes, dispatch, and all I/O stay on TaskRunner.

## Testing

This is a purely structural refactor. All existing tests should pass with no changes. If any test directly constructs `TaskRunner` and accesses `.tasks` or `.next_task_id`, update it to use `.state.tasks` and `.state.next_task_id`.

No new tests are needed for this change since no behavior changes. New RunState-specific tests come in later sub-refactors when methods move onto RunState.
