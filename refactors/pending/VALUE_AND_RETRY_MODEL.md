# Value and Retry Model Cleanup

**Status:** Ready for implementation
**Priority:** Do BEFORE `FINALLY_TRACKING`
**Blocks:** `FINALLY_TRACKING`, `FINALLY_SCHEDULING`

## Mental Model

### Task Lifecycle

A task goes through this lifecycle:

```
queued → running → success (spawns children)
                 → failure → retry (new execution of same logical task)
                          → dropped (max retries exceeded)
```

Key insights:
- **A retry is a new execution** - it gets a fresh `LogTaskId`
- **A retry's parent is the failed execution** - this creates an execution chain
- **Finally hooks wait for eventual success** - not just immediate completion

### Spawned Children vs Retries

| Spawned Children | Retries |
|-----------------|---------|
| New tasks returned by successful agent | New execution after failure |
| Parent = the task that spawned them | Parent = the failed execution |
| Are "descendants" for finally tracking | NOT descendants - part of retry chain |
| Success/failure affects parent's finally | Parent waits for eventual success |

### Finally Hook Semantics

Finally hook runs when:
1. A task **succeeds** (not just completes)
2. AND all its **spawned children** succeed (recursively)

Finally hook does NOT care about:
- How many retries it took to succeed
- Failed attempts that were retried

### Value Types

- `task.value` - the original value on the queued task
- `EffectiveValue` - value after pre-hook transformation (or original if no pre-hook)

The `EffectiveValue` is what gets sent to the agent/command. If pre-hook fails, there IS no effective value because the task never ran.

---

## The Bug

### Current Behavior (WRONG)

When a task fails and retries, `notify_origin_of_completion` is called immediately. This causes the parent's finally hook to run too early.

**Scenario: A has finally_hook, spawns B. B fails and retries.**

```
1. A runs, succeeds, spawns [B]
   - finally_tracker.start_tracking(A.id, pending=1)
   - B queued with origin_id = Some(A.id)

2. B runs, FAILS, needs retry
   - process_submit_result returns Requeued, tasks = [B']
   - B' queued with origin_id = Some(A.id)
   - notify_origin_of_completion(A.id) ← CALLED TOO EARLY!
   - A's pending goes from 1 to 0
   - A's finally hook runs ← WRONG! B hasn't succeeded yet!

3. B' runs, succeeds
   - notify_origin_of_completion(A.id) is called
   - But A's tracking entry was already removed
   - Nothing happens ← orphaned notification
```

**Result:** A's finally runs after B fails, before B' succeeds.

### Additional Bug: When B Also Has a Finally Hook

If B has a finally_hook, the behavior is slightly different but still wrong:

```
1. A runs, succeeds, spawns [B]
   - finally_tracker[A] = { pending: 1 }
   - B queued with origin_id = Some(A)

2. B runs, FAILS, needs retry
   - process_submit_result returns tasks = [B']
   - B has finally_hook, final_tasks = [B'] is non-empty
   - start_tracking(B.id, pending=1) ← B tracks B' as "descendant" (wrong!)
   - child_origin_id = Some(B)
   - B' queued with origin_id = Some(B) (not A)
   - notify_origin(A): pending 1→0
   - A's finally runs ← TOO EARLY!

3. B' runs, succeeds
   - notify_origin(B): pending 1→0
   - B's finally runs ← with stale finally_value from failed B!
```

**Bugs in this scenario:**
1. A's finally still runs too early (when B fails)
2. B's finally treats retry B' as a "descendant" - conceptually wrong
3. B's finally runs with `finally_value` from failed execution, not successful one

### The Code That's Wrong

```rust
// mod.rs:263-266 - CURRENT (BROKEN)
// This is called unconditionally, even for retries
if let Some(oid) = origin_id {
    self.notify_origin_of_completion(oid);
}
```

The problem: we notify the origin regardless of whether the task succeeded, failed and is retrying, or was dropped.

---

## Proposed Architecture (FIXED)

### New Return Type

Separate spawned children from retry signaling:

```rust
// response.rs - PROPOSED
pub struct ProcessedSubmit {
    pub outcome: TaskOutcome,
    pub post_input: PostHookInput,
}

pub enum TaskOutcome {
    /// Task succeeded, may have spawned children
    Success {
        spawned: Vec<Task>,
        finally_value: EffectiveValue,
    },
    /// Task failed, should be retried (returns the retry task)
    Retry(Task),
    /// Task failed, no more retries (max exceeded or disabled)
    Dropped,
}
```

