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
t7         A has no children              1          [B]
t8         children_done(A)               1          [B]       check for finally
t9           has finally → schedule F     1          [B, F]    F queued
t10          A → Waiting{count:1}         0          [B, F]    slot freed!
t11    return from process_result         0          [B, F]
t12    runner.next() called again         0          [B, F]
t13    dispatch_all_pending()             0          [B, F]
t14      dispatch(B)                      1          [F]       B starts immediately
t15    rx.recv() blocks...                1          [F]
t16    B completes                        0          [F]
t17    dispatch_all_pending()             0          [F]
t18      dispatch(F)                      1          []        finally starts
t19    rx.recv() blocks...                1          []
t20    F completes                        1          []
t21      decrement_parent(A)              0          []
t22      children_done(A)                 0          []        effective_value=None (consumed)
t23      handle_completion(A)             0          []        A removed
t24    done                               0          []

Observed order: A_done, B_done, A_finally
```

**Key difference:** At t10, instead of blocking, we schedule F and free the concurrency slot. B can start at t14 while A waits for F.

---

## Test Cases

### Test 1: `finally_should_not_block_concurrency`

**File:** `crates/gsd_config/tests/finally_retry_bugs.rs`

```rust
/// Bug: Synchronous finally blocks other tasks from starting.
///
/// Setup: max_concurrency=1, tasks [A, B] where A has finally
/// Expected order: A_done, B_done, A_finally (B starts while A waits for finally)
/// Actual (buggy): A_done, A_finally, B_done (finally blocks B)
#[test]
#[should_panic(expected = "wrong order")]
fn finally_should_not_block_concurrency() {
    // Config:
    // - max_concurrency: 1
    // - StepA: action completes, has finally hook that records "A_finally"
    // - StepB: action completes, records "B_done", no finally
    //
    // Initial tasks: [A, B]
    //
    // We use the mock pool's ability to control completion order.
    // Both A and B complete their actions quickly.
    // The finally hook also completes quickly.
    // The question is: does B start before or after A's finally runs?
}
```

### Test 2: `finally_retries_on_failure`

```rust
/// Finally tasks should retry on failure.
///
/// Setup: A has finally that fails twice, succeeds on third try
/// Expected: A completes successfully after finally succeeds
/// Current (buggy): Finally failures are silently ignored, no retry
#[test]
#[should_panic(expected = "finally did not retry")]
fn finally_retries_on_failure() {
    // Config:
    // - StepA: has finally hook that fails first 2 times, succeeds 3rd time
    // - retries: 3 for finally
    //
    // Verify that finally is retried and eventually succeeds
}
```

### Test 3: `finally_failure_propagates_after_retries_exhausted`

```rust
/// When finally exhausts retries, failure should propagate to parent.
///
/// Setup: A has finally that always fails, retries = 2
/// Expected: A's parent is notified of failure (or A is marked failed)
/// Current (buggy): Finally failures silently ignored
#[test]
#[should_panic(expected = "finally failure not propagated")]
fn finally_failure_propagates_after_retries_exhausted() {
    // Config:
    // - StepA: has finally hook that always fails
    // - retries: 2
    //
    // Verify that after 3 attempts (initial + 2 retries), failure propagates
}
```

---

## Implementation Details

### Data Structure Changes

#### `types.rs` - TaskKind enum (no magic strings)

Per coding standards, use enums instead of magic sentinel values like `"__finally__"`.

**Key constraint:** `TaskKind` is ONLY matched in `dispatch()` to determine how to spawn the task. All other code (retry, failure, completion, parent notification) treats tasks uniformly regardless of kind. This keeps the abstraction clean - "finally is just a task."

```rust
/// What kind of task this is - determines dispatch behavior.
/// ONLY matched in dispatch(). All other task handling is kind-agnostic.
pub enum TaskKind {
    /// Regular step task from config
    Step(Task),
    /// Finally task - runs when parent's children complete
    Finally {
        /// The step whose finally hook this runs (used to look up the script)
        step: StepName,
        /// Input value passed to the finally hook
        input: serde_json::Value,
    },
}

