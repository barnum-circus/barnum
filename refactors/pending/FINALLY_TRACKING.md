# Finally Tracking Refactor

**Status:** Not started

**Prerequisites:** VALUE_AND_RETRY_MODEL (COMPLETED - see `refactors/past/VALUE_AND_RETRY_MODEL.md`)

**Blocks:** FINALLY_SCHEDULING

## Known Bugs (with tests on `test-subtree-finally-bug` branch)

### Bug 1: A's finally doesn't wait for grandchildren

**Test:** `subtree_finally_waits_for_grandchildren`

**Setup:** A (with finally) → B (with finally) → C (no finally)

**Expected order:** `C_done, B_finally, A_finally`
**Actual order:** `A_finally, C_done, B_finally`

**Root cause:** In `mod.rs:317-319`, we notify the origin when a task succeeds, even if that task set up its own finally tracking for children. A gets notified when B succeeds, not when B's finally completes.

### Bug 2: A's finally doesn't wait for B's finally-spawned tasks

**Test:** `finally_waits_for_finally_spawned_tasks` (on `test-finally-spawned-tasks` branch)

**Setup:** A (with finally) → B (with finally that spawns cleanup task C)

**Expected order:** `B_finally, C_done, A_finally`
**Actual order:** `B_finally, A_finally, C_done`

**Root cause:** When B's finally runs and spawns cleanup tasks, they are queued as "new roots" with `finally_origin_id: None`. A's finally runs immediately when B's finally completes, not waiting for the cleanup tasks.

---

## Existing Types (for reference)

These types already exist and will be reused:

- **`EffectiveValue`** (`types.rs:124`): Newtype wrapper `pub struct EffectiveValue(pub serde_json::Value)`. The task value after pre-hook transformation.

- **`run_finally_hook`** (`finally.rs:88`): Takes `&FinallyState`, returns `Vec<Task>`. Runs the shell command with the original value as JSON stdin, parses stdout as task array.

- **`run_finally_hook_direct`** (`finally.rs:95`): Takes `&HookScript` and `&serde_json::Value` directly. Used when task has no children (finally runs immediately).

---

## Motivation

