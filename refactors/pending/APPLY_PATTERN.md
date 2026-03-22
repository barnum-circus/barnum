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

Every applier sees task IDs only through `StateLogEntry` values. Appliers read IDs from the entries they receive; they never allocate IDs independently. RunState owns `next_task_id: u32` and advances it past any ID it sees in an entry via `max(current, entry.task_id + 1)` — this is used during replay.

During live execution, IDs are allocated from a shared `Arc<AtomicU32>` counter. Workers allocate IDs for children and retries. The Engine allocates IDs for finally tasks. The counter guarantees uniqueness across concurrent allocators.

### Target event loop

The channel carries `Vec<StateLogEntry>`. Workers produce entries directly (allocating IDs via a shared `Arc<AtomicU32>`) and send them on `tx`. The coordinator receives entry batches and passes them through all appliers via `process_entries`.

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

    let (tx, rx) = mpsc::channel::<Vec<StateLogEntry>>();

    let mut appliers: Vec<Box<dyn Applier>> = vec![
        Box::new(Engine::new(&config, run_state, runner_config, tx)),
        Box::new(LogApplier::new(&runner_config.state_log_path)?),
    ];

    process_entries(&mut appliers, &seed);

    while let Ok(entries) = rx.recv() {
        process_entries(&mut appliers, &entries);
    }

    Ok(())
}

fn process_entries(appliers: &mut [Box<dyn Applier>], entries: &[StateLogEntry]) {
    for applier in appliers.iter_mut() {
        applier.apply(entries);
    }
}
```

`tx` moves into the Engine — workers get clones when spawned. Workers produce full `StateLogEntry` batches (TaskCompleted + children's TaskSubmitted + retry's TaskSubmitted) and send them on `tx`. When all senders are dropped (workers done, Engine drops its copy), `rx.recv()` returns `Err` and the loop exits.

If an applier needs to trigger follow-up events (e.g., Engine producing finally entries from cascading), it sends them on `tx`. They arrive as the next (or a subsequent) message in the coordinator loop and flow through all appliers like any other batch.

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
    /// Retry of a failed task. parent_id derived from the replaced task.
    Retry { replaces: LogTaskId },
    /// Finally task for a removed parent. parent_id derived from
    /// the finally_for task's parent (the grandparent).
    Finally { finally_for: LogTaskId },
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
    finally_value: StepInputValue,
}

struct TaskFailed {
    reason: FailureReason,
}
```

`TaskSuccess` carries only the finally value. It has no information about children — children are separate `TaskSubmitted` entries with `origin: Spawned { parent_id }`. The parent's child count is derived from actual entries, not from a redundant count on the success.

`TaskFailed` carries only the failure reason. Workers decide whether to retry based on step config (captured in their closure). A retry produces a `TaskSubmitted` with `TaskOrigin::Retry { replaces }` — `apply_submitted` derives the parent_id by looking up the replaced task in the map.

Each `TaskOrigin` variant carries only non-derivable information. `Spawned { parent_id }` needs the parent explicitly (no other way to know it). `Retry { replaces }` and `Finally { finally_for }` reference a task that's still in the map — `apply_submitted` derives `parent_id` from the referenced task's entry. `Seed` has no relationships.

Each variant records a fact. The Engine derives task removal internally when all children of a parent complete.

## Applier

```rust
trait Applier {
    fn apply(&mut self, entries: &[StateLogEntry]);
}
```

Both Engine and LogApplier implement this trait. One method. The coordinator calls it on every applier for every batch of entries.

### Engine

Owns the full execution lifecycle: task state, dispatch, and entry production. Holds a `Sender<Vec<StateLogEntry>>` — workers get clones (they send entry batches), and the Engine sends cascade entries (finally tasks).