### Implementation Steps

#### Step 0: Write tests that demonstrate the bug (DO THIS FIRST)

Before changing any production code, write tests that:
1. Fail with the current implementation (proving the bug exists)
2. Will pass after the fix is implemented

**Test file:** `crates/gsd_config/tests/finally_retry_bugs.rs`

**Test 1: `finally_runs_too_early_on_retry`**
```rust
/// Demonstrates the bug: A's finally hook runs when B fails, not when B' succeeds.
///
/// Setup:
/// - A has finally_hook that writes to a file
/// - A spawns B
/// - B fails first attempt (returns invalid JSON)
/// - B succeeds on retry (returns valid JSON)
///
/// Bug behavior (current):
/// - A's finally runs after B fails (wrong!)
/// - When B' succeeds, A's finally has already run
///
/// Correct behavior (after fix):
/// - A's finally runs after B' succeeds
#[test]
fn finally_runs_too_early_on_retry() {
    // ... setup agent that fails once then succeeds ...
    // ... config with finally_hook that writes timestamp to file ...
    // ... assert finally ran AFTER retry succeeded, not after initial failure ...
}
```

**How to detect the bug:**
- Finally hook writes `{"ran_at": <timestamp>, "task_count": <count>}` to a file
- Track when B fails (timestamp 1) and when B' succeeds (timestamp 2)
- Assert finally's `ran_at` is AFTER timestamp 2, not between timestamps 1 and 2

**Alternative detection (simpler):**
- Finally hook increments a counter in a file
- Agent fails first N times, succeeds on N+1
- Assert finally ran exactly once AND ran after all agent calls completed

#### Step 1: Add `TaskOutcome` enum

```rust
// types.rs - ADD
pub enum TaskOutcome {
    Success {
        spawned: Vec<Task>,
        finally_value: EffectiveValue,
    },
    Retry(Task),
    Dropped,
}
```

#### Step 2: Refactor `process_submit_result` to return `TaskOutcome`

```rust
// response.rs - BEFORE
pub fn process_submit_result(...) -> ProcessedSubmit {
    // ... lots of code ...
    let (result, tasks) = process_retry(task, &step.options, FailureKind::SubmitError);
    ProcessedSubmit {
        result,
        tasks,  // ← retry task mixed in with spawned children
        post_input,
        finally_value,
    }
}

// response.rs - AFTER
pub fn process_submit_result(...) -> ProcessedSubmit {
    // ... on failure ...
    let outcome = if should_retry(task, &step.options, failure_kind) {
        let mut retry_task = task.clone();
        retry_task.retries += 1;
        TaskOutcome::Retry(retry_task)
    } else {
        TaskOutcome::Dropped
    };
    ProcessedSubmit { outcome, post_input }
}
```

#### Step 3: Refactor `process_result` to match on `TaskOutcome`