impl TaskKind {
    /// For logging - describe what this task is
    pub fn description(&self) -> String {
        match self {
            TaskKind::Step(task) => format!("step {}", task.step),
            TaskKind::Finally { step, .. } => format!("finally for {}", step),
        }
    }
}
```

**Note:** `StepName` will eventually be interned as a `u32`, making this very efficient. The actual `HookScript` is looked up from `config.steps[step].finally_hook` at dispatch time.

#### `types.rs` - TaskEntry changes

```rust
pub(super) struct TaskEntry {
    pub parent_id: Option<LogTaskId>,
    pub state: TaskState,
    pub kind: TaskKind,           // NEW: replaces Task in Pending state
    pub retries_remaining: u32,   // NEW: for retry logic (finally tasks retry too)
    pub effective_value: Option<EffectiveValue>,  // NEW: set when task succeeds (for finally input)
}

impl TaskEntry {
    /// Get the step name from the task kind.
    pub fn step_name(&self) -> &StepName {
        match &self.kind {
            TaskKind::Step(task) => &task.step,
            TaskKind::Finally { step, .. } => step,
        }
    }
}

// TaskState::Pending no longer holds Task - it's in TaskEntry.kind
// TaskState::Waiting no longer holds continuation - finally is scheduled when count hits 0
pub(super) enum TaskState {
    Pending,                      // CHANGED: no longer Pending(Task)
    InFlight(InFlight),
    Waiting {
        pending_count: NonZeroU16,  // Just the count - no continuation
    },
}
```

**Key simplification:** `Waiting` just holds `pending_count`. When count hits zero, we look up whether the step has a finally hook (using `entry.step_name()` derived from `TaskKind`) and schedule it then. The `effective_value` is stored on TaskEntry when the task succeeds, ready to pass to finally.

**Note:** `finally_for` field is NOT needed. The `parent_id` already tells us which task the finally is for. We can identify finally tasks by matching on `TaskKind::Finally`.

### Code Changes

#### `mod.rs` - Completion flow changes

The key insight: `Waiting` no longer stores a continuation. When `pending_count` hits zero, we check if the step has a finally hook and schedule it at that moment.

```rust
// BEFORE: handle_completion took continuation parameter, ran finally synchronously
// AFTER: handle_completion just removes the task and notifies parent

/// Called when a task is fully done (no more children, no finally to run).
/// Removes task from map and notifies parent.
fn handle_completion(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.remove(&task_id).expect("task must exist");
    if let Some(parent_id) = entry.parent_id {
        self.decrement_parent(parent_id);
    }
}

/// Called when a task's pending_count hits zero.
/// Checks if step has finally hook AND we haven't scheduled it yet.
/// Uses effective_value as the marker: Some = not yet scheduled, None = already scheduled.
fn children_done(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.get_mut(&task_id).expect("task must exist");

    // .take() atomically checks and clears effective_value
    // If Some, we haven't scheduled finally yet; if None, we have (or task never succeeded)
    if let Some(effective_value) = entry.effective_value.take() {
        let step_name = entry.step_name().clone();  // Derived from TaskKind

        // Check if this step has a finally hook
        let has_finally = self.config.steps.iter()
            .find(|s| s.name == step_name)
            .and_then(|s| s.finally_hook.as_ref())
            .is_some();

        if has_finally {
            // Schedule finally as child task
            self.schedule_finally(task_id, step_name, effective_value);
            return;
        }
    }

    // No finally (or already scheduled) - task is done
    self.handle_completion(task_id);
}

/// Schedule a finally task as child of parent_id.
fn schedule_finally(
    &mut self,
    parent_id: LogTaskId,
    step: StepName,
    effective_value: EffectiveValue,
) {
    let id = self.next_task_id();
    // step is stored in TaskKind::Finally, accessible via entry.step_name()
    let kind = TaskKind::Finally { step: step.clone(), input: effective_value.0 };

    // Get retry count from step config
    let retries_remaining = self.step_map.get(&step)
        .map(|s| s.options.max_retries)
        .unwrap_or(0);

    let entry = TaskEntry {
        parent_id: Some(parent_id),
        state: TaskState::Pending,
        kind,  // step_name derived from kind.step
        retries_remaining,
        effective_value: None,  // Finally tasks don't have their own effective_value
    };

    // Update parent to wait for this finally task
    let parent = self.tasks.get_mut(&parent_id).expect("parent must exist");
    parent.state = TaskState::Waiting {
        pending_count: NonZeroU16::new(1).unwrap(),
    };

    let prev = self.tasks.insert(id, entry);
    assert!(prev.is_none());
}

