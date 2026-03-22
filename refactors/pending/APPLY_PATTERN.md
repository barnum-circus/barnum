# Apply Pattern for State/Log Consistency

**Status:** Not started

**Depends on:** None

## Motivation

State changes and log writes are separate operations scattered throughout `TaskRunner`. Nothing enforces that they stay in sync:

```rust
self.state_log.write(TaskSubmitted { ... });
self.tasks.insert(id, entry);
```

This creates bugs: missed log writes when state changes, missed state updates when logs are written, and resume logic that silently diverges from live logic.

## Architecture

Barnum's current loop (`crates/barnum_config/src/runner/mod.rs:951`):

```rust
impl Iterator for TaskRunner<'_> {
    type Item = TaskResult;
    fn next(&mut self) -> Option<Self::Item> {
        self.dispatch_all_pending();
        if self.in_flight == 0 { return None; }
        let result = self.rx.recv().ok()?;
        Some(self.process_result(result))
    }
}
```

The structure is already there: receive a result, process it. `process_result` interprets the result (runs post hooks, determines success/failure/retry), then calls `task_succeeded` or `task_failed` which manually write the log and mutate state as separate operations.

### Design invariant: IDs come from entries

Every applier sees task IDs only through `StateLogEntry` values. Appliers read IDs from the entries they receive; they never allocate IDs independently. RunState owns the ID counter (`next_task_id: u32`) and advances it past any ID it sees in an entry via `max(current, entry.task_id + 1)`. Only the Engine allocates new IDs — for children, retries, and finally tasks.

Workers produce only `TaskCompleted` entries, which reference an existing task ID (no allocation needed). The Engine processes completions, allocates IDs for any resulting children/retries, and produces `TaskSubmitted` entries.

### Target event loop

The coordinator owns a `Receiver<StateLogEntry>` and a `Vec<Box<dyn Applier>>`. It receives a single entry from the channel, wraps it in a slice, and passes it to every applier via `process_entries`. That's it — the coordinator has no knowledge of RunState, config, or any other internal detail.

```rust
enum RunMode {
    Fresh { initial_tasks: Vec<Task> },
    Resume { old_log_path: PathBuf },
}

pub fn run(mode: RunMode, runner_config: &RunnerConfig) -> io::Result<()> {
    let (config, run_state, seed) = match mode {
        RunMode::Fresh { initial_tasks } => {
            let config = /* loaded by caller or passed separately */;
            let seed = build_seed_entries(&config, &initial_tasks);
            (config, RunState::new(), seed)
        }
        RunMode::Resume { old_log_path } => {
            let (config, run_state) = replay_log(&old_log_path)?;
            (config, run_state, vec![])
        }
    };

    let (tx, rx) = mpsc::channel();

    let mut appliers: Vec<Box<dyn Applier>> = vec![
        Box::new(Engine::new(&config, run_state, runner_config, tx)),
        Box::new(LogApplier::new(&runner_config.state_log_path)?),
    ];

    process_entries(&mut appliers, &seed);

    while let Ok(entry) = rx.recv() {
        process_entries(&mut appliers, &[entry]);
    }

    Ok(())
}

fn process_entries(appliers: &mut [Box<dyn Applier>], entries: &[StateLogEntry]) {
    for applier in appliers {
        applier.apply(entries);
    }
}
```

`tx` moves into the Engine — no clone, no coordinator-side sender. Workers hold `tx` clones (given by the Engine when it spawns them). The Engine sends each produced entry individually on `tx`. When all senders are dropped, `rx.recv()` returns `Err` and the loop exits.

## StateLogEntry