```rust
struct Engine<'a> {
    state: RunState,
    config: &'a Config,
    tx: Option<Sender<Vec<StateLogEntry>>>,
    id_counter: Arc<AtomicU32>,
    pool: PoolConnection,
    in_flight: usize,
    max_concurrency: usize,
    pending_dispatches: VecDeque<PendingTask>,
    dispatched: HashSet<LogTaskId>,
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

`id_counter` is shared between Engine and workers (via `Arc<AtomicU32>`). Workers allocate IDs atomically for children and retries. Engine allocates IDs for finally tasks. `RunState.next_task_id` is only used during replay (advanced by `apply_submitted`'s `max(current, entry.task_id + 1)`).

**`apply()`**: Applies entries to state, produces cascade entries (sends on `tx`), and flushes dispatches. Duplicate `TaskSubmitted` or unknown `TaskCompleted` entries are logic bugs that panic.

```rust
impl Applier for Engine<'_> {
    fn apply(&mut self, entries: &[StateLogEntry]) {
        self.apply_batch(entries);
        let finally_entries = self.produce_finally_entries();
        if !finally_entries.is_empty() {
            if let Some(tx) = &self.tx {
                tx.send(finally_entries)
                    .expect("[P050] channel send failed");
            }
        }
        self.flush_dispatches();
    }
}
```

Cascade entries go on `tx` and arrive as a subsequent message in the coordinator loop. They flow through all appliers like any other batch. Engine does NOT apply cascade entries to its own state when producing them — that happens when they come back through the channel.

**`apply_batch()`**: Processes a batch of entries: applies to state, tracks completions, and cleans up transient states. Duplicate `TaskSubmitted` or unknown `TaskCompleted` entries are logic bugs that panic.

```rust
fn apply_batch(&mut self, entries: &[StateLogEntry]) {
    let mut just_completed = Vec::new();

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
                self.state.apply_completed(c);
                just_completed.push(c.task_id);
                assert!(self.dispatched.remove(&c.task_id),
                    "[P037] TaskCompleted for non-dispatched task {:?}", c.task_id);
                self.in_flight -= 1;
            }
            StateLogEntry::Config(_) => {}
        }
    }

    // Clean up transient states from this batch.
    // Tasks still in Succeeded are leaf tasks (no children submitted).
    // Tasks still in Failed are permanent failures (no retry submitted).
    // Both need removal — if children/retry were in the same batch,
    // apply_submitted already transitioned them out of these states.
    for task_id in just_completed {
        if let Some(entry) = self.state.tasks.get(&task_id) {
            match &entry.state {
                TaskState::Succeeded { .. } | TaskState::Failed => {
                    self.state.remove_and_notify_parent(task_id);
                }
                TaskState::Pending { .. } => {
                    panic!("[P046] completed task still in Pending state");
                }
                TaskState::WaitingForChildren { .. } => {} // children submitted
            }
        } // else: already removed (Retry's apply_submitted removed it)
    }
}
```

**`produce_finally_entries()`**: Drains `removed_parents` and produces finally entries. The entries go on `tx` and come back through the coordinator loop.

For parents **with** a finally script: produce the `TaskSubmitted` entry. The parent is NOT removed here — `apply_submitted` for the Finally origin removes it when the entry is applied (see RunState internals). The finally task replaces the parent as a child of the grandparent, so the grandparent's count doesn't change.

For parents **without** a finally script: remove immediately via `remove_and_notify_parent`. This may cascade (adding more entries to `removed_parents`), which the while loop picks up.

```rust
fn produce_finally_entries(&mut self) -> Vec<StateLogEntry> {
    let mut entries = Vec::new();
    while let Some(parent_id) = self.state.removed_parents.pop() {
        let parent = self.state.tasks.get(&parent_id)
            .expect("[P040] removed parent must still be in map");
        let finally_value = match &parent.state {
            TaskState::WaitingForChildren { finally_value, .. } => finally_value.clone(),
            TaskState::Pending { .. } | TaskState::Succeeded { .. } | TaskState::Failed => {
                panic!("[P041] removed parent not in WaitingForChildren state");
            }
        };
        let step = parent.step.clone();

        let script = self.config.step_map.get(&step)
            .and_then(|s| s.finally.as_ref());
        if let Some(script) = script {
            let id = self.next_id();
            entries.push(StateLogEntry::TaskSubmitted(TaskSubmitted {
                task_id: id,
                step: script.step.clone(),
                value: finally_value,
                origin: TaskOrigin::Finally { finally_for: parent_id },
            }));
        } else {
            // No finally script — remove immediately, may cascade
            self.state.remove_and_notify_parent(parent_id);
        }
    }
    entries
}
```

**`flush_dispatches()`**: Spawns worker threads. Each worker gets a `tx` clone, an `id_counter` clone, and the step config.

```rust
fn flush_dispatches(&mut self) {
    let Some(tx) = &self.tx else { return };

    while self.in_flight < self.max_concurrency {
        let Some(task) = self.pending_dispatches.pop_front() else { break };
        self.in_flight += 1;
        self.dispatched.insert(task.task_id);
        let tx = tx.clone();
        let id_counter = self.id_counter.clone();
        // spawn worker thread with task, step config, tx, id_counter
    }

    if self.pending_dispatches.is_empty() && self.in_flight == 0 {
        self.tx = None; // drop sender → channel closes when workers finish
    }
}
```

**Workers**: Each worker thread:

1. Runs the task via the pool
2. Interprets the result (`process_submit_result`, post hooks) — step config captured in closure
3. Allocates IDs for children/retries from the shared `id_counter`
4. Produces a `Vec<StateLogEntry>` batch (TaskCompleted + children's TaskSubmitted + retry's TaskSubmitted)
5. Sends the batch on `tx`, drops `tx` clone

**RunState internals**:

```rust
impl RunState {
    fn next_id(&mut self) -> LogTaskId {
        let id = LogTaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }

    fn apply_submitted(&mut self, submitted: &TaskSubmitted) {
        self.next_task_id = self.next_task_id.max(submitted.task_id.0 + 1);

        let parent_id = match &submitted.origin {
            TaskOrigin::Seed => None,
            TaskOrigin::Spawned { parent_id } => {
                // Transition parent: Succeeded → WaitingForChildren(1),
                // or increment existing WaitingForChildren count.
                let parent = self.tasks.get_mut(parent_id)
                    .expect("[P046] spawned child's parent must exist");
                match &mut parent.state {
                    TaskState::Succeeded { finally_value } => {
                        parent.state = TaskState::WaitingForChildren {
                            pending_children_count: NonZeroU16::new(1)
                                .expect("[P047] literal 1"),
                            finally_value: finally_value.clone(),
                        };
                    }
                    TaskState::WaitingForChildren { pending_children_count, .. } => {
                        *pending_children_count = NonZeroU16::new(
                            pending_children_count.get() + 1
                        ).expect("[P048] child count overflow");
                    }
                    TaskState::Pending { .. } | TaskState::Failed => {
                        panic!("[P049] spawned child's parent in {:?} state",
                            std::mem::discriminant(&parent.state));
                    }
                }
                Some(*parent_id)
            }
            TaskOrigin::Retry { replaces } => {
                // Replace the failed task. Inherit its parent.
                let old = self.tasks.remove(replaces)
                    .expect("[P042] retry target must exist");
                assert!(matches!(old.state, TaskState::Failed),
                    "[P045] retry target not in Failed state");
                old.parent_id
            }
            TaskOrigin::Finally { finally_for } => {
                // Remove the finally_for task from the map. It's done —
                // its children are all complete. The finally task replaces
                // it as a child of the grandparent, so the grandparent's
                // count doesn't change (one child removed, one child added).
                let removed = self.tasks.remove(finally_for)
                    .expect("[P043] finally target must exist");
                assert!(matches!(removed.state,
                    TaskState::WaitingForChildren { .. }),
                    "[P044] finally target not in WaitingForChildren state");
                removed.parent_id
            }
        };

        self.tasks.insert(submitted.task_id, TaskEntry {
            step: submitted.step.clone(),
            parent_id,
            state: TaskState::Pending {
                value: submitted.value.clone(),
            },
        });
    }