The current implementation scatters task state across multiple data structures:
- `VecDeque<QueuedTask>` for pending tasks
- `in_flight: usize` counter (doesn't even track which tasks!)
- `HashMap<LogTaskId, FinallyState>` for finally tracking

This makes it hard to reason about, hard to test, and impossible to reconstruct from logs.

---

## Proposed Design: Unified Task State Map

Replace scattered task tracking with a single `BTreeMap<LogTaskId, TaskEntry>`.

### Data Structures

```rust
use std::collections::BTreeMap;

struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    pool: PoolConnection,
    max_concurrency: usize,
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,
    next_task_id: u32,

    /// All task state in one place. Tasks not in this map are fully done.
    tasks: BTreeMap<LogTaskId, TaskEntry>,

    /// Cached count of InFlight tasks (for concurrency limiting)
    in_flight: usize,
}

/// Wrapper containing parent relationship and current state
struct TaskEntry {
    parent_id: Option<LogTaskId>,
    state: TaskState,
}

/// Task waiting to be dispatched
struct PendingTask {
    task: Task,
}

/// Task currently executing (dispatched to agent/command)
struct InFlightTask {
    step_name: StepName,  // Only need this to look up finally hook when task completes
}

/// Task succeeded, waiting for descendants to complete before running finally
struct WaitingForDescendants {
    pending_count: NonZeroU16,
    effective_value: EffectiveValue,
    step_name: StepName,       // Look up finally_hook from config when needed
    finally_already_ran: bool, // True if waiting for finally-spawned tasks
}

enum TaskState {
    Pending(PendingTask),
    InFlight(InFlightTask),
    WaitingForDescendants(WaitingForDescendants),
}
```

### Why BTreeMap?

- `LogTaskId` is monotonically increasing
- BTreeMap ordering by key = FIFO dispatch order automatically
- "Next task to dispatch" = first `Pending` entry when iterating
- Single source of truth for all task states

### Why keep `in_flight` counter?

Could calculate via `tasks.values().filter(|e| matches!(e.state, InFlight { .. })).count()`, but that's O(n) on every dispatch check. Keep a cached counter instead - increment on `Pending→InFlight`, decrement on `InFlight→{WaitingForDescendants, removed}`.

### Task Lifecycle

```
                    ┌──────────────────────────────────────┐
                    │                                      │
                    ▼                                      │
Task created → [Pending] → [InFlight] ──┬── success ──────┼──┬── no children ────→ done (remove from map)
                    ▲                   │                 │  │
                    │                   │                 │  └── has children ───→ [WaitingForDescendants]
                    │                   │                 │                              │
                    │                   ├── retry ───────►│                              │ all descendants done
                    │                   │                 │                              ▼
                    │                   └── dropped ─────►└──────────────────────── run finally, notify parent
                    │                                                                    │
                    └────────────────────────────────────────────────────────────────────┘
                                        (finally may spawn new Pending tasks)
```

### Key Operations

#### Dispatch next task

```rust
fn dispatch_next(&mut self) -> Option<(LogTaskId, Task)> {
    if self.in_flight >= self.max_concurrency {
        return None;
    }

    // Find first Pending task (BTreeMap gives us FIFO order)
    let (task_id, pending) = self.tasks.iter()
        .find_map(|(id, entry)| match &entry.state {
            TaskState::Pending(p) => Some((*id, p)),
            _ => None,
        })?;

    let task_to_dispatch = pending.task.clone();
    let step_name = pending.task.step.clone();

    // Transition Pending → InFlight
    let entry = self.tasks.get_mut(&task_id).expect("task_id from iterator must exist");
    entry.state = TaskState::InFlight(InFlightTask { step_name });
    self.in_flight += 1;
    Some((task_id, task_to_dispatch))
}
```

#### Task completes successfully

```rust
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, effective_value: EffectiveValue) {
    let entry = self.tasks.get(&task_id).expect("task_succeeded called with unknown task_id");
    let TaskState::InFlight(in_flight) = &entry.state else {
        panic!("task_succeeded on non-InFlight task");
    };
    let step_name = in_flight.step_name.clone();
    let parent_id = entry.parent_id;

    self.in_flight -= 1;

    if spawned.is_empty() {
        // No children - run finally (if any) and mark done
        self.task_fully_done(task_id, step_name, effective_value, false);
    } else {
        // Has children - transition to WaitingForDescendants
        let count = NonZeroU16::new(spawned.len() as u16).expect("spawned is non-empty");
        self.tasks.insert(task_id, TaskEntry {
            parent_id,
            state: TaskState::WaitingForDescendants(WaitingForDescendants {
                pending_count: count,
                effective_value,
                step_name,
                finally_already_ran: false,
            }),
        });

        // Queue children
        for child_task in spawned {
            let child_id = self.next_task_id();
            self.tasks.insert(child_id, TaskEntry {
                parent_id: Some(task_id),  // Always immediate parent!
                state: TaskState::Pending(PendingTask { task: child_task }),
            });
        }
    }
}
```

#### Task fully done (all descendants complete)

```rust
fn task_fully_done(
    &mut self,
    task_id: LogTaskId,
    step_name: StepName,
    effective_value: EffectiveValue,
    finally_already_ran: bool,
) {
    let entry = self.tasks.remove(&task_id).expect("task_fully_done called with unknown task_id");
    let parent_id = entry.parent_id;

    // Run finally hook if present AND not already run
    if !finally_already_ran {
        let finally_hook = self.config.steps.get(&step_name).and_then(|s| s.finally_hook.as_ref());

        if let Some(hook) = finally_hook {
            let spawned = run_finally_hook_direct(hook, &effective_value.0);

            if !spawned.is_empty() {
                // Finally spawned tasks - re-add ourselves as WaitingForDescendants
                let count = NonZeroU16::new(spawned.len() as u16).expect("spawned is non-empty");
                self.tasks.insert(task_id, TaskEntry {
                    parent_id,
                    state: TaskState::WaitingForDescendants(WaitingForDescendants {
                        pending_count: count,
                        effective_value,
                        step_name,
                        finally_already_ran: true,  // Mark: finally already ran
                    }),
                });

                // Queue finally-spawned tasks with this task as parent
                for child_task in spawned {
                    let child_id = self.next_task_id();
                    self.tasks.insert(child_id, TaskEntry {
                        parent_id: Some(task_id),
                        state: TaskState::Pending(PendingTask { task: child_task }),
                    });
                }
                return;  // Don't notify parent yet
            }
        }
    }

    // Notify parent (if any)
    if let Some(pid) = parent_id {
        self.decrement_parent(pid);
    }
}

fn decrement_parent(&mut self, parent_id: LogTaskId) {
    let entry = self.tasks.get_mut(&parent_id).expect("parent_id must exist in tasks");

    let TaskState::WaitingForDescendants(waiting) = &mut entry.state else {
        panic!("decrement_parent on non-WaitingForDescendants task");
    };

    let new_count = waiting.pending_count.get() - 1;
    if new_count == 0 {
        // Parent is now fully done
        let ev = waiting.effective_value.clone();
        let sn = waiting.step_name.clone();
        let far = waiting.finally_already_ran;
        self.task_fully_done(parent_id, sn, ev, far);
    } else {
        waiting.pending_count = NonZeroU16::new(new_count).expect("new_count > 0 checked above");
    }
}
```

### Example Trace

```
A (finally) spawns B (finally), B spawns C

Initial:
  tasks[0/A] = { parent: None, state: Pending(task: A) }
  in_flight = 0

A dispatched:
  tasks[0/A] = { parent: None, state: InFlight(step: "A") }
  in_flight = 1

A succeeds, spawns B:
  tasks[0/A] = { parent: None, state: WaitingForDescendants(count: 1, step: "A") }
  tasks[1/B] = { parent: Some(0), state: Pending(task: B) }
  in_flight = 0

B dispatched:
  tasks[0/A] = { parent: None, state: WaitingForDescendants(count: 1, step: "A") }
  tasks[1/B] = { parent: Some(0), state: InFlight(step: "B") }
  in_flight = 1

B succeeds, spawns C:
  tasks[0/A] = { parent: None, state: WaitingForDescendants(count: 1, step: "A") }
  tasks[1/B] = { parent: Some(0), state: WaitingForDescendants(count: 1, step: "B") }
  tasks[2/C] = { parent: Some(1), state: Pending(task: C) }
  in_flight = 0

C dispatched and succeeds (no children, no finally):
  tasks[2/C] removed
  decrement_parent(1/B): count 1→0
  B fully done:
    tasks[1/B] removed
    look up config.steps["B"].finally_hook → run it (spawns nothing)
    decrement_parent(0/A): count 1→0
    A fully done:
      tasks[0/A] removed
      look up config.steps["A"].finally_hook → run it
      no parent to notify

  tasks = {} (empty, all done)
```

Order of finally hooks: B_finally, A_finally ✓

### Retry Handling

When a task fails and will retry:
1. Create a new `Pending` task with new ID but **same parent_id**
2. Do NOT decrement the parent's `pending_count` - the logical task is still in progress
3. The retried task is a continuation, not a new descendant

When a task is dropped (max retries exceeded):
1. Remove the task from the map
2. Decrement parent's `pending_count` (the descendant is "done" even though it failed)
3. If parent's count reaches 0, parent runs its finally hook

```rust
fn task_retried(&mut self, task_id: LogTaskId, retry_task: Task) {
    let entry = self.tasks.remove(&task_id).expect("task_retried on unknown task");
    let parent_id = entry.parent_id;

    self.in_flight -= 1;

    // Queue retry with same parent - parent's count stays the same
    let new_id = self.next_task_id();
    self.tasks.insert(new_id, TaskEntry {
        parent_id,  // Keep same parent relationship
        state: TaskState::Pending(PendingTask { task: retry_task }),
    });
}

fn task_dropped(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.remove(&task_id).expect("task_dropped on unknown task");
    let parent_id = entry.parent_id;

    self.in_flight -= 1;

    // Notify parent - descendant is done (failed)
    if let Some(pid) = parent_id {
        self.decrement_parent(pid);
    }
}
```

### StepName Marker for Finally-Spawned Tasks

When a finally hook spawns tasks, we re-add the task to `WaitingForDescendants` but need to avoid running the finally hook again. Options:

1. **Use a boolean flag** in `WaitingForDescendants`: `finally_already_ran: bool`
2. **Use a sentinel StepName** like `StepName::new("__finally_cleanup__")`

**Decision:** Use a boolean flag `finally_already_ran: bool`. Simpler, doesn't require special StepName handling.

```rust
struct WaitingForDescendants {
    pending_count: NonZeroU16,
    effective_value: EffectiveValue,
    step_name: StepName,
    finally_already_ran: bool,  // <-- Add this
}
```

---

## Files Changed

- `crates/gsd_config/src/runner/mod.rs`
  - Replace `queue: VecDeque<QueuedTask>` with `tasks: BTreeMap<LogTaskId, TaskEntry>`
  - Keep `in_flight: usize` counter (but now properly maintained)
  - Remove `finally_tracker: FinallyTracker`
  - Rewrite dispatch/completion logic

- `crates/gsd_config/src/runner/types.rs`
  - Remove `QueuedTask` struct
  - Add `TaskEntry` wrapper struct
  - Add `TaskState` enum

- `crates/gsd_config/src/runner/finally.rs`
  - Remove `FinallyTracker` and `FinallyState`
  - Keep `run_finally_hook` function

---

## Testing

### Existing Tests (should continue passing)

All tests in `crates/gsd_config/tests/` that don't exercise the bugs should pass unchanged. The refactor doesn't change behavior for correct cases.

### Bug Fix Tests (should start passing)

These tests currently have `#[should_panic]` because they document bugs. After the refactor, remove `#[should_panic]`:

1. **`subtree_finally_waits_for_grandchildren`** - Bug 1 fix
   - Location: `crates/gsd_config/tests/finally_retry_bugs.rs`
   - Currently expects panic with "Finally hooks ran in wrong order"

2. **`finally_waits_for_finally_spawned_tasks`** - Bug 2 fix
   - Location: `crates/gsd_config/tests/finally_retry_bugs.rs`
   - Currently expects panic with "Finally hooks ran in wrong order"

### New Tests to Add

1. **Deeply nested finally chains** - A→B→C→D all with finally hooks
   - Verify order: D_finally, C_finally, B_finally, A_finally

2. **Retry with finally** - Task with finally that retries
   - Verify finally runs only once after final success/drop
   - Verify parent waits for retry to complete

3. **Multiple children with finally** - A spawns B and C, both with finally
   - Verify A waits for both B_finally and C_finally before A_finally

4. **Finally spawns multiple tasks** - A's finally spawns B and C
   - Verify parent (if any) waits for all finally-spawned tasks

### Test Execution Notes

Tests in `finally_retry_bugs.rs` require IPC (agent pool). They skip in the sandbox with "SKIP: IPC not available". To run them:

```bash
# Via command pool (outside sandbox):
./target/debug/agent_pool submit_task --pool cmd --notify file --data \
  '{"kind": "Task", "task": {"instructions": "Run tests", "data": {"cmd": "cargo test -p gsd_config --test finally_retry_bugs 2>&1"}}}'
```

When adding tests with `#[should_panic]` to document bugs, commit with `--no-verify` (pre-commit hook fails because the test skips in sandbox and doesn't panic).
