# Apply Pattern for State/Log Consistency

**Status:** Not started

**Depends on:** STATE_PERSISTENCE (should be implemented as part of or after Phase 3)

## Motivation

Currently, state changes and log writes are separate operations scattered throughout `TaskRunner`:

```rust
// BEFORE: Two operations that must stay in sync — but nothing enforces it
self.state_log.write(TaskSubmitted { ... });
self.tasks.insert(id, entry);
```

This creates risk of:
1. Forgetting to write to log when changing state
2. Log/state getting out of sync
3. Bugs where state is updated but log isn't (or vice versa)
4. Resume logic diverging from live logic

## Architecture

### Where state lives

There are three distinct kinds of state in the runner. The apply pattern clarifies their ownership:

| State | Owner | Persisted? | Survives resume? |
|-------|-------|------------|------------------|
| **Task map** (Pending, WaitingForChildren) | `RunState` | Yes (via log) | Yes — rebuilt from log |
| **In-flight tracking** (InFlight, channels) | `TaskRunner` | No | No — Pending tasks re-dispatched |
| **Config, schemas, pool connection** | `TaskRunner` | Config in log | Config from log, rest from CLI args |

The key insight: **`RunState` is the only thing that needs to be persisted.** It holds exactly the state that can be reconstructed from the log. Everything else (`in_flight` count, mpsc channels, pool connection) is transient runtime machinery.

### The Applier trait

```rust
/// Reacts to state log events.
///
/// Each applier handles one concern: writing to disk, updating in-memory
/// state, emitting metrics, etc. The runner dispatches every event to
/// every applier.
trait Applier {
    fn apply(&mut self, entry: &StateLogEntry);
}
```

One method. No flags, no modes, no conditional logic.

### Two appliers

**`LogApplier`** — writes the entry to the NDJSON state log file. Zero logic.

```rust
struct LogApplier {
    writer: io::BufWriter<File>,
}

impl Applier for LogApplier {
    fn apply(&mut self, entry: &StateLogEntry) {
        barnum_state::write_entry(&mut self.writer, entry)
            .expect("failed to write state log entry");
    }
}
```

**`RunState`** — the in-memory state that is fully derivable from the log. This is the "source of truth" for what tasks exist and what state they're in.

```rust
struct RunState {
    /// All task state. Tasks not in this map are fully done.
    /// BTreeMap ordering = FIFO dispatch order (task IDs are monotonic).
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    /// Monotonic counter for assigning task IDs.
    next_task_id: u32,
}

impl Applier for RunState {
    fn apply(&mut self, entry: &StateLogEntry) {
        match entry {
            StateLogEntry::Config(_) => {}
            StateLogEntry::TaskSubmitted(s) => self.apply_submitted(s),
            StateLogEntry::TaskCompleted(c) => self.apply_completed(c),
        }
    }
}
```

### RunState methods

```rust
impl RunState {
    fn apply_submitted(&mut self, submitted: &TaskSubmitted) {
        // Advance ID counter
        self.next_task_id = self.next_task_id.max(submitted.task_id.0 + 1);

        self.tasks.insert(submitted.task_id, TaskEntry {
            step: submitted.step.clone(),
            parent_id: submitted.parent_id,
            state: TaskState::Pending {
                value: submitted.value.clone(),
            },
        });
    }

    fn apply_completed(&mut self, completed: &TaskCompleted) {
        match &completed.outcome {
            TaskOutcome::Success(success) => {
                if success.spawned_task_ids.is_empty() {
                    // Leaf task — remove and notify parent
                    self.remove_and_notify_parent(completed.task_id);
                } else {
                    // Has children — transition to WaitingForChildren.
                    // pending_children_count is derived: it equals
                    // spawned_task_ids.len() at this moment. As children
                    // complete, apply_completed will decrement it.
                    let count = success.spawned_task_ids.len();
                    let entry = self.tasks.get_mut(&completed.task_id)
                        .expect("completed task must exist");
                    entry.state = TaskState::WaitingForChildren {
                        pending_children_count: NonZeroU16::new(count as u16)
                            .expect("spawned_task_ids is non-empty"),
                        finally_value: success.finally_value.clone(),
                    };
                }
            }
            TaskOutcome::Failed(failed) => {
                self.remove_and_notify_parent(completed.task_id);
                // If retry_task_id is Some, the retry's TaskSubmitted
                // will arrive separately — no special handling here.
                let _ = failed;
            }
        }
    }

    /// Remove a task and decrement its parent's pending_children_count.
    /// If parent reaches zero children, recursively remove it too
    /// (scheduling its finally hook if configured).
    fn remove_and_notify_parent(&mut self, task_id: LogTaskId) {
        let parent_id = self.tasks.get(&task_id)
            .and_then(|e| e.parent_id);
        self.tasks.remove(&task_id);

        if let Some(pid) = parent_id {
            if let Some(parent) = self.tasks.get_mut(&pid) {
                if let TaskState::WaitingForChildren { pending_children_count, .. } = &mut parent.state {
                    match NonZeroU16::new(pending_children_count.get() - 1) {
                        Some(n) => *pending_children_count = n,
                        None => {
                            // All children done. Remove parent
                            // (finally scheduling happens in TaskRunner,
                            // not here — RunState is pure data).
                            self.tasks.remove(&pid);
                            // Cascade: parent's parent may now be done too
                            if let Some(grandparent_id) = self.tasks.get(&pid)
                                .and_then(|e| e.parent_id) {
                                // ... same logic, but in practice this
                                // doesn't cascade because finally tasks
                                // are separate TaskSubmitted entries
                            }
                        }
                    }
                }
            }
        }
    }
}
```