    /// Called for every completion — both success and failure, both live
    /// and replay. Transitions to a transient state: Succeeded or Failed.
    /// These transient states are resolved by subsequent entries in the
    /// same batch (children's apply_submitted, retry's apply_submitted)
    /// or by cleanup (apply_batch transient cleanup, replay cleanup).
    fn apply_completed(&mut self, completed: &TaskCompleted) {
        let entry = self.tasks.get_mut(&completed.task_id)
            .expect("[P033] completed task must exist");
        assert!(matches!(&entry.state, TaskState::Pending { .. }),
            "[P034] completed task not in Pending state");
        match &completed.outcome {
            TaskOutcome::Success(success) => {
                entry.state = TaskState::Succeeded {
                    finally_value: success.finally_value.clone(),
                };
            }
            TaskOutcome::Failed(_) => {
                // Mark as failed. Don't remove — the task stays in the map.
                // If a retry follows, apply_submitted for the Retry origin
                // removes this entry and inherits its parent_id.
                // If no retry follows, cleanup removes it and notifies parent.
                entry.state = TaskState::Failed;
            }
        }
    }
}
```

`remove_and_notify_parent` is unchanged from EXTRACT_RUN_STATE (non-recursive, accumulates into `removed_parents`). During replay, a task's parent may have been removed by a Finally entry's `apply_submitted` before cleanup runs. If `parent_id` refers to a task not in the map, `remove_and_notify_parent` skips the notification silently — the parent was already handled.

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

Worker threads hold `Sender<Vec<StateLogEntry>>` clones. They drop them after sending. The Engine drops its `tx` when `pending_dispatches` is empty and `in_flight == 0`. With all senders dropped, `rx.recv()` returns `Err` and the coordinator loop exits.

### TaskEntry and TaskState

```rust
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    state: TaskState,
}

enum TaskState {
    Pending { value: StepInputValue },
    /// Task completed successfully. Transient state between
    /// apply_completed and either:
    /// - apply_submitted (Spawned) → transitions to WaitingForChildren
    /// - cleanup → leaf task, removed via remove_and_notify_parent
    Succeeded { finally_value: StepInputValue },
    WaitingForChildren {
        pending_children_count: NonZeroU16,
        finally_value: StepInputValue,
    },
    /// Task completed with failure. Transient state between
    /// apply_completed and either:
    /// - apply_submitted (Retry) → replaced by new task
    /// - cleanup → permanent failure, removed via remove_and_notify_parent
    Failed,
}
```

TaskState has four variants. `Pending` and `WaitingForChildren` are the stable live states. `Succeeded` and `Failed` are transient: `apply_completed` transitions to them, and they're resolved within the same batch. For success, child entries in the same batch transition the parent from `Succeeded` to `WaitingForChildren`; leaf tasks (no children) stay `Succeeded` and are cleaned up by `apply_batch`'s transient state cleanup. For failure, a retry entry in the same batch replaces the task; permanent failures stay `Failed` and are cleaned up the same way. During replay, the same pattern holds — subsequent entries resolve transient states, and replay cleanup handles anything left over.

The current `InFlight` variant is replaced by `in_flight: usize` + `dispatched: HashSet<LogTaskId>` on the Engine. `finally_script` and `retries_remaining` are removed from TaskEntry — the Engine looks up the finally script from config when building finally entries, and workers determine whether to retry based on step config captured in their closures.

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

    // Cleanup phase 1: remove leaf tasks and permanent failures.
    // Succeeded tasks with no children submitted are leaf tasks.
    // Failed tasks with no retry submitted are permanent failures.
    // Both need removal and parent notification.
    let transient: Vec<LogTaskId> = state.tasks.iter()
        .filter(|(_, e)| matches!(&e.state,
            TaskState::Succeeded { .. } | TaskState::Failed))
        .map(|(id, _)| *id)
        .collect();
    for id in transient {
        state.remove_and_notify_parent(id);
    }

    // Cleanup phase 2: drain removed_parents from phase 1 removals.
    // Parents whose finally entries are in the log were already removed
    // by apply_submitted(Finally) during replay — their children's
    // remove_and_notify_parent skipped the missing parent silently,
    // so these parents never appear in removed_parents.
    //
    // Parents that reach removed_parents here either:
    // (a) have no finally script, or
    // (b) had their finally entry interrupted before logging.
    // Both are removed directly. May cascade.
    while let Some(parent_id) = state.removed_parents.pop() {
        state.remove_and_notify_parent(parent_id);
    }

    let config = /* deserialize config_json */;
    Ok((config, state))
}
```

