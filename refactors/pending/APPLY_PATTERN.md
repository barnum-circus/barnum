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

Workers produce `WorkerResult` values (not log entries), which reference an existing task ID (no allocation needed). The Engine processes worker results, allocates IDs for any resulting children/retries, and produces `StateLogEntry` values.

### Target event loop

The coordinator owns a `Receiver<WorkerResult>`, an `Engine`, and a `LogApplier`. It receives worker results from the channel, passes them to the Engine (which interprets them into log entries), then applies those entries through both the Engine and LogApplier. The `apply_entries` loop processes entries and any entries they produce (e.g., finally tasks) until no more are generated.

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

    let (worker_tx, worker_rx) = mpsc::channel();

    let mut engine = Engine::new(&config, run_state, runner_config, worker_tx);
    let mut log_applier = LogApplier::new(&runner_config.state_log_path)?;

    apply_entries(&mut engine, &mut log_applier, &seed);

    while let Ok(wr) = worker_rx.recv() {
        let entries = engine.process_worker_result(wr);
        apply_entries(&mut engine, &mut log_applier, &entries);
    }

    Ok(())
}

fn apply_entries(engine: &mut Engine, log: &mut LogApplier, entries: &[StateLogEntry]) {
    engine.apply(entries);
    log.apply(entries);
    let produced = engine.take_produced();
    if !produced.is_empty() {
        log.apply(&produced);
    }
}
```

`worker_tx` moves into the Engine — workers get clones when spawned. Engine.apply() handles all cascading internally (cleanup, finally entries, deferred removals) and accumulates produced entries. The coordinator feeds produced entries to LogApplier via `take_produced()`. When all senders are dropped (workers done, Engine drops its copy), `worker_rx.recv()` returns `Err` and the loop exits.

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

`TaskFailed` carries only the failure reason. The Engine decides whether to retry based on step config and the original task's step/value (already in RunState). A retry produces a `TaskSubmitted` with `TaskOrigin::Retry { replaces }` — `apply_submitted` derives the parent_id by looking up the replaced task in the map.

Each `TaskOrigin` variant carries only non-derivable information. `Spawned { parent_id }` needs the parent explicitly (no other way to know it). `Retry { replaces }` and `Finally { finally_for }` reference a task that's still in the map — `apply_submitted` derives `parent_id` from the referenced task's entry. `Seed` has no relationships.

Each variant records a fact. The Engine derives task removal internally when all children of a parent complete.

## Worker types

Workers communicate raw results to the coordinator — not log entries. The coordinator passes these to the Engine, which interprets them into `StateLogEntry` values.

```rust
struct WorkerResult {
    task_id: LogTaskId,
    outcome: WorkerOutcome,
}

enum WorkerOutcome {
    Success {
        spawned: Vec<TaskSpec>,
        finally_value: StepInputValue,
    },
    Failed {
        reason: FailureReason,
    },
}

struct TaskSpec {
    step: StepName,
    value: StepInputValue,
}
```

`WorkerOutcome::Success` carries spawned specs because the worker determines what children to create (from `process_submit_result` and post hooks). These specs are NOT logged — the Engine converts them into `TaskSubmitted` log entries with allocated IDs. The `TaskCompleted` log entry records only the `finally_value`; the children exist solely as their own `TaskSubmitted` entries.

## Applier

```rust
trait Applier {
    fn apply(&mut self, entries: &[StateLogEntry]);
}
```

Passive consumers of log entries. `LogApplier` implements this trait. The Engine does NOT — it has a different `apply()` signature that returns produced entries.

### Engine

Owns the full execution lifecycle: task state, dispatch, and entry production. Holds a `Sender<WorkerResult>` to give clones to workers.

```rust
struct Engine<'a> {
    state: RunState,
    config: &'a Config,
    worker_tx: Option<Sender<WorkerResult>>,
    pool: PoolConnection,
    in_flight: usize,
    max_concurrency: usize,
    pending_dispatches: VecDeque<PendingTask>,
    dispatched: HashSet<LogTaskId>,
    pending_removals: Vec<LogTaskId>,
    /// Entries produced during apply() (finally tasks).
    /// Drained by take_produced() after each apply() call.
    produced: Vec<StateLogEntry>,
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

**`apply()`**: Processes entries and handles all cascading effects internally (cleanup, finally entries, deferred removals). Produced entries accumulate in `self.produced` for the coordinator to feed to LogApplier via `take_produced()`. Duplicate `TaskSubmitted` or unknown `TaskCompleted` entries are logic bugs that panic.

