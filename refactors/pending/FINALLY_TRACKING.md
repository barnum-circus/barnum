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

struct TaskEntry {
    parent_id: Option<LogTaskId>,
    state: TaskState,
}

enum TaskState {
    /// Task waiting to be dispatched
    Pending(Task),
    /// Task currently executing
    InFlight,
    /// Task succeeded, waiting for children/cleanup to complete
    Waiting {
        pending_count: NonZeroU16,
        finally: Option<FinallyData>,
    },
}

/// Data needed to run a finally hook. Present = run finally when count hits 0.
/// None = no subsequent step, just done when count hits 0.
struct FinallyData {
    step_name: StepName,
    effective_value: EffectiveValue,
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
Task created → [Pending] → [InFlight] ──┬── success with children ──→ [Waiting { finally: Some/None }]
                    ▲                   │                                        │
                    │                   ├── success, no children, has finally ──→│ (run finally immediately)
                    │                   │    └── finally spawns ────────────────→│ [Waiting { finally: None }]
                    │                   │    └── finally spawns nothing ─────────┼──→ done
                    │                   │                                        │
                    │                   ├── success, no children, no finally ────┼──→ done
                    │                   ├── retry ──────────────────────────────→│ (new Pending, same parent)
                    │                   └── dropped ────────────────────────────→│ done (notify parent)
                    │                                                            │
                    │                                                            ▼ count hits 0
                    │                   ┌── finally.is_some() ──→ run finally ───┤
                    │                   │    └── spawns tasks ──→ [Waiting { finally: None }]
                    │                   │    └── spawns nothing ─────────────────┼──→ done
                    └───────────────────┴── finally.is_none() ───────────────────┴──→ done
```

### State Transitions

```rust
/// Pending → InFlight
fn dispatch(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.get_mut(&task_id).expect("task must exist");
    assert!(matches!(entry.state, TaskState::Pending(_)));
    entry.state = TaskState::InFlight;
    self.in_flight += 1;
}

/// InFlight → Waiting
fn start_waiting(
    &mut self,
    task_id: LogTaskId,
    pending_count: NonZeroU16,
    finally: Option<FinallyData>,
) {
    let entry = self.tasks.get_mut(&task_id).expect("task must exist");
    assert!(matches!(entry.state, TaskState::InFlight));
    entry.state = TaskState::Waiting { pending_count, finally };
    self.in_flight -= 1;
}

/// InFlight → removed
fn finish_in_flight(&mut self, task_id: LogTaskId) -> Option<LogTaskId> {
    let entry = self.tasks.remove(&task_id).expect("task must exist");
    assert!(matches!(entry.state, TaskState::InFlight));
    self.in_flight -= 1;
    entry.parent_id
}

/// Waiting → removed
fn finish_waiting(&mut self, task_id: LogTaskId) -> Option<LogTaskId> {
    let entry = self.tasks.remove(&task_id).expect("task must exist");
    assert!(matches!(entry.state, TaskState::Waiting { .. }));
    entry.parent_id
}

/// Add a new pending task
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>) {
    let id = self.next_task_id();
    self.tasks.insert(id, TaskEntry {
        parent_id,
        state: TaskState::Pending(task),
    });
}
```

### Key Operations

#### Dispatch next task

```rust
fn dispatch_next(&mut self) -> Option<(LogTaskId, Task)> {
    if self.in_flight >= self.max_concurrency {
        return None;
    }

    let (task_id, task) = self.tasks.iter()
        .find_map(|(id, entry)| match &entry.state {
            TaskState::Pending(t) => Some((*id, t.clone())),
            _ => None,
        })?;

    self.dispatch(task_id);
    Some((task_id, task))
}
```

#### Task succeeds

```rust
fn task_succeeded(&mut self, task_id: LogTaskId, step_name: StepName, spawned: Vec<Task>, effective_value: EffectiveValue) {
    let parent_id = self.tasks.get(&task_id).expect("task must exist").parent_id;
    let finally_hook = self.config.steps.get(&step_name).and_then(|s| s.finally_hook.clone());

    if !spawned.is_empty() {
        // Has children - wait for them
        let finally = finally_hook.map(|_| FinallyData { step_name, effective_value });
        let count = NonZeroU16::new(spawned.len() as u16).unwrap();
        self.start_waiting(task_id, count, finally);
        for child in spawned {
            self.queue_task(child, Some(task_id));
        }
    } else if let Some(hook) = finally_hook {
        // No children, has finally - run it now
        let finally_spawned = run_finally_hook_direct(&hook, &effective_value.0);
        if !finally_spawned.is_empty() {
            let count = NonZeroU16::new(finally_spawned.len() as u16).unwrap();
            self.start_waiting(task_id, count, None);  // finally already ran
            for child in finally_spawned {
                self.queue_task(child, Some(task_id));
            }
        } else {
            self.finish_in_flight_and_notify(task_id);
        }
    } else {
        // No children, no finally - done
        self.finish_in_flight_and_notify(task_id);
    }
}