After replay:
- `RunState` contains only active tasks (pending + waiting-for-children with live children)
- `next_task_id` is past all replayed IDs (advanced by `apply_submitted`)
- `removed_parents` is empty (drained by both cleanup phases)
- `RunMode::Resume` returns empty seed — `Engine::new()` initializes `pending_dispatches` from RunState's pending tasks, and `apply_entries` with empty entries flushes them
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

Introduce the `Applier` trait, `Engine`, and `LogApplier`. The coordinator becomes `Vec<Box<dyn Applier>>` with the `process_entries` loop. Seed entries flow through `process_entries` like everything else.

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

After: Workers produce `StateLogEntry` batches directly (TaskCompleted + TaskSubmitted for children). Those entries flow through both Engine and LogApplier — state and log see the same entries and can never diverge.

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

After: the coordinator is a dumb loop over `Vec<Box<dyn Applier>>`.

```rust
while let Ok(entries) = rx.recv() {
    process_entries(&mut appliers, &entries);
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
#[test] fn apply_submitted_spawned_transitions_parent_to_waiting()
#[test] fn apply_submitted_spawned_increments_existing_child_count()
#[test] fn apply_submitted_finally_removes_parent_and_inherits_grandparent()
#[test] fn apply_submitted_retry_removes_failed_task()
#[test] fn apply_submitted_retry_inherits_parent_id()
#[test] fn apply_completed_success_transitions_to_succeeded()
#[test] fn apply_completed_failure_transitions_to_failed()
#[test] fn apply_completed_child_decrements_parent_count()
#[test] fn apply_completed_last_child_captures_removed_parent()
#[test] fn apply_completed_recursive_removal_up_tree()

// Engine — apply()
#[test] fn apply_submitted_queues_dispatch()
#[test] fn apply_submitted_panics_on_duplicate_id()
#[test] fn apply_completed_panics_on_unknown_task()
#[test] fn apply_completed_panics_on_non_dispatched_task()
#[test] fn apply_cleans_up_leaf_tasks()
#[test] fn apply_cleans_up_permanent_failures()
#[test] fn apply_does_not_clean_up_tasks_with_children()
#[test] fn apply_does_not_clean_up_retried_tasks()
#[test] fn produce_finally_removes_parent_without_script_immediately()
#[test] fn produce_finally_sends_entries_on_tx()
#[test] fn produce_finally_looks_up_parent_from_map()
#[test] fn flush_dispatches_up_to_max_concurrency()
#[test] fn flush_drops_tx_when_empty_and_no_in_flight()

// Engine — apply() integration
#[test] fn apply_updates_state()
#[test] fn apply_sends_finally_entries_on_tx()

// LogApplier
#[test] fn writes_all_entry_variants()

// Coordinator
#[test] fn process_entries_calls_all_appliers()
#[test] fn event_loop_exits_when_channel_closes()

// Replay
#[test] fn replay_reconstructs_state_from_log()
#[test] fn replay_advances_next_task_id()
#[test] fn replay_cleans_up_leaf_tasks()
#[test] fn replay_cleans_up_permanent_failures()
#[test] fn replay_drains_removed_parents()
#[test] fn replay_handles_retry_followed_by_permanent_failure()
#[test] fn replay_cleanup_skips_parent_removed_by_finally()
#[test] fn replay_cleanup_cascades_multi_level()
#[test] fn engine_dispatches_remaining_tasks_after_replay()

// Workers
#[test] fn worker_produces_entry_batch()
#[test] fn worker_produces_children_and_retry_entries()
```
