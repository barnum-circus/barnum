# Finally Tracking Refactor

**Status:** Completed (2026-03-08)

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

/// Zero-sized proof that a task was dispatched to a worker.
/// Can only be created by submitting to the worker channel, enforcing
/// that InFlight state means the task is actually running.
struct InFlight(());

impl InFlight {
    fn new(tx: &mpsc::Sender<WorkerTask>, task_id: LogTaskId, task: Task) -> Self {
        tx.send(WorkerTask { task_id, task }).expect("worker channel closed");
        InFlight(())
    }
}

enum TaskState {
    /// Task waiting to be dispatched
    Pending(Task),
    /// Task currently executing - ZST proves dispatch happened
    InFlight(InFlight),
    /// Task succeeded, waiting for children to complete
    Waiting {
        pending_count: NonZeroU16,
        continuation: Option<Continuation>,
    },
}

/// What to run when all children complete. The task tree doesn't know what
/// this does - it just runs it and queues any spawned tasks as children.
/// In GSD, this is a finally hook, but could be anything.
struct Continuation {
    step_name: StepName,      // Used to look up what to run
    value: EffectiveValue,    // Passed to it
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
Task created → [Pending] → [InFlight] ──┬── success with children ──→ [Waiting { continuation: Some/None }]
                    ▲                   │                                        │
                    │                   ├── success, no children, has continuation ──→│ (run continuation immediately)
                    │                   │    └── continuation spawns ────────────────→│ [Waiting { continuation: None }]
                    │                   │    └── continuation spawns nothing ─────────┼──→ done
                    │                   │                                        │
                    │                   ├── success, no children, no continuation ────┼──→ done
                    │                   ├── retry ──────────────────────────────→│ (new Pending, same parent)
                    │                   └── dropped ────────────────────────────→│ done (notify parent)
                    │                                                            │
                    │                                                            ▼ count hits 0
                    │                   ┌── continuation.is_some() ──→ run continuation ───┤
                    │                   │    └── spawns tasks ──→ [Waiting { continuation: None }]
                    │                   │    └── spawns nothing ─────────────────┼──→ done
                    └───────────────────┴── continuation.is_none() ───────────────────┴──→ done
```

### State Transitions

```rust
/// Create InFlight task - ZST constructor enforces dispatch
fn dispatch(&mut self, task_id: LogTaskId, task: Task, parent_id: Option<LogTaskId>) {
    let in_flight = InFlight::new(&self.tx, task_id, task);
    let prev = self.tasks.insert(task_id, TaskEntry {
        parent_id,
        state: TaskState::InFlight(in_flight),
    });
    assert!(prev.is_none(), "task_id collision: {task_id:?} already in map");
    self.in_flight += 1;
}

/// Pending → InFlight (for tasks that were queued while at max concurrency)
fn dispatch_pending(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.remove(&task_id).expect("task must exist");
    let TaskState::Pending(task) = entry.state else {
        panic!("dispatch_pending called on non-Pending task");
    };
    self.dispatch(task_id, task, entry.parent_id);  // Reuse dispatch()
}

/// InFlight → Waiting
fn transition_to_waiting(
    &mut self,
    task_id: LogTaskId,
    pending_count: NonZeroU16,
    continuation: Option<Continuation>,
) {
    let entry = self.tasks.get_mut(&task_id).expect("task must exist");
    assert!(matches!(entry.state, TaskState::InFlight(_)));
    entry.state = TaskState::Waiting { pending_count, continuation };
    self.in_flight -= 1;
}

/// InFlight → removed
fn transition_to_done(&mut self, task_id: LogTaskId) -> Option<LogTaskId> {
    let entry = self.tasks.remove(&task_id).expect("task must exist");
    assert!(matches!(entry.state, TaskState::InFlight(_)));
    self.in_flight -= 1;
    entry.parent_id
}

/// Add a new task - dispatch immediately if under concurrency, otherwise queue as Pending
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>) {
    let id = self.next_task_id();

    if self.in_flight < self.max_concurrency {
        self.dispatch(id, task, parent_id);  // Atomic: create InFlight + send to worker
    } else {
        let prev = self.tasks.insert(id, TaskEntry {
            parent_id,
            state: TaskState::Pending(task),
        });
        assert!(prev.is_none(), "task_id collision: {id:?} already in map");
    }
}
```

Note: `dispatch` is atomic - creating InFlight state and submitting to worker happen together. You can't have one without the other.

### Key Operations

#### Dispatch pending tasks

Only needed for initial bootstrap or when tasks were queued while at max concurrency.

```rust
fn dispatch_all_pending(&mut self) {
    while self.in_flight < self.max_concurrency {
        let Some(task_id) = self.tasks.iter()
            .find_map(|(id, entry)| matches!(entry.state, TaskState::Pending(_)).then_some(*id))
        else {
            break;
        };
        self.dispatch_pending(task_id);  // Reuse single-task dispatch
    }
}
```

#### Task succeeds

```rust
fn task_succeeded(&mut self, task_id: LogTaskId, step_name: StepName, spawned: Vec<Task>, effective_value: EffectiveValue) {
    let finally_hook = self.config.steps.get(&step_name).and_then(|s| s.finally_hook.clone());
    let continuation = finally_hook.map(|_| Continuation { step_name: step_name.clone(), value: effective_value });

    if !spawned.is_empty() {
        // Has children - wait for them
        let count = NonZeroU16::new(spawned.len() as u16).unwrap();
        self.transition_to_waiting(task_id, count, continuation);
        for child in spawned {
            self.queue_task(child, Some(task_id));
        }
    } else {
        // No children - handle completion (may run continuation)
        self.handle_completion(task_id, continuation);
    }
}
```

#### Task fails (with optional retry)

```rust
fn task_failed(&mut self, task_id: LogTaskId, retry: Option<Task>) {
    let parent_id = self.tasks.get(&task_id).expect("task must exist").parent_id;

    if let Some(retry_task) = retry {
        self.queue_task(retry_task, parent_id);
        self.transition_to_done(task_id);  // Don't notify - retry takes over
    } else {
        // Permanent failure - remove and notify parent
        let entry = self.tasks.remove(&task_id).expect("task must exist");
        if matches!(entry.state, TaskState::InFlight(_)) {
            self.in_flight -= 1;
        }
        if let Some(parent_id) = entry.parent_id {
            self.decrement_parent(parent_id);
        }
    }
}
```

#### Decrement parent count

```rust
fn decrement_parent(&mut self, parent_id: LogTaskId) {
    let (hit_zero, continuation) = {
        let entry = self.tasks.get_mut(&parent_id).expect("parent must exist");
        let TaskState::Waiting { pending_count, continuation } = &mut entry.state else {
            panic!("parent not in Waiting state");
        };

        let new_count = pending_count.get() - 1;
        if new_count == 0 {
            (true, continuation.take())
        } else {
            *pending_count = NonZeroU16::new(new_count).unwrap();
            (false, None)
        }
    };

    if hit_zero {
        self.handle_completion(parent_id, continuation);
    }
}
```

#### Handle completion (shared logic)

```rust
/// Called when a task may be ready to finish. Runs continuation if present;
/// if that spawns tasks, waits for them. Otherwise removes task and notifies parent.
/// Task may be InFlight (success with no children) or Waiting (count hit 0).
fn handle_completion(&mut self, task_id: LogTaskId, continuation: Option<Continuation>) {
    let spawned = if let Some(cont) = continuation {
        let hook = self.config.steps.get(&cont.step_name)
            .and_then(|s| s.finally_hook.as_ref())
            .expect("continuation implies finally hook exists");
        run_finally_hook_direct(hook, &cont.value.0)
    } else {
        vec![]
    };

    if !spawned.is_empty() {
        let count = NonZeroU16::new(spawned.len() as u16).unwrap();

        // Transition to or update Waiting state
        let entry = self.tasks.get_mut(&task_id).expect("task must exist");
        if matches!(entry.state, TaskState::InFlight(_)) {
            self.in_flight -= 1;
        }
        entry.state = TaskState::Waiting { pending_count: count, continuation: None };

        for child in spawned {
            self.queue_task(child, Some(task_id));
        }
    } else {
        // Remove and notify parent
        let entry = self.tasks.remove(&task_id).expect("task must exist");
        if matches!(entry.state, TaskState::InFlight(_)) {
            self.in_flight -= 1;
        }
        if let Some(parent_id) = entry.parent_id {
            self.decrement_parent(parent_id);
        }
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
  tasks[0/A] = { parent: None, Waiting { count: 1, continuation: Some(...) } }
  tasks[1/B] = { parent: Some(0), Pending(B) }

B dispatched and succeeds, spawns C:
  tasks[0/A] = { parent: None, Waiting { count: 1, continuation: Some(...) } }
  tasks[1/B] = { parent: Some(0), Waiting { count: 1, continuation: Some(...) } }
  tasks[2/C] = { parent: Some(1), Pending(C) }

C dispatched and succeeds (no children, no finally):
  decrement_parent(1/B): count 1→0, continuation.is_some() → run B_finally (spawns nothing)
    decrement_parent(0/A): count 1→0, continuation.is_some() → run A_finally
    done
```

**Order: B_finally, A_finally ✓**

#### Bug 2: A's finally waits for B's finally-spawned tasks

```
A (finally) spawns B (finally that spawns C)

A dispatched and succeeds, spawns B:
  tasks[0/A] = { parent: None, Waiting { count: 1, continuation: Some(...) } }
  tasks[1/B] = { parent: Some(0), Pending(B) }

B dispatched and succeeds (no children, has finally):
  Run B_finally → spawns C
  tasks[0/A] = { parent: None, Waiting { count: 1, continuation: Some(...) } }
  tasks[1/B] = { parent: Some(0), Waiting { count: 1, continuation: None } }  ← KEY!
  tasks[2/C] = { parent: Some(1), Pending(C) }

C dispatched and succeeds:
  decrement_parent(1/B): count 1→0, continuation.is_none() → done
    decrement_parent(0/A): count 1→0, continuation.is_some() → run A_finally
    done
```

**Order: B_finally runs, C completes, THEN A_finally ✓**

The key: when B's finally spawns C, B enters `Waiting { continuation: None }`. B isn't "done" until C completes, but there's no more continuation to run - it already ran.

---

## Files Changed

- `crates/gsd_config/src/runner/mod.rs`
  - Replace `queue: VecDeque<QueuedTask>` with `tasks: BTreeMap<LogTaskId, TaskEntry>`
  - Keep `in_flight: usize` counter
  - Remove `finally_tracker: FinallyTracker`
  - Rewrite dispatch/completion logic

- `crates/gsd_config/src/runner/types.rs`
  - Remove `QueuedTask` struct
  - Add `TaskEntry`, `TaskState`, `Continuation`

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

2. **Retry with finally** - Task with finally that retries then succeeds
   - Verify finally runs only once after final success
   - Verify parent waits for retry to complete

3. **Multiple children with finally** - A spawns B and C, both with finally
   - Verify A waits for both B_finally and C_finally before A_finally

4. **Finally spawns multiple tasks** - A's finally spawns B and C
   - Verify parent (if any) waits for all finally-spawned tasks

5. **Child fails, parent finally still runs** - A (with finally) spawns B, B fails
   - Verify A's finally runs (child failure counts as "done")
   - Verify B's finally does NOT run (B never succeeded)

6. **Task drops after max retries, no finally** - Task with finally exceeds retry limit
   - Verify finally does NOT run (task never succeeded)
   - Verify parent is still notified (child is "done")

### Test Execution Notes

Tests in `finally_retry_bugs.rs` require IPC (agent pool). They skip in the sandbox with "SKIP: IPC not available". To run them:

```bash
# Via command pool (outside sandbox):
./target/debug/agent_pool submit_task --pool cmd --notify file --data \
  '{"kind": "Task", "task": {"instructions": "Run tests", "data": {"cmd": "cargo test -p gsd_config --test finally_retry_bugs 2>&1"}}}'
```

When adding tests with `#[should_panic]` to document bugs, commit with `--no-verify` (pre-commit hook fails because the test skips in sandbox and doesn't panic).