fn finish_in_flight_and_notify(&mut self, task_id: LogTaskId) {
    if let Some(parent_id) = self.finish_in_flight(task_id) {
        self.decrement_parent(parent_id);
    }
}
```

#### Task retries

```rust
fn task_retried(&mut self, task_id: LogTaskId, retry_task: Task) {
    let parent_id = self.tasks.get(&task_id).expect("task must exist").parent_id;
    self.queue_task(retry_task, parent_id);
    self.finish_in_flight(task_id);  // Don't notify parent
}
```

#### Task fails permanently

```rust
fn task_failed(&mut self, task_id: LogTaskId) {
    self.finish_in_flight_and_notify(task_id);
}
```

#### Decrement parent count

```rust
fn decrement_parent(&mut self, parent_id: LogTaskId) {
    let entry = self.tasks.get_mut(&parent_id).expect("parent must exist");
    let TaskState::Waiting { pending_count, finally } = &mut entry.state else {
        panic!("parent not in Waiting state");
    };

    let new_count = pending_count.get() - 1;
    if new_count == 0 {
        // All children done
        if let Some(finally_data) = finally.take() {
            // Run finally hook
            let hook = self.config.steps.get(&finally_data.step_name)
                .and_then(|s| s.finally_hook.as_ref())
                .expect("finally_data implies finally hook exists");
            let finally_spawned = run_finally_hook_direct(hook, &finally_data.effective_value.0);

            if !finally_spawned.is_empty() {
                // Finally spawned tasks - update count, finally is now None
                *pending_count = NonZeroU16::new(finally_spawned.len() as u16).unwrap();
                for child in finally_spawned {
                    self.queue_task(child, Some(parent_id));
                }
                return;
            }
        }
        // No finally or finally spawned nothing - done
        if let Some(grandparent_id) = self.finish_waiting(parent_id) {
            self.decrement_parent(grandparent_id);
        }
    } else {
        *pending_count = NonZeroU16::new(new_count).unwrap();
    }
}
```

### Example Traces

#### Bug 1: A's finally waits for grandchildren

```
A (finally) spawns B (finally), B spawns C

Initial:
  tasks[0/A] = { parent: None, Pending(A) }

A dispatched and succeeds, spawns B:
  tasks[0/A] = { parent: None, Waiting { count: 1, finally: Some(...) } }
  tasks[1/B] = { parent: Some(0), Pending(B) }

B dispatched and succeeds, spawns C:
  tasks[0/A] = { parent: None, Waiting { count: 1, finally: Some(...) } }
  tasks[1/B] = { parent: Some(0), Waiting { count: 1, finally: Some(...) } }
  tasks[2/C] = { parent: Some(1), Pending(C) }

C dispatched and succeeds (no children, no finally):
  decrement_parent(1/B): count 1→0, finally.is_some() → run B_finally (spawns nothing)
    decrement_parent(0/A): count 1→0, finally.is_some() → run A_finally
    done
```

**Order: B_finally, A_finally ✓**

#### Bug 2: A's finally waits for B's finally-spawned tasks

```
A (finally) spawns B (finally that spawns C)

A dispatched and succeeds, spawns B:
  tasks[0/A] = { parent: None, Waiting { count: 1, finally: Some(...) } }
  tasks[1/B] = { parent: Some(0), Pending(B) }

B dispatched and succeeds (no children, has finally):
  Run B_finally → spawns C
  tasks[0/A] = { parent: None, Waiting { count: 1, finally: Some(...) } }
  tasks[1/B] = { parent: Some(0), Waiting { count: 1, finally: None } }  ← KEY!
  tasks[2/C] = { parent: Some(1), Pending(C) }

C dispatched and succeeds:
  decrement_parent(1/B): count 1→0, finally.is_none() → done
    decrement_parent(0/A): count 1→0, finally.is_some() → run A_finally
    done
```

**Order: B_finally runs, C completes, THEN A_finally ✓**

The key: when B's finally spawns C, B enters `Waiting { finally: None }`. B isn't "done" until C completes, but there's no more finally to run - it already ran.

---

## Files Changed

- `crates/gsd_config/src/runner/mod.rs`
  - Replace `queue: VecDeque<QueuedTask>` with `tasks: BTreeMap<LogTaskId, TaskEntry>`
  - Keep `in_flight: usize` counter
  - Remove `finally_tracker: FinallyTracker`
  - Rewrite dispatch/completion logic

- `crates/gsd_config/src/runner/types.rs`
  - Remove `QueuedTask` struct
  - Add `TaskEntry`, `TaskState`, `FinallyData`

- `crates/gsd_config/src/runner/finally.rs`
  - Remove `FinallyTracker` and `FinallyState`
  - Keep `run_finally_hook_direct` function

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