/// Decrement parent's pending_count. If hits zero, call children_done.
fn decrement_parent(&mut self, parent_id: LogTaskId) {
    let entry = self.tasks.get_mut(&parent_id).expect("parent must exist");
    let TaskState::Waiting { pending_count } = &mut entry.state else {
        panic!("parent not in Waiting state");
    };

    let new_count = pending_count.get() - 1;
    if new_count == 0 {
        self.children_done(parent_id);
    } else {
        *pending_count = NonZeroU16::new(new_count).unwrap();
    }
}
```

```rust
/// Called when task action completes successfully.
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, effective_value: EffectiveValue) {
    // Store effective_value for later finally scheduling
    let entry = self.tasks.get_mut(&task_id).expect("task must exist");
    entry.effective_value = Some(effective_value);

    self.in_flight -= 1;  // Unconditional - we know task was InFlight

    if spawned.is_empty() {
        // No children - check for finally immediately
        self.children_done(task_id);
    } else {
        // Has children - wait for them
        let count = NonZeroU16::new(spawned.len() as u16).unwrap();
        entry.state = TaskState::Waiting { pending_count: count };
        for child in spawned {
            self.queue_task(child, Some(task_id));
        }
    }
}
```

**Key changes:**
1. `handle_completion` no longer takes `continuation` - it just removes the task
2. `task_succeeded` stores `effective_value` on TaskEntry for later use
3. New `children_done` function: when count hits 0, checks for finally (via `effective_value`) and schedules it
4. `schedule_finally` creates finally task and updates parent to wait for it
5. `in_flight` decrement is unconditional at call sites that know the task state

#### `mod.rs` - `dispatch()` changes

**This is the ONLY place that matches on `TaskKind`.** The key invariant: `InFlight::new()` is only called immediately after spawning the thread - creating the marker proves dispatch happened.

```rust
/// Dispatch a pending task. Called from dispatch_all_pending().
/// Precondition: task_id exists in self.tasks with state Pending.
fn dispatch(&mut self, task_id: LogTaskId) {
    let entry = self.tasks.get_mut(&task_id).expect("task must exist");
    let TaskState::Pending = &entry.state else {
        panic!("dispatch called on non-Pending task");
    };

    let tx = self.tx.clone();

    match &entry.kind {
        TaskKind::Step(task) => {
            // Existing dispatch logic for regular tasks
            let step = self.step_map.get(&task.step).expect("step must exist");
            // ... spawn thread, submit to pool or run command
        }
        TaskKind::Finally { step, input } => {
            // Finally task - look up script from config, run as shell command
            let script = self.config.steps.iter()
                .find(|s| &s.name == step)
                .and_then(|s| s.finally_hook.as_ref())
                .expect("finally task must have corresponding hook in config")
                .clone();

            let input_json = serde_json::to_string(input).expect("input serializes");
            let working_dir = self.pool.working_dir.clone();

            info!(task_id = ?task_id, step = %step, "dispatching finally task");

            thread::spawn(move || {
                let result = run_shell_command(script.as_str(), &input_json, None);
                // ... send result back via tx
            });
        }
    }

    // InFlight::new() immediately after spawn - creating marker proves dispatch
    entry.state = TaskState::InFlight(InFlight::new());
    self.in_flight += 1;
}