```rust
// mod.rs - BEFORE
fn process_result(&mut self, inflight: InFlightResult) -> TaskResult {
    let ProcessedSubmit { result, tasks, post_input, finally_value } = ...;

    // tasks contains BOTH spawned children AND retries - can't tell apart!
    let (final_result, final_tasks) = if let Some(hook) = &step.post {
        // ...
    };

    for new_task in final_tasks {
        let id = self.next_task_id();
        self.queue.push_back(QueuedTask {
            task: new_task,
            id,
            origin_id: child_origin_id,  // ← retry gets wrong origin
        });
    }

    // WRONG: called even for retries
    if let Some(oid) = origin_id {
        self.notify_origin_of_completion(oid);
    }

    final_result
}

// mod.rs - AFTER
fn process_result(&mut self, inflight: InFlightResult) -> TaskResult {
    let ProcessedSubmit { outcome, post_input } = ...;

    match outcome {
        TaskOutcome::Success { spawned, finally_value } => {
            // Run post hook, may modify spawned tasks
            let final_tasks = if let Some(hook) = &step.post {
                run_post_hook_and_extract(hook, &post_input)
            } else {
                spawned
            };

            // Set up finally tracking for ACTUAL spawned children
            let child_origin_id = if let Some(finally) = &step.finally_hook
                && !final_tasks.is_empty()
            {
                self.finally_tracker.start_tracking(
                    task_id,
                    final_tasks.len(),
                    finally_value.0,
                    finally.clone(),
                );
                Some(task_id)
            } else {
                origin_id
            };

            // Queue spawned children with new IDs
            for child in final_tasks {
                let id = self.next_task_id();
                self.queue.push_back(QueuedTask {
                    task: child,
                    id,
                    origin_id: child_origin_id
                });
            }

            // NOW we can notify origin - task succeeded
            if let Some(oid) = origin_id {
                self.notify_origin_of_completion(oid);
            }

            TaskResult::Completed
        }

        TaskOutcome::Retry(retry_task) => {
            // Queue retry with new ID, parent = failed task
            let retry_id = self.next_task_id();
            self.queue.push_back(QueuedTask {
                task: retry_task,
                id: retry_id,
                origin_id: Some(task_id),  // ← parent is the failed execution
            });

            // Do NOT notify origin - task isn't done yet, it's retrying
            // The retry chain will eventually succeed or drop

            TaskResult::Requeued
        }

        TaskOutcome::Dropped => {
            // Task failed permanently
            // Notify origin - this descendant is "done" (failed)
            if let Some(oid) = origin_id {
                self.notify_origin_of_completion(oid);
            }

            TaskResult::Dropped
        }
    }
}
```

### Key Behavioral Changes

1. **Retry gets new `LogTaskId`**: Each execution is uniquely identified.

2. **Retry's parent is the failed task**: `origin_id = Some(task_id)` where `task_id` is the failed execution. This creates an execution chain.

3. **No origin notification on retry**: The failed execution doesn't notify its origin because the logical task isn't done - it's being retried.

4. **`finally_value` only exists for success**: The `EffectiveValue` is in the `Success` variant, not available for failures.

5. **Post hook only runs on success**: Question - currently post hook runs for all outcomes. Should it?

---

## Example Traces

### Before (Broken)

```
A spawns B (finally on A)
  finally_tracker[A] = { pending: 1 }

B (id=1) fails, retries
  → ProcessedSubmit { result: Requeued, tasks: [B'] }
  → B' queued with id=2, origin = A
  → notify_origin(A): pending 1→0
  → A's finally runs ← TOO EARLY!

B' (id=2) succeeds
  → notify_origin(A): no entry found, ignored
  → orphaned notification

Result: A's finally ran before B succeeded
```

### After (Fixed)

```
A spawns B (finally on A)
  finally_tracker[A] = { pending: 1 }

B (id=1) fails, retries
  → TaskOutcome::Retry(B')
  → B' queued with id=2, origin = B (id=1)  ← parent is failed execution
  → NO notification to A  ← correct!
  → finally_tracker unchanged

B' (id=2) succeeds
  → TaskOutcome::Success { spawned: [] }
  → notify_origin(B): no entry found (B has no finally), ignored
  → but wait... A still has pending=1!

PROBLEM: Now A's finally never runs because we lost the chain.
```

### The Missing Piece: Retry Chain Completion

When the retry chain ends (either succeeds or is dropped), we need to notify the ORIGINAL ancestor that started the chain. This requires tracking the "finally origin" separately from the "execution parent".

Options:
1. **Propagate on success**: When B' succeeds, it notifies B (its parent). B then propagates to A.
2. **Track chain origin**: B' remembers "A is waiting for me" even though B is its execution parent.
3. **Two-level tracking**: `execution_parent_id` (for logging) vs `finally_origin_id` (for completion).

The cleanest is probably option 3: separate execution parent (for the ID chain) from finally origin (for completion tracking).

```rust
pub(super) struct QueuedTask {
    pub task: Task,
    pub id: LogTaskId,
    pub execution_parent_id: Option<LogTaskId>,  // NEW: for logging/tracing
    pub finally_origin_id: Option<LogTaskId>,    // RENAMED: for finally tracking
}
```

When B fails and retries:
- B' gets `execution_parent_id = Some(B)`
- B' gets `finally_origin_id = B.finally_origin_id` (propagates A's tracking)

When B' succeeds:
- notify `finally_origin_id` (which is A)

This keeps both concerns addressed.

---

## Files Changed

### `crates/gsd_config/src/runner/types.rs`

