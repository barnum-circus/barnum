# Extract RunState from TaskRunner

**Status:** Completed

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

`RemovedParent` carries the same `finally_data` that's currently on `WaitingForChildren`. It also carries `step` and `parent_id` because the task will already be removed from the map by the time `schedule_finally` runs, so these must be captured eagerly.

### 2.2: Move remove_and_notify_parent onto RunState (non-recursive)

The method is **non-recursive**. When a parent's child count hits zero, it pushes to `removed_parents` but does NOT remove the parent or recurse. The caller drives the cascade.

```rust
fn remove_and_notify_parent(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.remove(&task_id).expect("[P021] task must exist");
    let Some(parent_id) = entry.parent_id else { return };
    let parent = self.tasks.get_mut(&parent_id).expect("[P022] parent task must exist");
    let TaskState::WaitingForChildren {
        pending_children_count,
        finally_data,
    } = &mut parent.state
    else {
        panic!("[P023] parent task not in WaitingForChildren state");
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
        // NOT recursive — caller drives cascade via schedule_removed_finally
    }
}
```

**Why non-recursive?** `schedule_finally` calls `increment_pending_children` on the grandparent. If we recursively remove all ancestors first, the grandparent is gone by the time we try to increment it. The cascade must interleave removal with finally scheduling: remove child -> schedule finally (increments grandparent) -> remove parent -> schedule finally -> ...

### 2.3: Update schedule_finally signature

`schedule_finally` now takes explicit `parent_id` and `step` parameters instead of looking them up from the task map, since the task may already be removed when called from the drain loop:

```rust
fn schedule_finally(
    &mut self,
    task_id: LogTaskId,
    parent_id: Option<LogTaskId>,
    step: &StepName,
    hook: HookScript,
    value: StepInputValue,
) { ... }
```

### 2.4: Add schedule_removed_finally on TaskRunner

This method drives the cascade loop, interleaving finally scheduling with parent removal:

```rust
fn schedule_removed_finally(&mut self) {
    while let Some(removed) = self.state.removed_parents.pop() {
        if let Some((hook, value)) = removed.finally_data {
            self.schedule_finally(
                removed.task_id, removed.parent_id, &removed.step, hook, value,
            );
        }
        self.state.remove_and_notify_parent(removed.task_id);
    }
}
```

Callers (`task_succeeded`, `task_failed`) call `self.state.remove_and_notify_parent(task_id)` then `self.schedule_removed_finally()`.

### 2.5: Delete decrement_pending_children and increment_pending_children

`decrement_pending_children` is replaced by `RunState::remove_and_notify_parent`.

`increment_pending_children` stays on TaskRunner since its only caller (`schedule_finally`) stays on TaskRunner.

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
```

All existing integration tests should pass unchanged — the behavior is identical, just the call path is different.