```rust
enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
}

struct TaskSubmitted {
    task_id: LogTaskId,
    step: StepName,
    value: StepInputValue,
    origin: TaskOrigin,
}

enum TaskOrigin {
    /// Initial task — no parent.
    Seed,
    /// Spawned by a successful parent task.
    Spawned { parent_id: LogTaskId },
    /// Retry of a failed task. Inherits the failed task's parent.
    Retry { replaces: LogTaskId, parent_id: Option<LogTaskId> },
    /// Finally task for a completed parent. Child of the parent's parent.
    Finally { finally_for: LogTaskId, parent_id: Option<LogTaskId> },
}

impl TaskOrigin {
    fn parent_id(&self) -> Option<LogTaskId> {
        match self {
            Self::Seed => None,
            Self::Spawned { parent_id } => Some(*parent_id),
            Self::Retry { parent_id, .. } | Self::Finally { parent_id, .. } => *parent_id,
        }
    }
}

struct TaskCompleted {
    task_id: LogTaskId,
    outcome: TaskOutcome,
}

enum TaskOutcome {
    Success(TaskSuccess),
    Failed(TaskFailed),
}

struct TaskSuccess {
    spawned: Vec<TaskSpec>,
    finally_value: StepInputValue,
}

struct TaskFailed {
    reason: FailureReason,
}

struct TaskSpec {
    step: StepName,
    value: StepInputValue,
}
```

`TaskSuccess` carries task specs (step + value), not allocated IDs. The worker doesn't allocate IDs — it just reports what children to spawn. The Engine allocates IDs when it produces `TaskSubmitted` entries for each spec. The parent transitions to `WaitingForChildren` with `pending_children_count` set from `spawned.len()` — it doesn't need child IDs, just the count. Children carry their parent via `TaskOrigin::Spawned { parent_id }`; when they complete, they decrement the parent's count.

`TaskFailed` carries only the failure reason. The Engine decides whether to retry based on step config and the original task's step/value (already in RunState). A retry produces a `TaskSubmitted` with `TaskOrigin::Retry { replaces, parent_id }` — inheriting the failed task's parent.

`parent_id` lives inside `TaskOrigin`, not as a separate field. Each origin variant carries exactly the relationship information it needs: `Seed` has none, `Spawned` always has a parent, `Retry` and `Finally` may or may not (root tasks have no parent).

Each variant records a fact. The Engine derives task removal internally when all children of a parent complete.

## Applier

```rust
trait Applier {
    fn apply(&mut self, entries: &[StateLogEntry]);
}
```

The coordinator calls `process_entries` which passes entries to `apply()` on each applier. No other methods on the trait.

### Engine

Owns the full execution lifecycle: task state, dispatch, and entry production. Holds a `Sender<StateLogEntry>` to feed entries back to the coordinator channel.

```rust
struct Engine<'a> {
    state: RunState,
    config: &'a Config,
    tx: Option<Sender<StateLogEntry>>,
    pool: PoolConnection,
    in_flight: usize,
    max_concurrency: usize,
    pending_dispatches: VecDeque<PendingTask>,
    dispatched: HashSet<LogTaskId>,
    pending_removals: Vec<LogTaskId>,
}

struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    next_task_id: u32,
    /// Parent task IDs whose children are all done. The parent entry
    /// is still in the map (in WaitingForChildren state with count 0)
    /// until explicitly removed.
    removed_parents: Vec<LogTaskId>,
}

struct PendingTask {
    task_id: LogTaskId,
    step: StepName,
    value: StepInputValue,
}
```

**`apply()`**: The Engine receives ALL state updates through entries — it never mutates its own state outside of `apply()`. Produced entries (children, retries, finally tasks) go on `tx`, come back through the channel, and are applied in a future call to `apply()`. Duplicate `TaskSubmitted` or unknown `TaskCompleted` entries are logic bugs that panic.

```rust
fn apply(&mut self, entries: &[StateLogEntry]) {
    let mut produced = Vec::new();

    // 1. Process incoming entries
    for entry in entries {
        match entry {
            StateLogEntry::TaskSubmitted(s) => {
                assert!(!self.state.tasks.contains_key(&s.task_id),
                    "[P035] duplicate TaskSubmitted for {:?}", s.task_id);
                self.state.apply_submitted(s);
                self.pending_dispatches.push_back(PendingTask {
                    task_id: s.task_id,
                    step: s.step.clone(),
                    value: s.value.clone(),
                });
            }
            StateLogEntry::TaskCompleted(c) => {
                assert!(self.state.tasks.contains_key(&c.task_id),
                    "[P036] TaskCompleted for unknown task {:?}", c.task_id);
                produced.extend(self.process_completion(c));
                assert!(self.dispatched.remove(&c.task_id),
                    "[P037] TaskCompleted for non-dispatched task {:?}", c.task_id);
                self.in_flight -= 1;
            }
            StateLogEntry::Config(_) => {}
        }
    }

    // 2. Process deferred parent removals from previous cycle.
    //    These were deferred because the finally entries they produced
    //    needed to be applied first (to increment grandparent counts).
    for parent_id in self.pending_removals.drain(..) {
        self.state.remove_and_notify_parent(parent_id);
    }

    // 3. Produce finally entries for all newly removed parents
    //    (from completions in step 1 and removals in step 2).
    //    Parents with finally scripts get deferred removal (pending_removals).
    //    Parents without finally scripts are removed immediately (may cascade).
    produced.extend(self.produce_finally_entries());

    // 4. Send produced entries on tx — they come back through the channel
    //    and are applied in a future call to apply()
    if let Some(tx) = &self.tx {
        for entry in produced {
            tx.send(entry).expect("[P031] channel open");
        }
    }

    // 5. Flush dispatches
    self.flush_dispatches();
}
```

