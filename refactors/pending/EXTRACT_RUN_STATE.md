# Extract RunState from TaskRunner

**Status:** Not started

**Depends on:** None

**Parent refactor:** `APPLY_PATTERN.md` (Phase 0a, 0d)

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

## What does NOT change (Phase 1)

- No logic changes. Every method does exactly what it did before.
- No new methods on RunState beyond `new()` and `next_id()`. Methods like `remove_and_notify_parent` stay on TaskRunner for now (they do I/O).
- TaskEntry and TaskState are unchanged.
- The Iterator impl is unchanged.
- Log writes, dispatch, and all I/O stay on TaskRunner.

## Phase 2: Move state-mutation methods onto RunState

**Depends on: Phase 1.**

Move `remove_and_notify_parent` and `decrement_pending_children` onto RunState. These currently live on TaskRunner because `decrement_pending_children` calls `schedule_finally` (I/O) when a parent's child count reaches zero. To make RunState I/O-free, replace that inline call with a deferred `removed_parents` vector.

### 2.1: Add removed_parents to RunState

```rust
struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    next_task_id: u32,
    removed_parents: Vec<RemovedParent>,
}

struct RemovedParent {
    task_id: LogTaskId,
    step: StepName,
    parent_id: Option<LogTaskId>,
    finally_data: Option<(HookScript, StepInputValue)>,
}
```

`RemovedParent` carries the same `finally_data` that's currently on `WaitingForChildren`. No data structure changes to TaskEntry or TaskState — `finally_data: Option<(HookScript, StepInputValue)>` stays as-is.

### 2.2: Move remove_and_notify_parent onto RunState

Before (on TaskRunner, line 636):

```rust
fn remove_and_notify_parent(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.remove(&task_id).expect("[P021] task must exist");
    if let Some(parent_id) = entry.parent_id {
        self.decrement_pending_children(parent_id);
    }
}
```

After (on RunState):

```rust
fn remove_and_notify_parent(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.remove(&task_id).expect("task must exist");
    let Some(parent_id) = entry.parent_id else { return };
    let Some(parent) = self.tasks.get_mut(&parent_id) else { return };
    let TaskState::WaitingForChildren {
        pending_children_count,
        finally_data,
    } = &mut parent.state
    else {
        return;
    };

    let new_count = pending_children_count.get() - 1;
    if new_count > 0 {
        *pending_children_count = NonZeroU16::new(new_count).unwrap();
    } else {
        let step = parent.step.clone();
        let fd = finally_data.take();
        self.removed_parents.push(RemovedParent {
            task_id: parent_id,
            step,
            parent_id: parent.parent_id,
            finally_data: fd,
        });
        self.remove_and_notify_parent(parent_id);
    }
}
```

This is the same logic as `decrement_pending_children` today, except instead of calling `schedule_finally` it pushes to `removed_parents`. The recursive call to `remove_and_notify_parent` handles cascading removals up the tree, same as today.

### 2.3: Add drain method

```rust
impl RunState {
    fn drain_removed_parents(&mut self) -> Vec<RemovedParent> {
        std::mem::take(&mut self.removed_parents)
    }
}
```

### 2.4: Update TaskRunner callers

TaskRunner methods that currently call `self.remove_and_notify_parent(id)` change to `self.state.remove_and_notify_parent(id)`. After the call, TaskRunner drains `removed_parents` and handles finally scheduling.

Before (`decrement_pending_children` on TaskRunner, line 646):

```rust
fn decrement_pending_children(&mut self, task_id: LogTaskId) {
    let (hit_zero, finally_data) = {
        let entry = self.tasks.get_mut(&task_id).expect("[P022] task must exist");
        let TaskState::WaitingForChildren {
            pending_children_count,
            finally_data,
        } = &mut entry.state
        else {
            panic!("[P023] task not in WaitingForChildren state");
        };
        let new_count = pending_children_count.get() - 1;
        if new_count == 0 {
            (true, finally_data.take())
        } else {
            *pending_children_count = NonZeroU16::new(new_count).unwrap();
            (false, None)
        }
    };
    if hit_zero {
        if let Some((hook, value)) = finally_data {
            self.schedule_finally(task_id, hook, value);
        }
        self.remove_and_notify_parent(task_id);
    }
}
```

After: `decrement_pending_children` is deleted from TaskRunner. Its logic is now inside `RunState::remove_and_notify_parent`. The callers of `remove_and_notify_parent` on TaskRunner (`task_succeeded` and `task_failed`) call through to RunState and then drain:

```rust
// In task_succeeded, after the state-mutation part:
self.state.remove_and_notify_parent(task_id);
for parent in self.state.drain_removed_parents() {
    if let Some((hook, value)) = parent.finally_data {
        self.schedule_finally(parent.task_id, hook, value);
    }
}
```

`schedule_finally` stays on TaskRunner unchanged — it still does I/O (log writes, task insertion). The only change is where it's called from: previously inline inside `decrement_pending_children`, now after draining `removed_parents`.

### 2.5: Delete decrement_pending_children and increment_pending_children

`decrement_pending_children` is replaced by `RunState::remove_and_notify_parent`.

`increment_pending_children` (line 620) is only called by `schedule_finally`. It stays on TaskRunner since it mutates `self.state.tasks` directly — it's a one-liner that increments a parent's `pending_children_count`. It can move onto RunState too for consistency, but it's not required.

## Testing

### Phase 1

Purely structural. All existing tests pass unchanged. No new tests needed.

### Phase 2

RunState can now be tested in isolation:

```rust
#[test] fn remove_and_notify_parent_removes_task()
#[test] fn remove_and_notify_parent_decrements_parent_count()
#[test] fn remove_and_notify_parent_captures_removed_parent_when_count_hits_zero()
#[test] fn remove_and_notify_parent_cascades_up_tree()
#[test] fn removed_parent_carries_finally_data()
#[test] fn drain_removed_parents_empties_vector()
```

All existing integration tests should pass unchanged — the behavior is identical, just the call path is different.
