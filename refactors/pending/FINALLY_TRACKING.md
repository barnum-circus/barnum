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

**Test:** TODO

**Setup:** A (with finally) → B (with finally that spawns cleanup tasks)

When B's finally runs, it may spawn additional tasks. Currently these are queued as "new roots" with `finally_origin_id: None`. A should wait for these.

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

enum TaskState {
    /// Waiting to be dispatched
    Pending { task: Task },

    /// Currently executing (dispatched to agent/command)
    /// Only need step_name to look up finally hook when task completes
    InFlight { step_name: StepName },

    /// Execution succeeded, waiting for descendants to complete before running finally
    WaitingForDescendants {
        pending_count: NonZeroU16,
        effective_value: EffectiveValue,
        step_name: StepName,  // Look up finally_hook from config when needed
    },
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
    let task_id = self.tasks.iter()
        .find(|(_, entry)| matches!(entry.state, TaskState::Pending { .. }))
        .map(|(id, _)| *id)?;

    // Transition Pending → InFlight, return task for dispatching
    let entry = self.tasks.get_mut(&task_id).unwrap();
    if let TaskState::Pending { task } = &entry.state {
        let task_to_dispatch = task.clone();
        entry.state = TaskState::InFlight { step_name: task.step.clone() };
        self.in_flight += 1;
        return Some((task_id, task_to_dispatch));
    }

    None
}
```

#### Task completes successfully

```rust
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, effective_value: EffectiveValue) {
    let entry = self.tasks.get(&task_id).unwrap();
    let step_name = match &entry.state {
        TaskState::InFlight { step_name } => step_name.clone(),
        _ => panic!("task_succeeded on non-InFlight task"),
    };

    self.in_flight -= 1;

    if spawned.is_empty() {
        // No children - run finally (if any) and mark done
        self.task_fully_done(task_id, step_name, effective_value);
    } else {
        // Has children - transition to WaitingForDescendants
        let parent_id = entry.parent_id;
        let count = NonZeroU16::new(spawned.len() as u16).unwrap();
        self.tasks.insert(task_id, TaskEntry {
            parent_id,
            state: TaskState::WaitingForDescendants {
                pending_count: count,
                effective_value,
                step_name,
            },
        });

        // Queue children
        for child_task in spawned {
            let child_id = self.next_task_id();
            self.tasks.insert(child_id, TaskEntry {
                parent_id: Some(task_id),  // Always immediate parent!
                state: TaskState::Pending { task: child_task },
            });
        }
    }
}
```

#### Task fully done (all descendants complete)

```rust
fn task_fully_done(&mut self, task_id: LogTaskId, step_name: StepName, effective_value: EffectiveValue) {
    let entry = self.tasks.remove(&task_id).unwrap();
    let parent_id = entry.parent_id;

    // Look up finally hook from config
    let finally_hook = self.config.steps.get(&step_name).and_then(|s| s.finally_hook.as_ref());

    // Run finally hook if present
    if let Some(hook) = finally_hook {
        let spawned = run_finally_hook(hook, &effective_value);

        if !spawned.is_empty() {
            // Finally spawned tasks - re-add ourselves as WaitingForDescendants
            let count = NonZeroU16::new(spawned.len() as u16).unwrap();
            self.tasks.insert(task_id, TaskEntry {
                parent_id,
                state: TaskState::WaitingForDescendants {
                    pending_count: count,
                    effective_value,
                    step_name: StepName::FinallyCleanup,  // Marker: finally already ran
                },
            });

            // Queue finally-spawned tasks with this task as parent
            for child_task in spawned {
                let child_id = self.next_task_id();
                self.tasks.insert(child_id, TaskEntry {
                    parent_id: Some(task_id),
                    state: TaskState::Pending { task: child_task },
                });
            }
            return;  // Don't notify parent yet
        }
    }

    // Notify parent (if any)
    if let Some(pid) = parent_id {
        self.decrement_parent(pid);
    }
}

fn decrement_parent(&mut self, parent_id: LogTaskId) {
    let entry = self.tasks.get_mut(&parent_id).unwrap();

    match &mut entry.state {
        TaskState::WaitingForDescendants { pending_count, effective_value, step_name } => {
            let new_count = pending_count.get() - 1;
            if new_count == 0 {
                // Parent is now fully done
                let ev = effective_value.clone();
                let sn = step_name.clone();
                self.task_fully_done(parent_id, sn, ev);
            } else {
                *pending_count = NonZeroU16::new(new_count).unwrap();
            }
        }
        _ => panic!("decrement_parent on non-WaitingForDescendants task"),
    }
}
```

### Example Trace

```
A (finally) spawns B (finally), B spawns C

Initial:
  tasks[0/A] = { parent: None, state: Pending { task: A } }
  in_flight = 0

A dispatched:
  tasks[0/A] = { parent: None, state: InFlight { step: "A" } }
  in_flight = 1

A succeeds, spawns B:
  tasks[0/A] = { parent: None, state: WaitingForDescendants { count: 1, step: "A" } }
  tasks[1/B] = { parent: Some(0), state: Pending { task: B } }
  in_flight = 0

B dispatched:
  tasks[0/A] = { parent: None, state: WaitingForDescendants { count: 1, step: "A" } }
  tasks[1/B] = { parent: Some(0), state: InFlight { step: "B" } }
  in_flight = 1

B succeeds, spawns C:
  tasks[0/A] = { parent: None, state: WaitingForDescendants { count: 1, step: "A" } }
  tasks[1/B] = { parent: Some(0), state: WaitingForDescendants { count: 1, step: "B" } }
  tasks[2/C] = { parent: Some(1), state: Pending { task: C } }
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

- Existing tests should pass (behavior unchanged for correct cases)
- `subtree_finally_waits_for_grandchildren` should pass (Bug 1 fixed)
- Add test for finally-spawned tasks (Bug 2)
- Add test for deeply nested finally chains