- Add `TaskOutcome` enum
- Change `QueuedTask` to have `execution_parent_id` and `finally_origin_id`
- Remove or modify `TaskResult::Requeued`

### `crates/gsd_config/src/runner/response.rs`

- Remove `process_retry` returning `Vec<Task>`
- Change `ProcessedSubmit` to use `TaskOutcome`
- Simplify `process_submit_result` - success returns spawned, failure returns Retry/Dropped

### `crates/gsd_config/src/runner/mod.rs`

- Rewrite `process_result` to match on `TaskOutcome`
- Handle retry by re-queuing with new ID, execution_parent = failed task
- Only set up finally tracking for `Success` variant
- Only notify `finally_origin_id` on `Success` or `Dropped`, not `Retry`

---

## Open Questions

1. **Post hook on failure**: Currently post hook runs for all outcomes. Should it only run on success? The `PostHookInput` enum has `Error` and `PreHookError` variants - are these useful?

2. **Dropped tasks and finally**: If B is dropped (max retries), should A's finally still run? Current proposal says yes (notify origin on Dropped). Is this correct?

---

## Test Plan

**CRITICAL: Write these tests BEFORE implementing the fix.** These tests should FAIL with the current code, proving the bug exists.

### Test 1: `finally_runs_too_early_on_retry` (MUST FAIL)

**Setup:**
- Step A: has `finally_hook` that appends "finally_ran" to a log file
- Step B: no finally hook
- A's agent returns `[{kind: "B", value: {}}]`
- B's agent: fails first call (invalid JSON), succeeds second call (`[]`)

**Execution:**
1. Run with initial task A
2. A succeeds, spawns B, sets up finally tracking (pending=1)
3. B fails, retries
4. **BUG**: `notify_origin(A)` called, pending goes 0→0, finally runs
5. B' succeeds
6. **BUG**: `notify_origin(A)` called but entry already removed

**Assertion (to detect bug):**
- Count how many times the agent was called for step B
- Assert finally ran AFTER all B agent calls completed
- With current code: finally runs after 1 call (wrong)
- With fix: finally runs after 2 calls (correct)

### Test 2: `finally_never_runs_after_retry_success` (MUST FAIL)

This is the flip side - in current broken code, the second notification is lost.

**Setup:** Same as Test 1

**Assertion:**
- Assert finally ran exactly once
- With current code: finally runs once (but at wrong time)
- With fix: finally runs once (at correct time)

This test might pass even with broken code (just at wrong time), so Test 1 is more important.

### Test 3: `nested_finally_with_retry` (MUST FAIL)

**Setup:**
- Step A: has `finally_hook` (writes "A_finally" to log)
- Step B: has `finally_hook` (writes "B_finally" to log)
- A's agent returns `[{kind: "B", value: {}}]`
- B's agent: fails first call, succeeds second call

**Expected correct order:**
1. A runs, spawns B
2. B fails (attempt 1)
3. B' succeeds (attempt 2)
4. B's finally runs (after B' succeeds)
5. A's finally runs (after B's finally completes)

**Actual buggy order:**
1. A runs, spawns B
2. B fails (attempt 1)
3. **A's finally runs** ← too early!
4. B' succeeds (attempt 2)
5. B's finally runs

**Assertion:**
- Read the log file
- Assert order is: B_finally, then A_finally (not A_finally then B_finally)

### Test 4: `finally_runs_when_all_retries_exhausted`

**Setup:**
- A has `finally_hook`
- B has `max_retries: 2`
- B's agent always fails

**Execution:**
1. A spawns B
2. B fails, retries (1)
3. B fails, retries (2)
4. B fails, dropped (max exceeded)

**Assertion:**
- Finally should run when B is dropped
- This tests the `Dropped` path

### Implementation Note

Use the existing test infrastructure in `tests/common/mod.rs`:
- `GsdTestAgent::start` with a closure that tracks call count
- Config with `finally_hook` pointing to a script
- Script writes to a temp file that we can read after run completes

---

## Relationship to Other Refactors

**Do this BEFORE:**
- `FINALLY_TRACKING` - that refactor assumes "what is a descendant" is well-defined
- `FINALLY_SCHEDULING` - converting finally to response data also depends on correct model

**This fixes:**
- The BUG comment in mod.rs about `notify_origin_of_completion` being called too early
- The confusing `finally_value` for failure cases
- The conflation of spawned tasks and retries