**`process_completion()`**: Handles the result interpretation that today lives in `process_result`. For success, delegates to `apply_completed` (shared with replay) and produces child entries. For failure, the Engine handles state directly — it decides whether to retry based on config, not from anything on the log entry. Does NOT apply produced entries — they go on `tx`, come back through the channel, and are applied in a future `apply()` call.

```rust
fn process_completion(&mut self, completed: &TaskCompleted) -> Vec<StateLogEntry> {
    match &completed.outcome {
        TaskOutcome::Success(success) => {
            self.state.apply_completed(completed);
            success.spawned.iter().map(|spec| {
                let id = self.state.next_id();
                StateLogEntry::TaskSubmitted(TaskSubmitted {
                    task_id: id,
                    step: spec.step.clone(),
                    value: spec.value.clone(),
                    origin: TaskOrigin::Spawned { parent_id: completed.task_id },
                })
            }).collect()
        }
        TaskOutcome::Failed(_) => {
            let entry = self.state.tasks.get(&completed.task_id)
                .expect("[P038] completed task must exist");
            let parent_id = entry.parent_id;
            let step = entry.step.clone();
            let value = match &entry.state {
                TaskState::Pending { value } => value.clone(),
                _ => panic!("[P039] failed task not in Pending state"),
            };

            if self.should_retry(&step) {
                // Retry: remove task without parent notification.
                // The retry inherits the failed task's parent.
                self.state.tasks.remove(&completed.task_id);
                let id = self.state.next_id();
                vec![StateLogEntry::TaskSubmitted(TaskSubmitted {
                    task_id: id,
                    step,
                    value,
                    origin: TaskOrigin::Retry {
                        replaces: completed.task_id,
                        parent_id,
                    },
                })]
            } else {
                // Permanent failure: remove and notify parent.
                self.state.remove_and_notify_parent(completed.task_id);
                vec![]
            }
        }
    }
}
```

**`produce_finally_entries()`**: Drains `removed_parents` and produces finally entries. Does NOT apply the produced entries — they go on `tx`, come back, and are applied in a future `apply()` call.

For parents **with** a finally script: produce the `TaskSubmitted` entry and **defer** the parent removal by pushing to `pending_removals`. The removal must wait until the finally entry has been applied (next cycle), because `apply_submitted` increments the grandparent's child count. If we removed the parent now, the grandparent's count could hit zero prematurely.

For parents **without** a finally script: remove immediately via `remove_and_notify_parent`. This may cascade (adding more entries to `removed_parents`), which the while loop picks up.

```rust
fn produce_finally_entries(&mut self) -> Vec<StateLogEntry> {
    let mut entries = Vec::new();
    while let Some(parent_id) = self.state.removed_parents.pop() {
        let parent = self.state.tasks.get(&parent_id)
            .expect("[P040] removed parent must still be in map");
        let finally_value = match &parent.state {
            TaskState::WaitingForChildren { finally_value, .. } => finally_value.clone(),
            _ => panic!("[P041] removed parent not in WaitingForChildren state"),
        };
        let grandparent_id = parent.parent_id;
        let step = parent.step.clone();

        let script = self.config.step_map.get(&step)
            .and_then(|s| s.finally.as_ref());
        if let Some(script) = script {
            let id = self.state.next_id();
            entries.push(StateLogEntry::TaskSubmitted(TaskSubmitted {
                task_id: id,
                step: script.step.clone(),
                value: finally_value,
                origin: TaskOrigin::Finally {
                    finally_for: parent_id,
                    parent_id: grandparent_id,
                },
            }));
            self.pending_removals.push(parent_id);
        } else {
            // No finally script — remove immediately, may cascade
            self.state.remove_and_notify_parent(parent_id);
        }
    }
    entries
}
```

