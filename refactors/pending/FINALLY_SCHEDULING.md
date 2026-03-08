# Finally Scheduling Refactor

**Status:** Not started

**Prerequisites:** VALUE_AND_RETRY_MODEL (COMPLETED), FINALLY_TRACKING (COMPLETED)

**Blocks:** STATE_PERSISTENCE (partially - persistence can work without this, but finally won't be logged)

---

## Bug: Synchronous Finally Blocks Concurrency

### The Problem

Currently, `handle_completion()` calls `run_finally_hook_direct()` **synchronously**. This blocks the entire runner loop, preventing other tasks from being dispatched even when concurrency slots are available.

### Reproduction Scenario

**Setup:**
- `max_concurrency = 1`
- Task A has a finally hook
- Task B has no finally hook
- Initial tasks: `[A, B]`

**Current (buggy) temporal trace:**

```
Time   Event                              in_flight  pending   Notes
─────────────────────────────────────────────────────────────────────────
t0     runner.next() called               0          [A, B]
t1     dispatch_all_pending()             0          [A, B]
t2       dispatch(A)                      1          [B]       A starts
t3     rx.recv() blocks...                1          [B]       waiting for A
t4     A completes, recv() returns        1          [B]
t5     process_result(A)                  1          [B]
t6       task_succeeded(A)                1          [B]
t7         handle_completion(A, Some)     1          [B]
t8           run_finally_hook_direct()    1          [B]       ← BLOCKS HERE
             ...finally runs...           1          [B]       B still waiting!
t9           finally returns              1          [B]
t10        spawned.is_empty() → remove A  0          [B]       slot freed
t11    return from process_result         0          [B]
t12    runner.next() called again         0          [B]
t13    dispatch_all_pending()             0          [B]
t14      dispatch(B)                      1          []        B finally starts
t15    rx.recv() blocks...                1          []
t16    B completes                        0          []
t17    done                               0          []

Observed order: A_done, A_finally, B_done
```

**Key problem:** Between t8 and t9, the runner is blocked running the finally hook. B cannot start even though A's action is complete and the concurrency slot should be free.

### Expected (fixed) temporal trace:

```
Time   Event                              in_flight  pending   Notes
─────────────────────────────────────────────────────────────────────────
t0     runner.next() called               0          [A, B]
t1     dispatch_all_pending()             0          [A, B]
t2       dispatch(A)                      1          [B]       A starts
t3     rx.recv() blocks...                1          [B]
t4     A completes, recv() returns        1          [B]
t5     process_result(A)                  1          [B]
t6       task_succeeded(A)                1          [B]
t7         A has no children, has finally 0          [B]       in_flight decremented
t8         schedule_finally    0          [B, F]    F queued as root (A has no parent)
t9         remove A                       0          [B, F]    A gone, slot freed!
t10    return from process_result         0          [B, F]
t11    runner.next() called again         0          [B, F]
t12    dispatch_all_pending()             0          [B, F]
t13      dispatch(B)                      1          [F]       B starts immediately
t14    rx.recv() blocks...                1          [F]
t15    B completes                        0          [F]
t16    dispatch_all_pending()             0          [F]
t17      dispatch(F)                      1          []        finally starts
t18    rx.recv() blocks...                1          []
t19    F completes                        0          []        F is root, no parent to notify
t20    done                               0          []

Observed order: A_done, B_done, A_finally
```

**Key difference:** At t9, A is removed immediately after scheduling F. B can start at t13 while F waits in pending queue.

---

## Test Cases

**File:** `crates/gsd_config/tests/finally_retry_bugs.rs`

Note: `finally_should_not_block_concurrency` is already covered by existing tests (`finally_runs_too_early_on_retry`, etc.).

### Test 1: `finally_retries_on_failure`

```rust
/// Finally tasks should retry on failure.
///
/// Setup: A has finally that fails twice, succeeds on third try
/// Expected: run() succeeds (finally eventually succeeded)
/// Current (buggy): Finally failures are silently ignored, no retry
#[test]
#[should_panic(expected = "finally did not retry")]
fn finally_retries_on_failure() {
    // Config:
    // - StepA: has finally hook that fails first 2 times, succeeds 3rd time
    // - max_retries: 3
    //
    // Track finally call count via file/counter
    // Verify finally was called 3 times and run() succeeded
}
```

### Test 2: `finally_failure_propagates_after_retries_exhausted`

```rust
/// When finally exhausts retries, failure should propagate.
///
/// Setup: A has finally that always fails, max_retries = 2
/// Expected: run() returns error (finally failed after retries)
/// Current (buggy): Finally failures silently ignored, run() succeeds
#[test]
#[should_panic(expected = "finally failure not propagated")]
fn finally_failure_propagates_after_retries_exhausted() {
    // Config:
    // - StepA: has finally hook that always fails (exit 1)
    // - max_retries: 2
    //
    // Verify run() returns Err after 3 attempts (initial + 2 retries)
}
```

### Test 3: `finally_child_failure_propagates`

```rust
/// When a task spawned by finally fails, finally fails.
///
/// Setup: A has finally that spawns B, B always fails
/// Expected: run() returns error (B failed → finally failed → propagates)
/// Current (buggy): Unknown - need to verify behavior
#[test]
fn finally_child_failure_propagates() {
    // Config:
    // - StepA: has finally hook that outputs [{"kind": "Cleanup", "value": {}}]
    // - Cleanup step: agent always returns invalid response (fails)
    // - max_retries: 0 (fail immediately)
    //
    // Verify run() returns Err
}
```

---

## Implementation Details

### Data Structure Changes

#### `types.rs` - TaskKind enum (no magic strings)

Per coding standards, use enums instead of magic sentinel values like `"__finally__"`.

**Key constraint:** `TaskKind` is ONLY matched in `dispatch()` to determine how to spawn the task. All other code (retry, failure, completion, parent notification) treats tasks uniformly regardless of kind. This keeps the abstraction clean - "finally is just a task."

```rust
pub(super) struct TaskEntry {
    pub step: StepName,
    pub parent_id: Option<LogTaskId>,
    /// **"Am I a finally task?"** (this task's type)
    ///
    /// - None = Step task (run pre-hook, then action)
    /// - Some = Finally task with this script (no pre-hook, just run script)
    ///
    /// The script is looked up once when the finally is scheduled, not again at dispatch.
    ///
    /// **Not to be confused with `finally_data` in WaitingForChildren:**
    /// - `finally_script`: "Am I a finally task?" (this task's type)
    /// - `finally_data`:   "Do I have a finally hook to run after my children?" (step's config)
    ///
    /// A Step task may have finally_data=Some (if its step config has a finally hook).
    /// A Finally task always has finally_data=None (no "finally of finally").
    pub finally_script: Option<HookScript>,
    pub state: TaskState,
    pub retries_remaining: u32,
}

pub(super) enum TaskState {
    /// Task is queued, waiting to be dispatched.
    /// value: The step input value. For Step tasks, may be transformed by pre-hook.
    ///        For Finally tasks, comes from parent (already through pre-hook).
    Pending { value: StepInputValue },

    /// Task is currently executing.
    InFlight(InFlight),

    /// Task completed its action, waiting for children to complete.
    WaitingForChildren {
        pending_children_count: NonZeroU16,
        /// **"Does this step have a finally hook to run after children?"** (step's config)
        ///
        /// Hook + value to schedule finally when all children complete.
        /// - Some for Step tasks whose step config has a finally hook
        /// - None for Finally tasks (no "finally of finally")
        ///
        /// The hook is looked up once when entering this state, not again when scheduling.
        finally_data: Option<(HookScript, StepInputValue)>,
    },
}
```

**Key simplifications:**
- `finally_script: Option<HookScript>` instead of `TaskKind` enum - dispatch checks this
- Script looked up once at scheduling time, not again at dispatch
- Three uniform states with clear data ownership
- `finally_value` in WaitingForChildren replaces the old continuation pattern

**Note:** `finally_for` field is NOT needed. The `parent_id` already tells us which task the finally is for. We identify finally tasks by checking `finally_script.is_some()`.

### Code Changes

#### `mod.rs` - Completion flow changes

The key insight: when a task with finally completes, schedule its finally as a **sibling** (child of the same parent), not as making the completed task wait. The completed task is removed immediately.

**Example:** A spawns B (which has finally)
1. B completes with output value
2. Schedule F (B's finally) as child of A → A's count: 1 → 2
3. Remove B, decrement A → A's count: 2 → 1
4. F completes → decrement A → A's count: 1 → 0
5. A is done (or schedules its own finally if it has one)

Tree structure is simpler - B is removed immediately, F takes its place:
```
A (waiting for F)
└── F (finally for B)
```

Instead of the more complex:
```
A (waiting for B)
└── B (waiting for F)
    └── F
```

```rust
/// Look up the finally hook for a task's step, if any.
/// Returns None for Finally tasks (no "finally of finally").
fn lookup_finally_hook(&self, entry: &TaskEntry) -> Option<HookScript> {
    if entry.finally_script.is_some() {
        return None;
    }
    self.config.steps.iter()
        .find(|s| s.name == entry.step)
        .and_then(|s| s.finally_hook.clone())
}

/// Called when a task completes successfully.
/// Schedules finally as sibling (if any), then removes task.
fn task_succeeded(
    &mut self,
    task_id: LogTaskId,
    spawned: Vec<Task>,
    value: StepInputValue,
) {
    self.in_flight -= 1;

    let entry = self.tasks.get(&task_id).expect("task must exist");
    let step_name = entry.step.clone();
    let finally_hook = self.lookup_finally_hook(entry);

    if spawned.is_empty() {
        if let Some(hook) = finally_hook {
            self.schedule_finally(task_id, step_name, hook, value);
        }
        self.remove_and_notify_parent(task_id);
    } else {
        let entry = self.tasks.get_mut(&task_id).expect("task must exist");
        let count = NonZeroU16::new(spawned.len() as u16).unwrap();
        entry.state = TaskState::WaitingForChildren {
            pending_children_count: count,
            finally_data: finally_hook.map(|hook| (hook, value)),
        };
        for child in spawned {
            self.queue_task(child, Some(task_id));
        }
    }
}

/// Schedule a finally task as a sibling of the given task.
/// Does NOT remove task_id - caller must do that.
fn schedule_finally(
    &mut self,
    task_id: LogTaskId,
    step: StepName,
    hook: HookScript,
    value: StepInputValue,
) {
    let entry = self.tasks.get(&task_id).expect("task must exist");
    let parent_id = entry.parent_id;

    if let Some(parent_id) = parent_id {
        self.increment_pending_children(parent_id);
    }

    let id = self.next_task_id();
    let retries_remaining = self.step_map.get(&step)
        .map(|s| s.options.max_retries)
        .unwrap_or(0);

    let entry = TaskEntry {
        step,
        parent_id,
        finally_script: Some(hook),
        state: TaskState::Pending { value },
        retries_remaining,
    };
    self.tasks.insert(id, entry);
}

/// Increment a task's pending_children_count.
fn increment_pending_children(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.get_mut(&task_id).expect("task must exist");
    let TaskState::WaitingForChildren { pending_children_count, .. } = &mut entry.state else {
        panic!("task not in WaitingForChildren state");
    };
    *pending_children_count = NonZeroU16::new(pending_children_count.get() + 1).unwrap();
}

/// Queue a step task.
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>) {
    let id = self.next_task_id();
    let retries_remaining = self.step_map.get(&task.step)
        .map(|s| s.options.max_retries)
        .unwrap_or(0);

    let entry = TaskEntry {
        step: task.step,
        parent_id,
        finally_script: None,
        state: TaskState::Pending { value: task.value },
        retries_remaining,
    };
    self.tasks.insert(id, entry);
}

/// Remove task and decrement parent's count.
fn remove_and_notify_parent(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.remove(&task_id).expect("task must exist");
    if let Some(parent_id) = entry.parent_id {
        self.decrement_pending_children(parent_id);
    }
}

/// Decrement a task's pending_children_count.
/// When count hits zero: schedule finally (if any), then remove.
fn decrement_pending_children(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.get_mut(&task_id).expect("task must exist");
    let TaskState::WaitingForChildren { pending_children_count, finally_data } = &mut entry.state else {
        panic!("task not in WaitingForChildren state");
    };

    let new_count = pending_children_count.get() - 1;
    if new_count == 0 {
        let step_name = entry.step.clone();
        if let Some((hook, value)) = finally_data.take() {
            self.schedule_finally(task_id, step_name, hook, value);
        }
        self.remove_and_notify_parent(task_id);
    } else {
        *pending_children_count = NonZeroU16::new(new_count).unwrap();
    }
}
```

**Key changes:**
1. Finally is scheduled as **sibling** (child of same parent), not child of completed task
2. Completed task is removed immediately after scheduling its finally
3. Parent's count goes up (for finally) then down (for completed task) - net effect: waiting for finally instead of original task
4. `finally_data` (hook + value) only stored in WaitingForChildren if task has children AND has finally
5. `in_flight` decrement is unconditional in `task_succeeded`

#### `mod.rs` - `dispatch()` changes

**This is the ONLY place that checks `finally_script`.**

```rust
/// Dispatch pending tasks up to max concurrency.
fn dispatch_all_pending(&mut self) {
    while self.in_flight < self.max_concurrency {
        let Some((task_id, step, value)) = self.take_next_pending() else { break };
        self.dispatch(task_id, step, value);
    }
}

/// Extract the next pending task's data, transitioning it to InFlight.
fn take_next_pending(&mut self) -> Option<(LogTaskId, StepName, StepInputValue)> {
    let task_id = self.tasks.iter()
        .find_map(|(id, e)| matches!(e.state, TaskState::Pending { .. }).then_some(*id))?;

    let entry = self.tasks.get_mut(&task_id).expect("[E060] task must exist");
    let TaskState::Pending { value } = &mut entry.state else { unreachable!() };
    let value = std::mem::take(value);
    let step = entry.step.clone();

    entry.state = TaskState::InFlight(InFlight::new());
    self.in_flight += 1;

    Some((task_id, step, value))
}

/// Dispatch a task that's already been transitioned to InFlight.
fn dispatch(&mut self, task_id: LogTaskId, step: StepName, value: StepInputValue) {
    let entry = self.tasks.get(&task_id).expect("[E061] task must exist");
    let tx = self.tx.clone();

    if let Some(script) = &entry.finally_script {
        let script = script.clone();
        let input_json = serde_json::to_string(&value).expect("[E062] input serializes");
        info!(task_id = ?task_id, step = %step, "dispatching finally task");
        thread::spawn(move || {
            let result = run_shell_command(script.as_str(), &input_json, None);
            // ... send result back via tx
        });
    } else {
        let step_config = self.step_map.get(&step).expect("[E063] step must exist");
        // ... spawn thread, submit to pool or run command
    }
}
```

#### Finally uses existing task handling

Outside of `dispatch()`, all task handling is kind-agnostic:
- Retry: uses existing `task_failed()`
- Failure: propagates to parent via existing mechanism
- Result: uses `SubmitResult::Command`
- Spawned tasks: become children of finally task

The only finally-specific code is the `if let Some(script)` branch in `dispatch()`.

---

## Task Tree Structure

### Terminology

- **Vertical parent**: The task's `parent_id` - up the tree
- **Horizontal parent**: The task whose finally this is (semantic relationship)

A finally task's `parent_id` is the **vertical parent** (its horizontal parent's parent), NOT the horizontal parent itself.

### Before (current, buggy)

When A spawns B (which has finally):
```
A spawns B
B completes
  → run_finally_hook_direct() SYNC  ← BLOCKS
  → A notified

Tree during finally execution:
A (WaitingForChildren: B)
└── B (running finally synchronously, blocking everything)
```

### After (fixed)

When A spawns B (which has finally):
```
A spawns B
  → A is WaitingForChildren{1}

B completes
  → Schedule F (B's finally) as child of A  → A is WaitingForChildren{2}
  → Remove B, decrement A                   → A is WaitingForChildren{1}

F dispatched (asynchronously!)
  → runs finally script

F completes
  → decrement A                             → A is WaitingForChildren{0}
  → A removed (or schedules its own finally)

Tree after B completes:
A (WaitingForChildren: F)
└── F (finally for B, parent_id = A)
```

**Key insight:** F's `parent_id` is A (vertical parent), not B (horizontal parent). B is removed immediately. F is a sibling that takes B's place in A's child count.

### Finally that spawns tasks

When B's finally spawns cleanup task C:
```
A spawns B (B has finally that will spawn C)

B completes → F scheduled under A, B removed
A (WaitingForChildren{1})
└── F

F runs, spawns C
A (WaitingForChildren{1})
└── F (WaitingForChildren{1})
    └── C

C completes → F done → A done
```

Tasks spawned by finally (C) are children of the finally task (F), not siblings.

---

## State Persistence Integration

### TaskSubmitted for finally tasks

```json
{"kind":"TaskSubmitted","task_id":4,"step":"__finally__","value":{"script":"./cleanup.sh","input":{...}},"parent_id":1,"finally_for":1}
```

### Resume logic

On resume, detect tasks needing finally:
1. Find all TaskCompleted entries
2. For each, check if step has finally hook (from config)
3. Check if finally task exists (TaskSubmitted with `finally_for: Some(task_id)`)
4. If not, and all descendants done, queue finally task

```rust
fn detect_missing_finally_tasks(
    config: &Config,
    completed: &HashSet<LogTaskId>,
    finally_submitted: &HashMap<LogTaskId, LogTaskId>,  // finally_for → task_id
) -> Vec<(LogTaskId, HookScript)> {
    let mut missing = vec![];

    for task_id in completed {
        // Check if this task's step has a finally hook
        let step = get_step_for_task(task_id);  // need to track this
        if let Some(hook) = &step.finally_hook {
            // Check if finally was already submitted
            if !finally_submitted.contains_key(task_id) {
                missing.push((*task_id, hook.clone()));
            }
        }
    }

    // Sort by depth (deepest first) to run in correct order
    missing.sort_by_key(|(id, _)| depth_of(*id));
    missing
}
```

---

## Edge Cases

### 1. Finally task times out

Currently not possible (shell commands don't have timeouts). After this change, finally tasks could have timeouts if we add that feature. For now, finally tasks run without timeout.

### 2. Finally task fails

**Current behavior:** failure is silently ignored, parent still completes.
**New behavior:** finally retries (default 3 attempts). If all retries exhausted, failure propagates - parent is notified that finally failed.

### 3. Crash during finally task

**Before:** finally runs synchronously, crash = no record, finally might re-run on resume.
**After:** finally is a task, TaskSubmitted logged. On resume, we see finally was submitted but not completed, so we re-run it (respecting remaining retries).

### 4. Finally spawns tasks that fail

Tasks spawned by finally are children of the finally task. If they fail, finally's pending_count decrements. When finally completes (success or failure after retries), parent is notified.

### 5. Retry of finally task

Finally retry works like any task retry:
- Retry gets same `parent_id` as failed finally
- Failed finally removed from map
- Retry takes its place
- Parent's pending_count unchanged (still waiting for 1 task)

---

## Temporal Steps (Stacked Branches)

Each step is a separate branch stacked on the previous. All must pass CI before merging to master.

### Branch 1: `finally/01-tests`
**Base:** `master`

Add all tests with `#[should_panic]`. One commit, tests document the bugs.

```
- finally_retries_on_failure
- finally_failure_propagates_after_retries_exhausted
- finally_child_failure_propagates
```

**Files:** `tests/finally_retry_bugs.rs`

**Note:** Commit with `--no-verify` since tests skip in sandbox (no IPC). CI will run them.

---

### Branch 2: `finally/02-dispatch-refactor` (Pre-factor)
**Base:** `finally/01-tests`

Refactor dispatch into `take_next_pending()` + `dispatch()`. Pure cleanup, no behavior change.

**Current:** `dispatch()` does: find pending → extract value → spawn thread → transition to InFlight

**After:**
- `take_next_pending()`: find pending → extract value → transition to InFlight → return data
- `dispatch()`: spawn thread with provided data

This is independent of the finally changes and makes the later refactor cleaner.

**Files:** `runner/mod.rs`

---

### Branch 3: `finally/03-data-structures`
**Base:** `finally/02-dispatch-refactor`

Change `TaskEntry` and `TaskState`. Compilation will break until construction sites are updated.

Changes:
- Add `finally_script: Option<HookScript>` to `TaskEntry`
- Change `TaskState::Pending` to hold `value: StepInputValue`
- Change `TaskState::Waiting` to `WaitingForChildren { pending_children_count, finally_data }`
- Update all `TaskEntry` construction sites to compile

**Files:** `runner/types.rs`, `runner/mod.rs`

---

### Branch 4: `finally/04-completion-flow`
**Base:** `finally/03-data-structures`

The main logic change. Restructure completion to schedule finally as sibling.

New functions:
- `lookup_finally_hook()` - check if step has finally hook
- `schedule_finally()` - create finally task as sibling, increment parent count
- `increment_pending_children()` / `decrement_pending_children()` - symmetric count operations
- `remove_and_notify_parent()` - remove task and decrement parent

Modified:
- `task_succeeded()` - look up hook once, schedule or store `finally_data`
- `dispatch()` - check `finally_script` to determine how to spawn

Removed:
- `run_finally_hook_direct()` - no longer needed

**Files:** `runner/mod.rs`, `runner/finally.rs`, `runner/dispatch.rs`

---

### Branch 5: `finally/05-tests-pass`
**Base:** `finally/04-completion-flow`

Remove `#[should_panic]` from tests. They should now pass.

**Files:** `tests/finally_retry_bugs.rs`

---

## Merge Strategy

```bash
# After all branches pass CI:
git checkout master
git merge finally/05-tests-pass
git push

# Move doc to past/
mv refactors/pending/FINALLY_SCHEDULING.md refactors/past/
```

---

## Files Changed Summary

| File | Branch | Changes |
|------|--------|---------|
| `tests/finally_retry_bugs.rs` | 01, 05 | Add 3 tests (01), remove should_panic (05) |
| `runner/mod.rs` | 02, 03, 04 | Dispatch refactor (02), data structures (03), completion flow (04) |
| `runner/types.rs` | 03 | TaskEntry + TaskState changes |
| `runner/finally.rs` | 04 | Remove `run_finally_hook_direct()` |
| `runner/dispatch.rs` | 04 | Check `finally_script` in dispatch |
