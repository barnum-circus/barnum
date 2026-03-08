# State Persistence and Resume

**Status:** Not started

**Depends on:** ROOT_FLAG_REFACTOR (complete), INLINED_CONFIG (complete), FINALLY_TRACKING (complete), FINALLY_SCHEDULING (complete)

## Motivation

Long-running GSD jobs can be interrupted (crash, Ctrl+C, OOM). State persistence enables resuming from where you left off.

## Core Idea

A run creates a single NDJSON log file. First entry is config, then task events. On resume, replay log to reconstruct pending tasks.

**Two types of logs (don't confuse them):**
- **Debug logs** (`--log-file`): Tracing output for debugging
- **State logs** (this feature): Machine-readable NDJSON for persistence/resume

## Current Runner Architecture (as of 2026-03-08, updated after FINALLY_SCHEDULING)

The runner module (`crates/gsd_config/src/runner/`) has these submodules:
- `mod.rs` - TaskRunner struct, main loop, `run()` public function, RunnerConfig, TaskEntry, TaskState, InFlight
- `dispatch.rs` - TaskContext, TaskIdentity, InFlightResult, SubmitResult, dispatch_*_task functions
- `hooks.rs` - run_pre_hook, run_post_hook
- `shell.rs` - `run_shell_command()` helper
- `response.rs` - Response processing and retry logic
- `submit.rs` - CLI invocation for agent_pool

Key current state:
- `initial_tasks` passed separately from `RunnerConfig`
- `RunnerConfig` passed by reference
- Unified task state in `BTreeMap<LogTaskId, TaskEntry>`:
  - `TaskEntry { step, parent_id, finally_script, state, retries_remaining }`
  - `TaskState::Pending { value }` - queued, waiting for dispatch
  - `TaskState::InFlight(InFlight)` - currently executing
  - `TaskState::WaitingForChildren { pending_children_count, finally_data }` - waiting for children
- `parent_id` is always the immediate parent (tree structure for proper finally tracking)
- `finally_script: Option<HookScript>` identifies finally tasks (same step name as parent, dispatched differently)

## State Log Format

Newline-delimited JSON. First entry MUST be `Config` (exactly once). Uses `#[serde(tag = "kind")]`.

```json
{"kind":"Config","config":{...}}
{"kind":"TaskSubmitted","task_id":1,"step":"Analyze","value":{...},"parent_id":null,"origin":{"kind":"Initial"}}
{"kind":"TaskSubmitted","task_id":2,"step":"Analyze","value":{...},"parent_id":null,"origin":{"kind":"Initial"}}
{"kind":"TaskCompleted","task_id":1,"outcome":{"kind":"Success","value":{"spawned_task_ids":[3]}}}
{"kind":"TaskSubmitted","task_id":3,"step":"Process","value":{...},"parent_id":1,"origin":{"kind":"Spawned"}}
{"kind":"TaskCompleted","task_id":2,"outcome":{"kind":"Failed","value":{"reason":{"kind":"Timeout"},"retry_task_id":4}}}
{"kind":"TaskSubmitted","task_id":4,"step":"Analyze","value":{...},"parent_id":null,"origin":{"kind":"Retry","replaces":2}}
{"kind":"TaskCompleted","task_id":4,"outcome":{"kind":"Success","value":{"spawned_task_ids":[]}}}
{"kind":"TaskSubmitted","task_id":5,"step":"Analyze","value":{...},"parent_id":1,"origin":{"kind":"Finally","finally_for":1}}
```

**Task origins:**
- `Initial`: From `--initial-state`
- `Spawned`: Created by parent task's output (regular child)
- `Retry`: Replacement for a failed task
- `Finally`: Finally hook task for a completed task

## Data Structures

```rust
use serde::{Deserialize, Serialize};
use crate::resolved::Config;
use crate::types::StepName;

// Defined in crate::types - shown here for reference
#[derive(Debug, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LogTaskId(pub u32);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StateLogConfig {
    pub config: Config,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskSubmitted {
    pub task_id: LogTaskId,
    pub step: StepName,
    pub value: serde_json::Value,
    /// Task waiting for this one to complete (tree parent).
    pub parent_id: Option<LogTaskId>,
    /// How this task came to exist.
    pub origin: TaskOrigin,
}

/// How a task came to be created.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TaskOrigin {
    /// From `--initial-state` (root task).
    Initial,
    /// Spawned by parent task's action output.
    Spawned,
    /// Retry of a failed task.
    Retry {
        /// The task this replaces.
        replaces: LogTaskId,
    },
    /// Finally hook for a completed task.
    Finally {
        /// The task whose finally hook this is.
        finally_for: LogTaskId,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskCompleted {
    pub task_id: LogTaskId,
    pub outcome: TaskOutcome,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum TaskOutcome {
    Success(TaskSuccess),
    Failed(TaskFailed),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskSuccess {
    /// IDs of tasks spawned by this task's output.
    pub spawned_task_ids: Vec<LogTaskId>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskFailed {
    pub reason: FailureReason,
    /// If the task was retried, the ID of the retry task.
    /// None if retries were exhausted or disabled.
    pub retry_task_id: Option<LogTaskId>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum FailureReason {
    Timeout,
    AgentLost,
    InvalidResponse { message: String },
}
```

## Writing/Reading

```rust
fn write_entry(file: &mut File, entry: &StateLogEntry) -> io::Result<()> {
    serde_json::to_writer(&mut *file, entry)?;
    file.write_all(b"\n")?;
    file.flush()
}

fn read_entries(file: File) -> impl Iterator<Item = io::Result<StateLogEntry>> {
    BufReader::new(file).lines().map(|line| {
        let line = line?;
        serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    })
}
```

Validation (config first, config once) happens at the call site.

## Reconstructing State on Resume

```rust
type PendingTasks = BTreeMap<LogTaskId, TaskSubmitted>;

#[derive(Debug, thiserror::Error)]
enum ReconstructError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("empty log file")]
    EmptyLog,
    #[error("first entry must be Config")]
    FirstEntryNotConfig,
    #[error("Config appeared more than once")]
    DuplicateConfig,
    #[error("duplicate task_id {0:?}")]
    DuplicateTaskId(LogTaskId),
    #[error("TaskCompleted for unknown task_id {0:?}")]
    UnknownTaskId(LogTaskId),
}

fn reconstruct(
    mut entries: impl Iterator<Item = io::Result<StateLogEntry>>,
) -> Result<(Config, PendingTasks), ReconstructError> {
    // First entry must be Config
    let config = match entries.next() {
        Some(Ok(StateLogEntry::Config(c))) => c.config,
        Some(Ok(_)) => return Err(ReconstructError::FirstEntryNotConfig),
        Some(Err(e)) => return Err(e.into()),
        None => return Err(ReconstructError::EmptyLog),
    };

    let mut pending = PendingTasks::new();

    for entry in entries {
        match entry? {
            StateLogEntry::Config(_) => {
                return Err(ReconstructError::DuplicateConfig);
            }
            StateLogEntry::TaskSubmitted(task) => {
                if pending.contains_key(&task.task_id) {
                    return Err(ReconstructError::DuplicateTaskId(task.task_id));
                }
                pending.insert(task.task_id, task);
            }
            StateLogEntry::TaskCompleted(c) => {
                if pending.remove(&c.task_id).is_none() {
                    return Err(ReconstructError::UnknownTaskId(c.task_id));
                }
            }
        }
    }

    Ok((config, pending))
}
```

**Notes on retry counting:** The number of retries is not stored in the log. Instead, it can be computed by following the `Retry { replaces }` chain backwards. On resume, we validate against `max_retries` from the config.

## CLI

```bash
# Normal run - creates state log in default folder
gsd run --config config.jsonc --pool mypool --initial-state '[...]'
# Creates: <root>/runs/<pool>/<timestamp>.ndjson

# Explicit state log path
gsd run --config config.jsonc --pool mypool --initial-state '[...]' --state-log /tmp/myrun.ndjson

# Resume to same default folder (new log file)
gsd run --resume-from /tmp/old.ndjson
# Creates new log, copies entries from old.ndjson, continues

# Resume to explicit path
gsd run --resume-from /tmp/old.ndjson --state-log /tmp/new.ndjson
# Copies entries from old.ndjson to new.ndjson, continues writing to new.ndjson

# PANIC: same path for both (no in-place mutation)
gsd run --resume-from /tmp/run.ndjson --state-log /tmp/run.ndjson  # panic!
```

`--resume-from` is incompatible with config file, `--initial-state`, and `--entrypoint-value` (panic if any combination provided).

## Implementation Phases

### Phase 1: Extract Shared Types → `gsd_types` crate

Create a new `gsd_types` crate with types shared between `gsd_config` and the new `gsd_state` crate.

**Identify candidates in `gsd_config`:**
- `LogTaskId` - used in log entries and runner
- `StepName` - used in log entries and config
- `StepInputValue` - used in log entries and runner
- `HookScript` - might be needed if we log finally hooks

**Tasks:**
1. Create `crates/gsd_types/` with `Cargo.toml`
2. Move shared types from `gsd_config::types` to `gsd_types`
3. Update `gsd_config` to re-export or depend on `gsd_types`
4. Verify all existing tests pass

**Branch:** `state/01-shared-types`

---

### Phase 2: Create `gsd_state` crate with Tests

Create the new crate with log types and comprehensive tests (no integration with runner yet).

**Types to implement:**
- `StateLogEntry` (Config, TaskSubmitted, TaskCompleted)
- `TaskSubmitted` (task_id, step, value, parent_id, origin)
- `TaskOrigin` (Initial, Spawned, Retry, Finally)
- `TaskCompleted` (task_id, outcome)
- `TaskOutcome` (Success, Failed)
- `TaskSuccess` (spawned_task_ids)
- `TaskFailed` (reason, retry_task_id)
- `FailureReason` (Timeout, AgentLost, InvalidResponse)

**Write/read functions:**
- `write_entry(file, entry)` - append NDJSON line
- `read_entries(file)` - iterator over entries
- `reconstruct(entries)` - return `(Config, ReconstructedState)`

**ReconstructedState contains:**
- Tasks needing action run (Pending)
- Tasks waiting for children (WaitingForChildren with pending_count and finally_data)
- Parent relationships preserved
- Finally tasks identified

**Tests to write:**

#### Snapshot Tests (Demo-Based)

Each demo produces a deterministic state log. We snapshot-test the log output to catch regressions.

**Storage location:** Snapshots live alongside each demo:
```
crates/gsd_cli/demos/
├── linear/
│   ├── config.jsonc
│   ├── demo.sh
│   └── snapshot.ndjson     # Expected state log output
├── fan-out/
│   ├── config.jsonc
│   ├── demo.sh
│   └── snapshot.ndjson
├── hooks/
│   ├── config.jsonc
│   ├── demo.sh
│   ├── pre-hook.sh
│   ├── post-hook.sh
│   ├── finally-hook.sh
│   └── snapshot.ndjson
└── ...
```

**How it works:**

1. **Demo scripts write snapshots.** Each `demo.sh` runs gsd with `--state-log` pointing to a temp file, then copies to `snapshot.ndjson`:
   ```bash
   TEMP_LOG=$(mktemp)
   $GSD run config.jsonc --pool "$POOL_ID" --state-log "$TEMP_LOG" ...

   # Update snapshot (only when GSD_UPDATE_SNAPSHOTS=1)
   if [ "$GSD_UPDATE_SNAPSHOTS" = "1" ]; then
       cp "$TEMP_LOG" "$SCRIPT_DIR/snapshot.ndjson"
   fi
   ```

2. **CI verifies no changes.** After running all demos, CI checks that working tree is clean:
   ```yaml
   # .github/workflows/ci.yml
   - name: Run demos (generates snapshots)
     run: ./scripts/run-all-demos.sh

   - name: Verify snapshots unchanged
     run: |
       if ! git diff --quiet crates/gsd_cli/demos/*/snapshot.ndjson; then
         echo "Snapshot files changed! Run demos locally and commit updated snapshots."
         git diff crates/gsd_cli/demos/*/snapshot.ndjson
         exit 1
       fi
   ```

3. **Local update workflow.** When log format changes intentionally:
   ```bash
   GSD_UPDATE_SNAPSHOTS=1 ./scripts/run-all-demos.sh
   git add crates/gsd_cli/demos/*/snapshot.ndjson
   git commit -m "Update state log snapshots"
   ```

**Tests:**
```rust
#[test] fn snapshot_demo_linear()
#[test] fn snapshot_demo_fan_out()
#[test] fn snapshot_demo_branching()
#[test] fn snapshot_demo_command()
#[test] fn snapshot_demo_hooks()  // exercises pre/post/finally
```

Each test:
1. Runs the demo config with `OrderedAgentController` (deterministic completion order)
2. Captures state log output
3. Compares against `snapshot.ndjson` in the demo folder
4. Fails if they differ

**Prerequisite: Deterministic Ordering** [DONE - see ORDERED_MOCK_POOL refactor]

`OrderedAgentController` (in `crates/gsd_config/tests/common/mod.rs`) provides deterministic task completion:

```rust
let (agent, ctrl) = GsdTestAgent::ordered(&root);

// Start GSD in background thread
let handle = thread::spawn(|| gsd.run(...));

// Wait for tasks, complete in controlled order
ctrl.wait_for_tasks(1);  // Block until 1 task waiting
let tasks = ctrl.waiting_tasks();  // Inspect (kind, payload)
ctrl.complete_at(0, "[]");  // Complete task at index 0

handle.join().unwrap();
```

For Command actions (bash), order is already deterministic with `max_concurrency: 1`.

#### Reconstruct: Basic Scenarios
```rust
#[test] fn reconstruct_empty_log_errors()
#[test] fn reconstruct_config_only_returns_empty_state()
#[test] fn reconstruct_single_task_pending()
#[test] fn reconstruct_single_task_completed_returns_empty()
#[test] fn reconstruct_submit_complete_submit_leaves_second_pending()
```

#### Reconstruct: Parent-Child Relationships
```rust
#[test] fn reconstruct_child_pending_parent_waiting()
#[test] fn reconstruct_two_children_one_complete_parent_waiting()
#[test] fn reconstruct_all_children_complete_parent_done()
#[test] fn reconstruct_grandchild_pending_sets_ancestor_waiting()
#[test] fn reconstruct_preserves_parent_id_on_pending_tasks()
```

#### Reconstruct: Retry Chains
```rust
#[test] fn reconstruct_failed_with_retry_only_retry_pending()
#[test] fn reconstruct_failed_without_retry_task_dropped()
#[test] fn reconstruct_retry_chain_only_final_pending()
#[test] fn reconstruct_retry_of_child_parent_still_waiting()
```

#### Reconstruct: Finally Tasks
```rust
#[test] fn reconstruct_finally_pending_after_parent_complete()
#[test] fn reconstruct_finally_identifies_via_origin()
#[test] fn reconstruct_finally_for_task_with_children_waits()
#[test] fn reconstruct_finally_complete_parent_done()
#[test] fn reconstruct_finally_spawns_children_finally_waiting()
```

#### Reconstruct: WaitingForChildren State
```rust
#[test] fn reconstruct_waiting_has_correct_pending_count()
#[test] fn reconstruct_waiting_preserves_finally_value()
#[test] fn reconstruct_waiting_task_not_re_queued_for_action()
```

#### Reconstruct: Error Cases
```rust
#[test] fn reconstruct_duplicate_task_id_errors()
#[test] fn reconstruct_complete_unknown_task_errors()
#[test] fn reconstruct_duplicate_config_errors()
#[test] fn reconstruct_first_entry_not_config_errors()
#[test] fn reconstruct_retry_for_nonexistent_task_errors()
#[test] fn reconstruct_finally_for_nonexistent_task_errors()
```

#### Reconstruct: Complex Scenarios
```rust
#[test] fn reconstruct_mixed_pending_waiting_done()
#[test] fn reconstruct_diamond_dependency()  // A spawns B,C; B,C both spawn D
#[test] fn reconstruct_deep_nesting_five_levels()
#[test] fn reconstruct_wide_fanout_ten_children()
#[test] fn reconstruct_interleaved_submits_and_completes()
```

#### Write/Read Functions
```rust
#[test] fn write_entry_appends_newline()
#[test] fn write_entry_flushes()
#[test] fn read_entries_parses_ndjson()
#[test] fn read_entries_handles_empty_file()
#[test] fn read_entries_handles_trailing_newline()
#[test] fn read_entries_errors_on_invalid_json()
```

---

**Note:** Snapshot tests require deterministic ordering. If no mechanism exists yet, Phase 2 should add `OrderedMockPool` (or similar) to the test harness before writing snapshot tests. This might warrant its own sub-branch:

- `state/02a-deterministic-test-harness` - add ordered mock pool
- `state/02b-crate-and-tests` - gsd_state crate with snapshot + reconstruct tests

**Branch:** `state/02-crate-and-tests`

---

### Phase 3: Integrate Logging into Runner

Add log writing to `gsd_config` runner without changing behavior.

**Changes to runner:**
- Accept optional `StateLogWriter` (trait or concrete type)
- `queue_task()` → write `TaskSubmitted`
- `task_succeeded()` → write `TaskCompleted(Success)`
- `task_failed()` → write `TaskCompleted(Failed)` with retry_task_id if applicable
- `schedule_finally()` → write `TaskSubmitted` with `Finally` origin

**CLI changes:**
- Add `--state-log <path>` flag to `gsd run`
- Print resume instructions on startup

#### Before/After: TaskRunner struct

```rust
// BEFORE
struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    pool: PoolConnection,
    max_concurrency: usize,
    in_flight: usize,
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,
    next_task_id: u32,
}

// AFTER
struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    pool: PoolConnection,
    max_concurrency: usize,
    in_flight: usize,
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,
    next_task_id: u32,
    log_writer: StateLogWriter,
}
```

#### Before/After: queue_task (for initial/spawned tasks)

```rust
// BEFORE
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>) {
    let id = self.next_task_id();
    let retries_remaining = self.step_map.get(&task.step).map_or(0, |s| s.options.max_retries);

    if self.in_flight < self.max_concurrency {
        let prev = self.tasks.insert(id, TaskEntry { /* ... */ });
        assert!(prev.is_none(), "task_id collision");
        self.in_flight += 1;
        self.dispatch(id, task);
    } else {
        let prev = self.tasks.insert(id, TaskEntry { /* ... */ });
        assert!(prev.is_none(), "task_id collision");
    }
}

// AFTER
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>, origin: TaskOrigin) {
    let id = self.next_task_id();
    let retries_remaining = self.step_map.get(&task.step).map_or(0, |s| s.options.max_retries);

    // LOG: TaskSubmitted
    self.log_writer.write(StateLogEntry::TaskSubmitted(TaskSubmitted {
        task_id: id,
        step: task.step.clone(),
        value: task.value.0.clone(),
        parent_id,
        origin,
    }));

    if self.in_flight < self.max_concurrency {
        let prev = self.tasks.insert(id, TaskEntry { /* ... */ });
        assert!(prev.is_none(), "task_id collision");
        self.in_flight += 1;
        self.dispatch(id, task);
    } else {
        let prev = self.tasks.insert(id, TaskEntry { /* ... */ });
        assert!(prev.is_none(), "task_id collision");
    }
}
```

#### Before/After: task_succeeded

```rust
// BEFORE
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    self.in_flight -= 1;

    let entry = self.tasks.get(&task_id).expect("task must exist");
    let finally_hook = self.lookup_finally_hook(entry);

    if spawned.is_empty() {
        if let Some(hook) = finally_hook {
            self.schedule_finally(task_id, hook, value);
        }
        self.remove_and_notify_parent(task_id);
    } else {
        let count = NonZeroU16::new(spawned.len() as u16).unwrap();
        let finally_data = finally_hook.map(|hook| (hook, value));
        // ... transition to WaitingForChildren, queue children
        for child in spawned {
            self.queue_task(child, Some(task_id));
        }
    }
}

// AFTER
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    self.in_flight -= 1;

    // Collect spawned task IDs for logging (assigned during queue_task)
    let spawned_start_id = LogTaskId(self.next_task_id);

    let entry = self.tasks.get(&task_id).expect("task must exist");
    let finally_hook = self.lookup_finally_hook(entry);

    if spawned.is_empty() {
        // LOG: TaskCompleted(Success) with no children
        self.log_writer.write(StateLogEntry::TaskCompleted(TaskCompleted {
            task_id,
            outcome: TaskOutcome::Success(TaskSuccess { spawned_task_ids: vec![] }),
        }));

        if let Some(hook) = finally_hook {
            self.schedule_finally(task_id, hook, value);
        }
        self.remove_and_notify_parent(task_id);
    } else {
        let spawned_count = spawned.len();
        let count = NonZeroU16::new(spawned_count as u16).unwrap();
        let finally_data = finally_hook.map(|hook| (hook, value));
        // ... transition to WaitingForChildren

        for child in spawned {
            self.queue_task(child, Some(task_id), TaskOrigin::Spawned);
        }

        // LOG: TaskCompleted(Success) with spawned_task_ids
        let spawned_task_ids: Vec<LogTaskId> = (spawned_start_id.0..self.next_task_id)
            .map(LogTaskId)
            .collect();
        self.log_writer.write(StateLogEntry::TaskCompleted(TaskCompleted {
            task_id,
            outcome: TaskOutcome::Success(TaskSuccess { spawned_task_ids }),
        }));
    }
}
```

#### Before/After: task_failed

```rust
// BEFORE
fn task_failed(&mut self, task_id: LogTaskId, retry: Option<Task>) {
    let parent_id = self.tasks.get(&task_id).expect("task must exist").parent_id;

    if let Some(retry_task) = retry {
        self.queue_task(retry_task, parent_id);
        self.transition_to_done(task_id);
    } else {
        // Permanent failure
        let entry = self.tasks.remove(&task_id).expect("task must exist");
        // ...
    }
}

// AFTER
fn task_failed(&mut self, task_id: LogTaskId, retry: Option<Task>, reason: FailureReason) {
    let parent_id = self.tasks.get(&task_id).expect("task must exist").parent_id;

    let retry_task_id = if let Some(retry_task) = retry {
        let retry_id = LogTaskId(self.next_task_id);  // Will be assigned by queue_task
        self.queue_task(retry_task, parent_id, TaskOrigin::Retry { replaces: task_id });
        self.transition_to_done(task_id);
        Some(retry_id)
    } else {
        let entry = self.tasks.remove(&task_id).expect("task must exist");
        // ...
        None
    };

    // LOG: TaskCompleted(Failed)
    self.log_writer.write(StateLogEntry::TaskCompleted(TaskCompleted {
        task_id,
        outcome: TaskOutcome::Failed(TaskFailed { reason, retry_task_id }),
    }));
}
```

#### Before/After: schedule_finally

```rust
// BEFORE
fn schedule_finally(&mut self, task_id: LogTaskId, hook: HookScript, value: StepInputValue) {
    let entry = self.tasks.get(&task_id).expect("task must exist");
    let parent_id = entry.parent_id;
    let step = entry.step.clone();

    if let Some(parent_id) = parent_id {
        self.increment_pending_children(parent_id);
    }

    let id = self.next_task_id();
    let finally_entry = TaskEntry {
        step,
        parent_id,
        finally_script: Some(hook),
        state: TaskState::Pending { value },
        retries_remaining,
    };
    self.tasks.insert(id, finally_entry);
}

// AFTER
fn schedule_finally(&mut self, task_id: LogTaskId, hook: HookScript, value: StepInputValue) {
    let entry = self.tasks.get(&task_id).expect("task must exist");
    let parent_id = entry.parent_id;
    let step = entry.step.clone();

    if let Some(parent_id) = parent_id {
        self.increment_pending_children(parent_id);
    }

    let id = self.next_task_id();

    // LOG: TaskSubmitted with Finally origin
    self.log_writer.write(StateLogEntry::TaskSubmitted(TaskSubmitted {
        task_id: id,
        step: step.clone(),
        value: value.0.clone(),
        parent_id,
        origin: TaskOrigin::Finally { finally_for: task_id },
    }));

    let finally_entry = TaskEntry {
        step,
        parent_id,
        finally_script: Some(hook),
        state: TaskState::Pending { value },
        retries_remaining,
    };
    self.tasks.insert(id, finally_entry);
}
```

**Tests:**
- Run existing demos with `--state-log`, verify log is valid NDJSON
- Parse log and verify task IDs are monotonic
- Verify parent_id relationships match spawned_task_ids
- Verify retry chains have correct `replaces` references
- Verify finally tasks have correct `finally_for` references

**Branch:** `state/03-logging`

---

### Phase 4: Implement Resume

Add `--resume-from` flag and resume logic.

**Tasks:**
1. Add `--resume-from <path>` CLI flag
2. Validate flag combinations (incompatible with config file, --initial-state)
3. Read log and call `reconstruct()` to get full runner state
4. Create new log file, copy existing entries
5. Initialize runner with reconstructed state (not just initial tasks)
6. Continue normal execution, appending to new log

**Runner changes for resume:**
The runner needs a way to be initialized with pre-existing state:
- Tasks in `Pending` state (need action run)
- Tasks in `WaitingForChildren` state (don't run action, just wait)
- Correct `pending_children_count` for waiting tasks
- `finally_data` preserved for tasks that have finally hooks
- Parent relationships intact

This is NOT exposed via `--initial-state` CLI. It's internal to resume.

#### Before/After: TaskRunner::new signature

```rust
// BEFORE
impl<'a> TaskRunner<'a> {
    fn new(
        config: &'a Config,
        schemas: &'a CompiledSchemas,
        runner_config: &RunnerConfig<'a>,
        initial_tasks: Vec<Task>,
    ) -> io::Result<Self> {
        // ... setup ...
        for task in initial_tasks {
            runner.queue_task(task, None, TaskOrigin::Initial);
        }
        Ok(runner)
    }
}

// AFTER
impl<'a> TaskRunner<'a> {
    fn new(
        config: &'a Config,
        schemas: &'a CompiledSchemas,
        runner_config: &RunnerConfig<'a>,
        initial_state: InitialState,
        log_writer: StateLogWriter,
    ) -> io::Result<Self> {
        // ... setup ...
        match initial_state {
            InitialState::Fresh(tasks) => {
                for task in tasks {
                    runner.queue_task(task, None, TaskOrigin::Initial);
                }
            }
            InitialState::Resumed(state) => {
                runner.load_reconstructed_state(state);
            }
        }
        Ok(runner)
    }
}
```

#### New: InitialState enum

```rust
pub enum InitialState {
    /// Fresh run with initial tasks (from --initial-state CLI)
    Fresh(Vec<Task>),
    /// Resumed from log file
    Resumed(ReconstructedState),
}
```

#### New: ReconstructedState (from gsd_state crate)

```rust
/// State reconstructed from a log file for resume.
pub struct ReconstructedState {
    /// Tasks that need their action run (were Pending or InFlight at crash).
    pub pending_tasks: Vec<ReconstructedTask>,
    /// Tasks waiting for children (action completed, children still running).
    pub waiting_tasks: Vec<WaitingTask>,
    /// Next task ID to use (continues from log).
    pub next_task_id: u32,
}

pub struct ReconstructedTask {
    pub task_id: LogTaskId,
    pub step: StepName,
    pub value: StepInputValue,
    pub parent_id: Option<LogTaskId>,
    pub origin: TaskOrigin,
}

pub struct WaitingTask {
    pub task_id: LogTaskId,
    pub step: StepName,
    pub parent_id: Option<LogTaskId>,
    pub pending_children_count: NonZeroU16,
    pub finally_value: Option<StepInputValue>,
}
```

#### New: load_reconstructed_state

```rust
impl TaskRunner<'_> {
    fn load_reconstructed_state(&mut self, state: ReconstructedState) {
        self.next_task_id = state.next_task_id;

        for waiting in state.waiting_tasks {
            let finally_data = waiting.finally_value.and_then(|value| {
                self.step_map
                    .get(&waiting.step)
                    .and_then(|s| s.finally.clone())
                    .map(|hook| (hook, value))
            });

            self.tasks.insert(waiting.task_id, TaskEntry {
                step: waiting.step,
                parent_id: waiting.parent_id,
                finally_script: None,
                state: TaskState::WaitingForChildren {
                    pending_children_count: waiting.pending_children_count,
                    finally_data,
                },
                retries_remaining: 0,
            });
        }

        for pending in state.pending_tasks {
            let retries_remaining = self.step_map
                .get(&pending.step)
                .map_or(0, |s| s.options.max_retries);

            let finally_script = if matches!(pending.origin, TaskOrigin::Finally { .. }) {
                self.step_map.get(&pending.step).and_then(|s| s.finally.clone())
            } else {
                None
            };

            self.tasks.insert(pending.task_id, TaskEntry {
                step: pending.step,
                parent_id: pending.parent_id,
                finally_script,
                state: TaskState::Pending { value: pending.value },
                retries_remaining,
            });
        }
    }
}
```

#### Before/After: run() public function

```rust
// BEFORE
pub fn run(
    config: &Config,
    schemas: &CompiledSchemas,
    runner_config: &RunnerConfig<'_>,
    initial_tasks: Vec<Task>,
) -> io::Result<()> {
    let mut runner = TaskRunner::new(config, schemas, runner_config, initial_tasks)?;
    // ... run loop ...
}

// AFTER
pub fn run(
    config: &Config,
    schemas: &CompiledSchemas,
    runner_config: &RunnerConfig<'_>,
    initial_state: InitialState,
    log_writer: StateLogWriter,
) -> io::Result<()> {
    let mut runner = TaskRunner::new(config, schemas, runner_config, initial_state, log_writer)?;
    // ... run loop ...
}

/// Resume a run from a log file.
pub fn resume(
    old_log_path: &Path,
    new_log_path: &Path,
    runner_config: &RunnerConfig<'_>,
) -> io::Result<()> {
    // 1. Read and reconstruct from old log
    let (config, state) = gsd_state::reconstruct_from_file(old_log_path)?;

    // 2. Create new log, copy old entries
    let mut writer = StateLogWriter::new(new_log_path)?;
    gsd_state::copy_log(old_log_path, &mut writer)?;

    // 3. Compile schemas from config
    let schemas = CompiledSchemas::new(&config)?;

    // 4. Run with reconstructed state
    run(&config, &schemas, runner_config, InitialState::Resumed(state), writer)
}
```

**Tests:**
```rust
#[test] fn resume_empty_log_errors()
#[test] fn resume_no_pending_completes_immediately()
#[test] fn resume_pending_root_task_runs_action()
#[test] fn resume_pending_child_parent_waits()
#[test] fn resume_waiting_parent_not_re_run()
#[test] fn resume_pending_retry_runs()
#[test] fn resume_pending_finally_runs()
#[test] fn resume_nested_waiting_child_pending_finally_pending()
#[test] fn resume_preserves_task_ids()  // new tasks get IDs after resumed ones
#[test] fn resume_new_log_contains_old_entries()
#[test] fn resume_new_log_appends_new_entries()
#[test] fn resume_twice_works()  // resume from resumed log
```

**Integration tests (with actual process):**
```rust
#[test] fn crash_simulation_linear_workflow()
#[test] fn crash_simulation_fan_out()
#[test] fn crash_simulation_with_finally()
#[test] fn crash_simulation_with_retries()
```

**Branch:** `state/04-resume`

---

### Phase 5: Polish and Edge Cases

- Handle corrupted/truncated logs gracefully
- Add `gsd log inspect <path>` command to view log contents
- Consider compression for large logs
- Documentation

## Resume Semantics

On resume, we reconstruct the **exact runner state** from the log. This is NOT exposed via `--initial-state` CLI - it's internal to the resume logic.

**Key insight:** Treat all in-flight tasks as cancelled. Re-queue them.

**State reconstruction from log:**

| Log state | Reconstructed state |
|-----------|---------------------|
| `TaskSubmitted` with no `TaskCompleted` | `Pending` - needs action run |
| `TaskCompleted(Success{spawned})` where some spawned tasks incomplete | `WaitingForChildren` - don't re-run action |
| `TaskCompleted(Success{spawned})` where all spawned tasks complete | Done - removed from state |
| `TaskCompleted(Failed{retry_task_id: Some(id)})` | Done - retry task handles it |
| `TaskCompleted(Failed{retry_task_id: None})` | Done - task dropped |

**What we reconstruct:**
- `BTreeMap<LogTaskId, TaskEntry>` with correct states (Pending or WaitingForChildren)
- Parent relationships (`parent_id` from log)
- Finally tasks (identified by `TaskOrigin::Finally`)
- Pending children counts (computed from incomplete spawned_task_ids)

**What we punt on:**
- In-flight tasks at crash time → re-queued as Pending (may cause duplicate work)
- Partial action execution → ignored (action runs again from scratch)

## Prerequisite Refactors

All prerequisites are now complete:

1. **FINALLY_TRACKING** [DONE] COMPLETE - Changed to `parent_id` (always immediate parent) in unified `BTreeMap<LogTaskId, TaskEntry>`. Tree structure enables reconstructing task relationships from the log.

2. **FINALLY_SCHEDULING** [DONE] COMPLETE - Finally hooks now go through the task queue as regular tasks. Key implementation details:
   - Finally tasks are identified by `finally_script: Option<HookScript>` on `TaskEntry`
   - Finally tasks use the **same step name** as their parent (NOT a special `"__finally__"` step)
   - Finally tasks are scheduled as **siblings** (child of same parent), not child of the original task
   - `TaskState::WaitingForChildren { pending_children_count, finally_data }` stores the hook + value to schedule when children complete

3. **Task ID Registry** - Not needed. The unified `BTreeMap<LogTaskId, TaskEntry>` already provides this.

## Future Work

- Visualize state logs (TUI or web UI)
- `gsd runs list` command to show run status

## Documentation Updates (when implementing)

Add to `.claude/CLAUDE.md`:

```markdown
## Updating State Log Snapshots

When state log format changes, update snapshots:
\`\`\`bash
GSD_UPDATE_SNAPSHOTS=1 ./scripts/run-all-demos.sh
git add crates/gsd_cli/demos/*/snapshot.ndjson
\`\`\`
```