**`flush_dispatches()`**: Spawns worker threads. Each worker gets a `tx` clone and the step config.

```rust
fn flush_dispatches(&mut self) {
    let Some(tx) = &self.tx else { return };

    while self.in_flight < self.max_concurrency {
        let Some(task) = self.pending_dispatches.pop_front() else { break };
        self.in_flight += 1;
        self.dispatched.insert(task.task_id);
        let tx = tx.clone();
        // spawn worker thread with task, step config, tx
    }

    if self.pending_dispatches.is_empty() && self.in_flight == 0 {
        self.tx = None; // drop sender → channel closes when workers finish
    }
}
```

**Workers**: Each worker thread:

1. Runs the task via the pool
2. Interprets the result (`process_submit_result`, post hooks) — step config captured in closure
3. Produces a single `TaskCompleted` entry (no ID allocation)
4. Sends on `tx`, drops `tx` clone

**RunState internals**:

```rust
impl RunState {
    fn next_id(&mut self) -> LogTaskId {
        let id = LogTaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }

    fn apply_submitted(&mut self, submitted: &TaskSubmitted) {
        let parent_id = submitted.origin.parent_id();
        self.next_task_id = self.next_task_id.max(submitted.task_id.0 + 1);
        self.tasks.insert(submitted.task_id, TaskEntry {
            step: submitted.step.clone(),
            parent_id,
            state: TaskState::Pending {
                value: submitted.value.clone(),
            },
        });
        // Finally tasks increment their parent's child count.
        // Spawned children don't — their count is set upfront in apply_completed.
        // Retry children don't — the Engine removed the failed task without
        // decrementing the parent count, so no increment is needed.
        if let TaskOrigin::Finally { parent_id: Some(gp), .. } = &submitted.origin {
            self.increment_pending_children(*gp);
        }
    }

    /// Used by both the Engine (for success) and replay (for all outcomes).
    /// For failures during live operation, the Engine handles state directly
    /// in process_completion instead of calling this.
    fn apply_completed(&mut self, completed: &TaskCompleted) {
        match &completed.outcome {
            TaskOutcome::Success(success) => {
                if success.spawned.is_empty() {
                    self.remove_and_notify_parent(completed.task_id);
                } else {
                    let count = success.spawned.len();
                    let entry = self.tasks.get_mut(&completed.task_id)
                        .expect("[P033] completed task must exist");
                    entry.state = TaskState::WaitingForChildren {
                        pending_children_count: NonZeroU16::new(count as u16)
                            .expect("[P034] spawned is non-empty"),
                        finally_value: success.finally_value.clone(),
                    };
                }
            }
            TaskOutcome::Failed(_) => {
                // Just remove the task. No parent notification.
                // During replay: if a retry follows, the TaskSubmitted(Retry)
                // takes its place (same parent_id). If no retry follows,
                // the replay cleanup reconciles parent counts.
                self.tasks.remove(&completed.task_id);
            }
        }
    }
}
```

`remove_and_notify_parent` is unchanged from EXTRACT_RUN_STATE (non-recursive, accumulates into `removed_parents`).

### LogApplier

```rust
struct LogApplier {
    writer: io::BufWriter<File>,
}

impl Applier for LogApplier {
    fn apply(&mut self, entries: &[StateLogEntry]) {
        for entry in entries {
            barnum_state::write_entry(&mut self.writer, entry)
                .expect("[P032] failed to write state log entry");
        }
    }
}
```

Created after replay so it never writes replayed entries. Writes every entry it receives.

### Termination

Worker threads hold `Sender` clones. They drop them after sending results. The Engine drops its `tx` when `pending_dispatches` is empty and `in_flight == 0`. With all senders dropped, `rx.recv()` returns `Err` and the coordinator loop exits.

### TaskEntry and TaskState

```rust
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    state: TaskState,
}

enum TaskState {
    Pending { value: StepInputValue },
    WaitingForChildren {
        pending_children_count: NonZeroU16,
        finally_value: StepInputValue,
    },
}
```