### TaskRunner owns the runtime, not the state

```rust
/// BEFORE: TaskRunner owns everything
struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    tasks: BTreeMap<LogTaskId, TaskEntry>,       // ← state
    pool: PoolConnection,
    max_concurrency: usize,
    in_flight: usize,
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,
    next_task_id: u32,                            // ← state
    state_log: io::BufWriter<std::fs::File>,      // ← applier
}

/// AFTER: State and appliers are separate from runtime machinery
struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    pool: PoolConnection,
    max_concurrency: usize,

    // Transient runtime (not persisted, not in RunState)
    in_flight: usize,
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,

    // Persisted state + appliers
    appliers: Vec<Box<dyn Applier>>,
    state: RunState,
}
```

**What moved:**
- `tasks` and `next_task_id` → `RunState` (an `Applier`)
- `state_log` → `LogApplier` (an `Applier`)
- `in_flight` stays on `TaskRunner` — it's transient runtime state tied to thread handles

**Why `RunState` is both an `Applier` AND a field on `TaskRunner`:**

`TaskRunner` needs to read `RunState` (to query pending tasks, check parent-child relationships, look up task entries). It also needs `RunState` to be in the applier chain so `apply()` updates it. We could do either:

(a) `RunState` is a field on `TaskRunner`, and `apply()` calls `self.state.apply()` explicitly in addition to iterating the appliers vec.

(b) `RunState` is stored only in the appliers vec and accessed through downcasting.

Option (a) is cleaner — `RunState` is privileged because it's the one applier the runner actually reads from. The other appliers are write-only sinks.

```rust
impl TaskRunner<'_> {
    fn apply(&mut self, entry: StateLogEntry) {
        // RunState is always updated
        self.state.apply(&entry);
        // Other appliers (log writer, metrics, etc.) are notified
        for applier in &mut self.appliers {
            applier.apply(&entry);
        }
    }
}
```

This means `appliers: Vec<Box<dyn Applier>>` contains `LogApplier` and any optional extras, but NOT `RunState`. `RunState` is a direct field.

### TaskEntry (cleaned up)

```rust
/// BEFORE
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    finally_script: Option<HookScript>,  // ← config concern, not state
    state: TaskState,
    retries_remaining: u32,              // ← dead code
}

/// AFTER
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    state: TaskState,
}
```

`finally_script` and `retries_remaining` are looked up from config at dispatch time, not stored in the entry. The entry stores only what's needed to reconstruct from the log.

### TaskState (cleaned up)

```rust
/// BEFORE
enum TaskState {
    Pending { value: StepInputValue },
    InFlight(InFlight),
    WaitingForChildren {
        pending_children_count: NonZeroU16,
        finally_data: Option<(HookScript, StepInputValue)>,
    },
}

/// AFTER
enum TaskState {
    /// Waiting to be dispatched. Only state that survives in RunState.
    Pending { value: StepInputValue },
    /// All children completed action, waiting for children to finish.
    WaitingForChildren {
        pending_children_count: NonZeroU16,
        /// Value for the finally hook (from the pre-hook-transformed input).
        /// None if the step has no finally hook.
        finally_value: StepInputValue,
    },
}
```

**`InFlight` is gone from `TaskState`.** InFlight is tracked separately by `TaskRunner` via the thread handles and `in_flight` counter. `RunState` doesn't know about it — from RunState's perspective, a dispatched task is still `Pending` until `TaskCompleted` arrives.

Wait — that creates a problem: on resume, RunState would see dispatched-but-not-completed tasks as `Pending` and they'd get re-dispatched. That's actually fine. That's exactly what we want on resume. InFlight tasks whose results were lost get re-dispatched. The log only records Submitted and Completed; anything in between is transient.

**`finally_data` simplified to `finally_value`.** The `HookScript` is looked up from config when the finally fires, not stored in state. Only the value (the pre-hook-transformed input) needs to persist so the finally hook gets the right input on resume.

## Resume

Resume is not a special mode. It's just reading a file and calling `apply()`.