```rust
fn apply(&mut self, entries: &[StateLogEntry]) {
    self.apply_batch(entries);
    self.resolve_cascades();
    self.flush_dispatches();
}

fn take_produced(&mut self) -> Vec<StateLogEntry> {
    std::mem::take(&mut self.produced)
}

/// Process a batch of entries: apply to state, track completions,
/// clean up transient states, and process deferred removals.
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

    // Process deferred parent removals from previous cycle.
    // These were deferred because the finally entries they produced
    // needed to be applied first (to increment grandparent counts).
    for parent_id in self.pending_removals.drain(..) {
        self.state.remove_and_notify_parent(parent_id);
    }
}

/// Produce finally entries and process them until no more cascades.
/// Each produced finally entry is applied to state (via apply_batch)
/// and accumulated in self.produced for LogApplier.
fn resolve_cascades(&mut self) {
    loop {
        let finally_entries = self.produce_finally_entries();
        if finally_entries.is_empty() { break; }
        self.apply_batch(&finally_entries);
        self.produced.extend(finally_entries);
    }
}
```

**`process_worker_result()`**: Called by the coordinator when a worker result arrives. Interprets the raw result into log entries. Reads task state BEFORE any transitions (the entries it produces will trigger transitions when `apply()` processes them). This is the only place that reads `WorkerOutcome` data.

```rust
fn process_worker_result(&mut self, wr: WorkerResult) -> Vec<StateLogEntry> {
    let entry = self.state.tasks.get(&wr.task_id)
        .expect("[P038] worker result for unknown task");
    assert!(matches!(&entry.state, TaskState::Pending { .. }),
        "[P039] worker result for task not in Pending state");

    match wr.outcome {
        WorkerOutcome::Success { spawned, finally_value } => {
            let mut entries = vec![StateLogEntry::TaskCompleted(TaskCompleted {
                task_id: wr.task_id,
                outcome: TaskOutcome::Success(TaskSuccess {
                    finally_value,
                }),
            })];
            for spec in spawned {
                let id = self.state.next_id();
                entries.push(StateLogEntry::TaskSubmitted(TaskSubmitted {
                    task_id: id,
                    step: spec.step,
                    value: spec.value,
                    origin: TaskOrigin::Spawned { parent_id: wr.task_id },
                }));
            }
            entries
        }
        WorkerOutcome::Failed { reason } => {
            let step = entry.step.clone();
            let value = match &entry.state {
                TaskState::Pending { value } => value.clone(),
                TaskState::Succeeded { .. } | TaskState::WaitingForChildren { .. }
                    | TaskState::Failed => unreachable!(), // asserted above
            };

            let mut entries = vec![StateLogEntry::TaskCompleted(TaskCompleted {
                task_id: wr.task_id,
                outcome: TaskOutcome::Failed(TaskFailed { reason }),
            })];

            if self.should_retry(&step) {
                let id = self.state.next_id();
                entries.push(StateLogEntry::TaskSubmitted(TaskSubmitted {
                    task_id: id,
                    step,
                    value,
                    origin: TaskOrigin::Retry { replaces: wr.task_id },
                }));
            }
            // If no retry, apply() will detect the Failed state and
            // call remove_and_notify_parent.

            entries
        }
    }
}
```

**`produce_finally_entries()`**: Drains `removed_parents` and produces finally entries. Called by `resolve_cascades`, which applies the returned entries via `apply_batch` and loops until no more are produced.

For parents **with** a finally script: produce the `TaskSubmitted` entry and **defer** the parent removal by pushing to `pending_removals`. The removal must wait until the finally entry has been applied (by `apply_batch` in the next `resolve_cascades` iteration), because `apply_submitted` increments the grandparent's child count. If we removed the parent now, the grandparent's count could hit zero prematurely.

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
                origin: TaskOrigin::Finally { finally_for: parent_id },
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

**`flush_dispatches()`**: Spawns worker threads. Each worker gets a `worker_tx` clone and the step config.

```rust
fn flush_dispatches(&mut self) {
    let Some(worker_tx) = &self.worker_tx else { return };

    while self.in_flight < self.max_concurrency {
        let Some(task) = self.pending_dispatches.pop_front() else { break };
        self.in_flight += 1;
        self.dispatched.insert(task.task_id);
        let tx = worker_tx.clone();
        // spawn worker thread with task, step config, tx
    }

    if self.pending_dispatches.is_empty() && self.in_flight == 0 {
        self.worker_tx = None; // drop sender → channel closes when workers finish
    }
}
```

**Workers**: Each worker thread:

1. Runs the task via the pool
2. Interprets the result (`process_submit_result`, post hooks) — step config captured in closure
3. Produces a single `WorkerResult` (no ID allocation)
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
                // Child of the grandparent (finally_for's parent).
                let parent_entry = self.tasks.get(finally_for)
                    .expect("[P043] finally target must exist");
                let grandparent_id = parent_entry.parent_id;
                // Increment grandparent's child count. The grandparent
                // is always in WaitingForChildren (it has at least one
                // child — the finally_for task).
                // Retry doesn't increment — the failed task's parent
                // count was never decremented.
                if let Some(gp) = grandparent_id {
                    self.increment_pending_children(gp);
                }
                grandparent_id
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
    /// or by cleanup (Engine.apply step 2, replay cleanup).
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

Worker threads hold `Sender<WorkerResult>` clones. They drop them after sending results. The Engine drops its `worker_tx` when `pending_dispatches` is empty and `in_flight == 0`. With all senders dropped, `worker_rx.recv()` returns `Err` and the coordinator loop exits.

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

TaskState has four variants. `Pending` and `WaitingForChildren` are the stable live states. `Succeeded` and `Failed` are transient: `apply_completed` transitions to them, and they're resolved within the same `apply_entries` loop iteration. For success, child entries in the same batch transition the parent from `Succeeded` to `WaitingForChildren`; leaf tasks (no children) stay `Succeeded` and are cleaned up in Engine.apply() step 2. For failure, a retry entry in the same batch replaces the task; permanent failures stay `Failed` and are cleaned up the same way. During replay, the same pattern holds — subsequent entries resolve transient states, and replay cleanup handles anything left over.

The current `InFlight` variant is replaced by `in_flight: usize` + `dispatched: HashSet<LogTaskId>` on the Engine. `finally_script` and `retries_remaining` are removed from TaskEntry — the Engine looks up the finally script from config when building finally entries, and the Engine determines whether to retry based on step config.

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

    // Cleanup phase 2: drain removed_parents from completions and
    // phase 1 removals above. During the original run, finally entries
    // were applied before parent removal. During replay, the finally
    // entries are already in the log (applied above), so we just need
    // the removals. May cascade.
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

Introduce the `Applier` trait for passive consumers (LogApplier). Separate the Engine as the active state machine with `process_worker_result()` and `apply() -> Vec<StateLogEntry>`. The coordinator becomes the event loop described above.

### Phase 3: Seeding through apply

**Depends on: Phase 2.**

Restructure `run()` so seed entries go through `apply_entries` directly. `build_seed_entries` produces entries, `apply_entries` feeds them through Engine and LogApplier.

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

After: Workers produce `WorkerResult` values. The Engine interprets them into `StateLogEntry` values (TaskCompleted + TaskSubmitted for children). Those entries flow through both Engine and LogApplier — state and log see the same entries and can never diverge.

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
while let Ok(wr) = worker_rx.recv() {
    let entries = engine.process_worker_result(wr);
    apply_entries(&mut engine, &mut log_applier, &entries);
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
#[test] fn apply_submitted_increments_grandparent_count_for_finally()
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
#[test] fn pending_removals_processed_after_cleanup()
#[test] fn finally_entry_defers_parent_removal()
#[test] fn no_finally_script_removes_parent_immediately()
#[test] fn deferred_removal_cascades_after_finally_applied()
#[test] fn produce_finally_looks_up_parent_from_map()
#[test] fn flush_dispatches_up_to_max_concurrency()
#[test] fn flush_drops_worker_tx_when_empty_and_no_in_flight()

// Engine — process_worker_result()
#[test] fn success_produces_completed_and_submitted_entries()
#[test] fn success_no_children_produces_only_completed()
#[test] fn failure_with_retry_produces_completed_and_submitted()
#[test] fn failure_without_retry_produces_only_completed()
#[test] fn process_worker_result_panics_on_unknown_task()

// LogApplier
#[test] fn writes_all_entry_variants()

// Coordinator
#[test] fn apply_entries_feeds_produced_to_log_applier()
#[test] fn event_loop_exits_when_channel_closes()

// Replay
#[test] fn replay_reconstructs_state_from_log()
#[test] fn replay_advances_next_task_id()
#[test] fn replay_cleans_up_leaf_tasks()
#[test] fn replay_cleans_up_permanent_failures()
#[test] fn replay_drains_removed_parents()
#[test] fn replay_handles_retry_followed_by_permanent_failure()
#[test] fn replay_cleanup_cascades_multi_level()
#[test] fn engine_dispatches_remaining_tasks_after_replay()

// Workers
#[test] fn worker_produces_worker_result()
#[test] fn worker_determines_success_with_spawned_specs()
```