TaskState has two variants. The current `InFlight` variant is replaced by `in_flight: usize` + `dispatched: HashSet<LogTaskId>` on the Engine. `finally_script` and `retries_remaining` are removed from TaskEntry — the Engine looks up the finally script from config when building finally entries, and the Engine determines whether to retry based on step config.

## Replay

Replay runs before the event loop and constructs RunState directly, bypassing the Applier trait:

```rust
fn replay_log(path: &Path) -> io::Result<(Config, RunState)> {
    let entries = barnum_state::read_entries(path);
    let mut state = RunState::new();
    let mut config_json = None;
    for entry in entries {
        match &entry {
            StateLogEntry::Config(c) => config_json = Some(c.config.clone()),
            StateLogEntry::TaskSubmitted(s) => state.apply_submitted(s),
            StateLogEntry::TaskCompleted(c) => state.apply_completed(c),
        }
    }

    // Cleanup phase 1: drain removed_parents accumulated from successes.
    // During the original run, finally entries were applied before parent
    // removal. During replay, the finally entries are already in the log
    // (applied above), so we just need the removals.
    while let Some(parent_id) = state.removed_parents.pop() {
        state.remove_and_notify_parent(parent_id);
    }

    // Cleanup phase 2: reconcile parent counts after failures.
    // During replay, apply_completed for failures just removes the task
    // without notifying the parent. Retries re-register as children
    // (same parent_id), so the parent's child set is correct. But
    // permanent failures leave parent counts stale. Scan for parents
    // whose children are all gone and trigger their removal cascade.
    loop {
        let removable: Vec<LogTaskId> = state.tasks.iter()
            .filter(|(_, entry)| matches!(&entry.state, TaskState::WaitingForChildren { .. }))
            .filter(|(task_id, _)| {
                !state.tasks.values().any(|e| e.parent_id == Some(**task_id))
            })
            .map(|(id, _)| *id)
            .collect();
        if removable.is_empty() { break; }
        for id in removable {
            state.remove_and_notify_parent(id);
        }
        while let Some(parent_id) = state.removed_parents.pop() {
            state.remove_and_notify_parent(parent_id);
        }
    }

    let config = /* deserialize config_json */;
    Ok((config, state))
}
```

After replay:
- `RunState` contains only active tasks (pending + waiting-for-children with live children)
- `next_task_id` is past all replayed IDs (advanced by `apply_submitted`)
- `removed_parents` is empty (drained by both cleanup phases)
- `RunMode::Resume` returns empty seed — `Engine::new()` initializes `pending_dispatches` from RunState's pending tasks, and `process_entries(&[])` flushes them
- `LogApplier` is created at the current log position (appending)

## Phasing

Each phase is a separate branch that passes CI and merges independently.

### Phase 0: Data structure cleanup

Independent sub-refactors, each in its own file. Can land in any order.

- **0a.** `EXTRACT_RUN_STATE.md` — **Done.** Moved `tasks` and `next_task_id` into RunState. Also moved `remove_and_notify_parent` onto RunState with deferred parent removal via non-recursive cascade (absorbs 0d).
- **0b.** `REMOVE_INFLIGHT_VARIANT.md` (not yet written) — Replace InFlight TaskState variant with `in_flight: usize` counter on TaskRunner.
- **0c.** `REMOVE_CONFIG_FROM_TASK_ENTRY.md` (not yet written) — Drop `finally_script` and `retries_remaining` from TaskEntry. Look them up from `step_map` when needed.
- **0d.** Absorbed into 0a (EXTRACT_RUN_STATE Phase 2).

### Phase 1: Event loop restructure

**Depends on: None (can run in parallel with Phase 0).**

Convert the Iterator-based loop to an explicit `run()` method with a recv loop. `process_result` still handles everything internally, and the Applier trait isn't introduced yet. The only change is the loop shape: `while let Ok(result) = self.rx.recv() { self.process_result(result); }`.

### Phase 2: Apply pattern

**Depends on: Phase 0, Phase 1.**

Introduce the `Applier` trait. Engine and LogApplier implement it. Build a `Vec<Box<dyn Applier>>`. The coordinator becomes the event loop described above.

### Phase 3: Seeding through apply

**Depends on: Phase 2.**