```rust
pub fn resume(old_log: &Path, new_log: &Path, runner_config: &RunnerConfig) -> io::Result<()> {
    // 1. Build an empty RunState and a LogApplier pointing at the new file
    let mut state = RunState::new();
    let mut log_applier = LogApplier::new(new_log)?;

    // 2. Read old log. Feed each entry through both.
    //    Config entry is first — extract it.
    let mut config_json = None;
    for entry in barnum_state::read_entries(File::open(old_log)?) {
        if let StateLogEntry::Config(ref c) = entry {
            config_json = Some(c.config.clone());
        }
        state.apply(&entry);
        log_applier.apply(&entry);
    }

    // 3. Build runner with reconstructed state
    let config: Config = serde_json::from_value(config_json.expect("log must start with Config"))?;
    let schemas = CompiledSchemas::compile(&config)?;

    let mut runner = TaskRunner {
        config: &config,
        schemas: &schemas,
        step_map: config.step_map(),
        pool: PoolConnection::new(runner_config),
        max_concurrency: config.max_concurrency.unwrap_or(DEFAULT_MAX_CONCURRENCY),
        in_flight: 0,
        tx_rx: mpsc::channel(),
        appliers: vec![Box::new(log_applier)],
        state,
    };

    // 4. All Pending tasks get dispatched. Normal execution continues.
    run_to_completion(&mut runner)
}
```

**Why this works:** `RunState.apply()` and `LogApplier.apply()` are the same code paths used during live execution. After replaying N entries, the state is identical to what it would have been at that point during the original run. The new log file is an exact copy of the old one. New events from the resumed run append to it.

**No special replay mode.** No flags, no conditionals. Just `apply()` in a loop.

## Before/After

### queue_task

```rust
// BEFORE
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>, origin: TaskOrigin) {
    let id = self.next_task_id();

    self.log_writer.write(TaskSubmitted { task_id: id, step: task.step.clone(), ... });
    self.tasks.insert(id, TaskEntry { ... });

    if self.in_flight < self.max_concurrency {
        self.dispatch(id);
    }
}

// AFTER
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>, origin: TaskOrigin) {
    let id = LogTaskId(self.state.next_task_id);

    self.apply(StateLogEntry::TaskSubmitted(TaskSubmitted {
        task_id: id,
        step: task.step.clone(),
        value: task.value,
        parent_id,
        origin,
    }));

    if self.in_flight < self.max_concurrency {
        self.dispatch(id);
    }
}
```

### task_completed (success)

```rust
// BEFORE
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    self.in_flight -= 1;
    // ... manually log ...
    self.log_writer.write(TaskCompleted { ... });
    // ... manually update state ...
    self.tasks.get_mut(&task_id).unwrap().state = TaskState::WaitingForChildren { ... };
}

// AFTER
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    self.in_flight -= 1;

    // Queue children first
    let spawned_ids: Vec<LogTaskId> = spawned.into_iter().map(|task| {
        let id = LogTaskId(self.state.next_task_id);
        self.apply(StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: id,
            step: task.step,
            value: task.value,
            parent_id: Some(task_id),
            origin: TaskOrigin::Spawned,
        }));
        id
    }).collect();

    // Complete the parent
    self.apply(StateLogEntry::TaskCompleted(TaskCompleted {
        task_id,
        outcome: TaskOutcome::Success(TaskSuccess {
            spawned_task_ids: spawned_ids,
            finally_value: value,
        }),
    }));
}
```

### dispatch (NOT through apply — transient state only)

```rust
fn dispatch(&mut self, task_id: LogTaskId) {
    let entry = self.state.tasks.get(&task_id).expect("task must exist");
    let value = match &entry.state {
        TaskState::Pending { value } => value.clone(),
        _ => unreachable!("can only dispatch Pending tasks"),
    };

    self.in_flight += 1;
    let tx = self.tx.clone();
    // ... spawn thread ...

    // Note: we do NOT call apply() or modify RunState here.
    // RunState still shows this task as Pending. On resume,
    // it will be re-dispatched. That's correct.
}
```

## Testing

```rust
// RunState in isolation (pure state machine, no I/O)
#[test] fn apply_submitted_creates_pending_entry()
#[test] fn apply_submitted_advances_next_task_id()
#[test] fn apply_completed_success_no_children_removes_task()
#[test] fn apply_completed_success_with_children_transitions_to_waiting()
#[test] fn apply_completed_child_decrements_parent_count()
#[test] fn apply_completed_last_child_removes_parent()
#[test] fn apply_completed_failed_with_retry_removes_task()
#[test] fn apply_completed_failed_no_retry_notifies_parent()

// LogApplier in isolation
#[test] fn log_applier_writes_ndjson_entries()

// Integration: replay produces identical state
#[test] fn replay_log_reconstructs_identical_run_state()
#[test] fn replay_copies_all_entries_to_new_log()
```

## Migration Path

1. Extract `RunState` struct from `TaskRunner` (move `tasks`, `next_task_id`)
2. Implement `Applier for RunState` (extract logic from existing state mutation code)
3. Extract `LogApplier` struct (wraps existing `BufWriter<File>`)
4. Add `appliers: Vec<Box<dyn Applier>>` to `TaskRunner`, keep `state: RunState` as direct field
5. Replace all direct `self.tasks.insert(...)` / `self.log_writer.write(...)` with `self.apply(...)`
6. Remove `InFlight` from `TaskState` — track in `TaskRunner` only
7. Remove `finally_script` and `retries_remaining` from `TaskEntry` — look up from config
8. Verify all tests pass
9. Delete dead code