/// Dispatch pending tasks up to max concurrency. Single place for concurrency check.
fn dispatch_all_pending(&mut self) {
    while self.in_flight < self.max_concurrency {
        let Some(task_id) = self.tasks.iter()
            .find_map(|(id, e)| matches!(e.state, TaskState::Pending).then_some(*id))
        else { break };

        self.dispatch(task_id);
    }
}
```

#### Finally uses existing task handling

**No special handling needed.** Outside of `dispatch()`, `TaskKind` is never matched. All task handling is kind-agnostic:
- Retry: uses existing `task_failed()` - no special case for finally
- Failure: uses existing failure propagation to parent - no special case for finally
- Result: uses `SubmitResult::Command` (it's a shell command) - same as any command task
- Spawned tasks: parsed from stdout as `Vec<Task>`, become children of finally task - standard behavior

The only finally-specific code is the match arm in `dispatch()` that looks up the script from config.

---

## Task Tree Structure

### Before (current)

When A has finally that spawns C:
```
A completes
  → run_finally_hook_direct() SYNC
  → returns [C]
  → C queued as child of A

Tree:
A (Waiting for C)
└── C
```

### After (fixed)

When A has finally that spawns C:
```
A completes
  → queue finally task F as child of A
  → A waits for F

F dispatched
  → runs finally script
  → returns [C]
  → C queued as child of F
  → F waits for C

C completes
  → F.pending_count → 0 → F done
  → A.pending_count → 0 → A done

Tree:
A (Waiting for F)
└── F (finally task, Waiting for C)
    └── C
```

This is cleaner - finally is just another task in the tree.

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

## Implementation Checklist

### Phase 1: Add tests documenting bugs (one commit)

- [ ] `finally_should_not_block_concurrency` - scheduling bug
- [ ] `finally_retries_on_failure` - no retry bug
- [ ] `finally_failure_propagates_after_retries_exhausted` - silent failure bug
- [ ] All tests `#[should_panic]` with current implementation
- [ ] Commit with `--no-verify` (tests skip in sandbox)

### Phase 2: Data structure changes

- [ ] Add `TaskKind` enum with `Step(Task)` and `Finally { step, input }` variants
- [ ] Add `TaskEntry::step_name()` method to derive step from `TaskKind`
- [ ] Add `retries_remaining: u32` to `TaskEntry`
- [ ] Add `effective_value: Option<EffectiveValue>` to `TaskEntry`
- [ ] Change `TaskState::Pending` to not hold `Task` (task data now in `TaskEntry.kind`)
- [ ] Change `TaskState::Waiting` to only hold `pending_count` (remove `continuation`)
- [ ] Remove `Continuation` type entirely
- [ ] Update all `TaskEntry` construction sites

### Phase 3: Restructure completion flow

- [ ] `task_succeeded`: store `effective_value`, unconditionally decrement `in_flight`
- [ ] New `children_done`: check `effective_value` for finally scheduling
- [ ] New `schedule_finally`: create finally task, update parent to Waiting
- [ ] Simplify `handle_completion`: just remove task and notify parent
- [ ] Modify `dispatch()` to match on `TaskKind`
- [ ] Add dispatch logic for `TaskKind::Finally`

### Phase 4: Finally retry and failure handling

- [ ] Finally tasks use existing retry logic (via `retries_remaining`)
- [ ] Finally failure propagates to parent via existing `task_failed()`
- [ ] Spawned tasks from finally become children of finally task (standard behavior)

### Phase 5: Verify and clean up

- [ ] Remove `#[should_panic]` from all tests
- [ ] Verify all existing tests pass
- [ ] Update STATE_PERSISTENCE.md to note finally is now loggable

---

## Files Changed Summary

| File | Changes |
|------|---------|
| `runner/types.rs` | Add `TaskKind` enum, add `step_name()` method and `effective_value`/`retries_remaining` fields to `TaskEntry`, simplify `Waiting` (remove `continuation`), remove `Continuation` type |
| `runner/mod.rs` | Add `children_done()`, `schedule_finally()`, simplify `handle_completion()`, modify `task_succeeded()` to store `effective_value`, modify `dispatch()` to match on `TaskKind` |
| `runner/response.rs` | Handle finally task results |
| `runner/finally.rs` | Remove `run_finally_hook_direct()` (no longer needed) |
| `tests/finally_retry_bugs.rs` | Add 3 new tests for finally scheduling bugs |