Restructure `run()` so seed entries go through `process_entries` directly (not through the channel). `build_seed_entries` produces entries, `process_entries` feeds them through all appliers.

## Before/After

### Task success: log and state are separate operations

Before (`runner/mod.rs:761`): `task_succeeded` manually writes the log, then separately mutates state. Miss either one and they diverge.

```rust
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    self.write_log(&StateLogEntry::TaskCompleted(...));  // 1. write log
    if let Some(hook) = finally_hook {
        self.schedule_finally(task_id, hook, value);     // 2. mutate state (finally)
    }
    self.remove_and_notify_parent(task_id);              // 3. mutate state (remove)
}
```

After: Workers produce `TaskCompleted` entries. Those entries flow through all appliers — Engine allocates IDs for children and sends `TaskSubmitted` entries on tx, LogApplier writes to disk. State and log see the same entries and can never diverge.

### Main loop: scattered responsibilities

Before (`runner/mod.rs:951`): Iterator trait with dispatch and processing mixed together.

```rust
impl Iterator for TaskRunner<'_> {
    type Item = TaskResult;
    fn next(&mut self) -> Option<Self::Item> {
        self.dispatch_all_pending();
        if self.in_flight == 0 { return None; }
        let result = self.rx.recv().ok()?;
        Some(self.process_result(result))
    }
}
```

After: the coordinator is a dumb loop.

```rust
while let Ok(entry) = rx.recv() {
    process_entries(&mut appliers, &[entry]);
}
```

### TaskEntry: config fields on every entry

Before (`runner/mod.rs:77`):

```rust
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    finally_script: Option<HookScript>,  // config
    state: TaskState,
    retries_remaining: u32,              // config
}
```

After:

```rust
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    state: TaskState,
}
```

`finally_script` and `retries_remaining` are looked up from config when needed.

## Testing

```rust
// RunState
#[test] fn apply_submitted_creates_pending_entry()
#[test] fn apply_submitted_derives_parent_id_from_origin()
#[test] fn apply_submitted_advances_next_task_id()
#[test] fn apply_submitted_increments_parent_count_for_finally_only()
#[test] fn apply_completed_success_no_children_removes_task()
#[test] fn apply_completed_success_with_children_transitions_to_waiting()
#[test] fn apply_completed_child_decrements_parent_count()
#[test] fn apply_completed_last_child_captures_removed_parent()
#[test] fn apply_completed_recursive_removal_up_tree()
#[test] fn apply_completed_failed_removes_task_without_parent_notification()

// Engine
#[test] fn apply_submitted_updates_state_and_queues_dispatch()
#[test] fn apply_submitted_panics_on_duplicate_id()
#[test] fn apply_completed_produces_child_entries_on_tx()
#[test] fn apply_completed_panics_on_unknown_task()
#[test] fn apply_completed_panics_on_non_dispatched_task()
#[test] fn retry_removes_task_without_parent_notification()
#[test] fn retry_entry_inherits_failed_tasks_parent()
#[test] fn permanent_failure_removes_and_notifies_parent()
#[test] fn produced_entries_not_applied_until_received_from_channel()
#[test] fn pending_removals_processed_after_incoming_entries()
#[test] fn finally_entry_defers_parent_removal()
#[test] fn no_finally_script_removes_parent_immediately()
#[test] fn deferred_removal_cascades_after_finally_applied()
#[test] fn produce_finally_looks_up_parent_from_map()
#[test] fn flush_dispatches_up_to_max_concurrency()
#[test] fn flush_drops_tx_when_empty_and_no_in_flight()

// LogApplier
#[test] fn writes_all_entry_variants()

// Coordinator
#[test] fn process_entries_feeds_all_appliers()
#[test] fn event_loop_exits_when_channel_closes()

// Replay
#[test] fn replay_reconstructs_state_from_log()
#[test] fn replay_advances_next_task_id()
#[test] fn replay_drains_removed_parents_from_successes()
#[test] fn replay_reconciles_parent_counts_after_failures()
#[test] fn replay_handles_retry_followed_by_permanent_failure()
#[test] fn replay_cleanup_cascades_multi_level()
#[test] fn engine_dispatches_remaining_tasks_after_replay()

// Workers
#[test] fn worker_produces_completion_entry()
#[test] fn worker_determines_success_with_spawned_specs()
```
