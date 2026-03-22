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

The channel carries individual `StateLogEntry` values — each message is exactly one entry. Workers produce a single `TaskCompleted` (with children/retry embedded in `subsequent`) and send it on `tx`. The coordinator receives entries and passes them through all appliers via `process_entries`.

```rust
enum RunMode {
    Fresh { initial_tasks: Vec<Task> },
    Resume { old_log_path: PathBuf },
}

pub fn run(mode: RunMode, runner_config: &RunnerConfig) -> io::Result<()> {
    let seed = match mode {
        RunMode::Fresh { initial_tasks } => {
            let config = /* loaded by caller or passed separately */;
            build_seed_entries(&config, &initial_tasks)
        }
        RunMode::Resume { old_log_path } => {
            barnum_state::read_entries(&old_log_path)?
        }
    };

    let (tx, rx) = mpsc::channel::<StateLogEntry>();

    let mut appliers: Vec<Box<dyn Applier>> = vec![
        Box::new(Engine::new(runner_config, tx)),
        Box::new(LogApplier::new(&runner_config.state_log_path)?),
    ];

    // Seed is the initial entries (Fresh) or the entire old log (Resume).
    // Applied as one batch — Engine processes all entries before dispatching.
    process_entries(&mut appliers, &seed);

    while let Ok(entry) = rx.recv() {
        process_entries(&mut appliers, &[entry]);
    }

    Ok(())
}

fn process_entries(appliers: &mut [Box<dyn Applier>], entries: &[StateLogEntry]) {
    for applier in appliers.iter_mut() {
        applier.apply(entries);
    }
}
```

`tx` moves into the Engine — workers get clones when spawned. Each worker produces a single `TaskCompleted` (with children/retry in `subsequent`) and sends it on `tx`. When all senders are dropped (workers done, Engine drops its copy), `rx.recv()` returns `Err` and the loop exits.

Every source on the channel produces exactly one event: workers send one `TaskCompleted`, Engine sends individual `TaskSubmitted` entries for finally tasks. The coordinator wraps each received entry in `&[entry]` for the batch-based Applier interface.

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
    /// Tasks created as a consequence of this completion.
    /// For success: children (Spawned origin).
    /// For failure: retry (Retry origin), if applicable.
    subsequent: Vec<TaskSubmitted>,
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

`TaskSuccess` carries only the finally value. Children are in `subsequent` as `TaskSubmitted` entries with `origin: Spawned { parent_id }`. `apply_completed` uses `subsequent.len()` to set the parent's child count directly — no transient state needed.

`TaskFailed` carries only the failure reason. Workers decide whether to retry based on step config (captured in their closure). A retry is in `subsequent` as a `TaskSubmitted` with `TaskOrigin::Retry { replaces }`.

`TaskCompleted.subsequent` makes each completion self-contained. `apply_completed` processes the completion and inserts all subsequent tasks atomically — no ordering dependencies between separate entries, no transient states, no two-pass processing.

Each `TaskOrigin` variant carries only non-derivable information. `Spawned { parent_id }` needs the parent explicitly (no other way to know it). `Retry { replaces }` and `Finally { finally_for }` reference a task that's still in the map — `apply_submitted` derives `parent_id` from the referenced task's entry. `Seed` has no relationships.

## Applier

```rust
trait Applier {
    fn apply(&mut self, entries: &[StateLogEntry]);
}
```

Both Engine and LogApplier implement this trait. One method. The coordinator calls it on every applier for every batch of entries. Engine processes all entries in the batch before dispatching — no threads are spawned mid-batch.

### Engine

Owns the full execution lifecycle: task state, dispatch, and entry production. Holds a `Sender<StateLogEntry>` — workers get clones (they send their `TaskCompleted`), and the Engine sends cascade entries (individual finally `TaskSubmitted` entries).

Config is not passed to the constructor — it arrives as the first `StateLogEntry::Config` entry in the seed batch. Engine validates that Config is the first entry it receives and that there are no duplicates.

```rust
struct Engine {
    state: RunState,
    config: Option<Config>,
    tx: Option<Sender<StateLogEntry>>,
    id_counter: Arc<AtomicU32>,
    pool: PoolConnection,
    in_flight: usize,
    max_concurrency: usize,
    pending_dispatches: VecDeque<PendingTask>,
    dispatched: HashSet<LogTaskId>,
}

impl Engine {
    fn config(&self) -> &Config {
        self.config.as_ref().expect("[P051] config not set")
    }
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

`id_counter` is shared between Engine and workers (via `Arc<AtomicU32>`). Workers allocate IDs atomically for children and retries. Engine allocates IDs for finally tasks. `RunState.next_task_id` tracks the highest seen ID (advanced by `apply_submitted`'s `max(current, entry.task_id + 1)`) — used to initialize the shared counter after the seed batch is applied.

**`apply()`**: Processes a batch of entries, then produces cascade entries (sends on `tx`), then flushes dispatches. All entries in the batch are processed before any dispatch happens — this is critical for replay, where the entire old log is applied as one batch.

```rust
impl Applier for Engine {
    fn apply(&mut self, entries: &[StateLogEntry]) {
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
                    // Dispatch tracking: during replay, tasks weren't actually
                    // dispatched, so dispatched.remove returns false.
                    if self.dispatched.remove(&c.task_id) {
                        self.in_flight -= 1;
                    }
                    self.state.apply_completed(c);
                    // apply_completed processed subsequent tasks internally.
                    // Queue them for dispatch.
                    for s in &c.subsequent {
                        self.pending_dispatches.push_back(PendingTask {
                            task_id: s.task_id,
                            step: s.step.clone(),
                            value: s.value.clone(),
                        });
                    }
                }
                StateLogEntry::Config(c) => {
                    assert!(self.config.is_none(),
                        "[P052] duplicate Config entry");
                    assert!(self.state.tasks.is_empty(),
                        "[P053] Config must be first entry");
                    self.config = Some(c.deserialize());
                }
            }
        }

        // Handle cascaded parent completions. When a parent's children
        // are all done, it lands in removed_parents. For parents with a
        // finally script, produce the entry and send on tx. For parents
        // without, remove immediately (may cascade — the while loop
        // picks up further removals). During replay, parents may already
        // be removed by a Finally entry in the same batch — skip them.
        while let Some(parent_id) = self.state.removed_parents.pop() {
            let Some(parent) = self.state.tasks.get(&parent_id) else {
                continue; // Already removed by a replayed Finally entry
            };
            let finally_value = match &parent.state {
                TaskState::WaitingForChildren { finally_value, .. } =>
                    finally_value.clone(),
                TaskState::Pending { .. } | TaskState::Failed =>
                    panic!("[P041] removed parent not in WaitingForChildren"),
            };
            let step = parent.step.clone();

            let script = self.config().step_map.get(&step)
                .and_then(|s| s.finally.as_ref());
            if let Some(script) = script {
                let id = self.next_id();
                let entry = StateLogEntry::TaskSubmitted(TaskSubmitted {
                    task_id: id,
                    step: script.step.clone(),
                    value: finally_value,
                    origin: TaskOrigin::Finally { finally_for: parent_id },
                });
                if let Some(tx) = &self.tx {
                    tx.send(entry).expect("[P050] channel send failed");
                }
            } else {
                // No finally script — remove immediately, may cascade
                self.state.remove_and_notify_parent(parent_id);
            }
        }

        self.flush_dispatches();
    }
}
```

Cascade entries go on `tx` as individual messages and arrive as subsequent entries in the coordinator loop. Each is wrapped in `&[entry]` and flows through all appliers. Engine does NOT apply cascade entries to its own state when producing them — that happens when they come back through the channel.

**`flush_dispatches()`**: Spawns worker threads. Each worker gets a `tx` clone, an `id_counter` clone, and the step config.

```rust
fn flush_dispatches(&mut self) {
    let Some(tx) = &self.tx else { return };

    while self.in_flight < self.max_concurrency {
        let Some(task) = self.pending_dispatches.pop_front() else { break };
        // Skip tasks no longer in Pending state (completed during replay).
        if !self.state.tasks.get(&task.task_id)
            .map_or(false, |e| matches!(&e.state, TaskState::Pending { .. })) {
            continue;
        }
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
4. Produces a single `TaskCompleted` with children/retry in `subsequent`
5. Sends `StateLogEntry::TaskCompleted(...)` on `tx`, drops `tx` clone

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
                // Parent already transitioned to WaitingForChildren by
                // apply_completed (which set the count from subsequent.len()).
                // Just verify it exists and is in the expected state.
                let parent = self.tasks.get(parent_id)
                    .expect("[P046] spawned child's parent must exist");
                assert!(matches!(&parent.state,
                    TaskState::WaitingForChildren { .. }),
                    "[P049] spawned child's parent not in WaitingForChildren state");
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
    /// and replay. Handles the completion and all subsequent tasks
    /// atomically. No transient states.
    fn apply_completed(&mut self, completed: &TaskCompleted) {
        let entry = self.tasks.get_mut(&completed.task_id)
            .expect("[P033] completed task must exist");
        assert!(matches!(&entry.state, TaskState::Pending { .. }),
            "[P034] completed task not in Pending state");

        match &completed.outcome {
            TaskOutcome::Success(success) => {
                if !completed.subsequent.is_empty() {
                    entry.state = TaskState::WaitingForChildren {
                        pending_children_count: NonZeroU16::new(
                            completed.subsequent.len() as u16
                        ).expect("[P047] non-empty subsequent"),
                        finally_value: success.finally_value.clone(),
                    };
                }
                // Empty subsequent → leaf task, removed below.
            }
            TaskOutcome::Failed(_) => {
                if !completed.subsequent.is_empty() {
                    // Retry follows. Mark as Failed so retry's
                    // apply_submitted can find and remove it.
                    entry.state = TaskState::Failed;
                }
                // Empty subsequent → permanent failure, removed below.
            }
        }

        // Process subsequent tasks (children or retry).
        for submitted in &completed.subsequent {
            self.apply_submitted(submitted);
        }

        // Remove leaf successes and permanent failures immediately.
        if completed.subsequent.is_empty() {
            self.remove_and_notify_parent(completed.task_id);
        }
    }
}
```

`remove_and_notify_parent` is unchanged from EXTRACT_RUN_STATE (non-recursive, accumulates into `removed_parents`). During replay, a task's parent may have been removed by a Finally entry's `apply_submitted`. If `parent_id` refers to a task not in the map, `remove_and_notify_parent` skips the notification silently — the parent was already handled.

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

Writes every entry it receives, including replayed entries during Resume. The new log is a complete record — it starts with a copy of the old log (replayed as part of the seed batch) and then appends new entries from live execution.

### Termination

Worker threads hold `Sender<StateLogEntry>` clones (one per worker). They drop them after sending. The Engine drops its `tx` when `pending_dispatches` is empty and `in_flight == 0`. With all senders dropped, `rx.recv()` returns `Err` and the coordinator loop exits.

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
    /// Task failed and a retry follows. Transient: only exists between
    /// apply_completed setting it and retry's apply_submitted removing
    /// the task (both happen within the same apply_completed call).
    Failed,
}
```

TaskState has three variants. `Pending` and `WaitingForChildren` are the stable live states. `Failed` is transient: it exists only when a retry follows (non-empty `subsequent`), and the retry's `apply_submitted` removes the task within the same `apply_completed` call. There is no `Succeeded` variant — `apply_completed` uses `TaskCompleted.subsequent` to transition directly to `WaitingForChildren(N)` for tasks with children, or to remove leaf tasks and permanent failures immediately.

The current `InFlight` variant is replaced by `in_flight: usize` + `dispatched: HashSet<LogTaskId>` on the Engine. `finally_script` and `retries_remaining` are removed from TaskEntry — the Engine looks up the finally script from config when building finally entries, and workers determine whether to retry based on step config captured in their closures.

## Replay

There is no separate replay function. Resume reads the old log and uses it as the seed:

```rust
RunMode::Resume { old_log_path } => {
    barnum_state::read_entries(&old_log_path)?
}
```

The old entries flow through `process_entries` like any other batch. The first entry is `Config` — Engine deserializes and stores it. Subsequent entries build up RunState. After the batch, `flush_dispatches` dispatches any remaining Pending tasks. `flush_dispatches` checks each task's state before dispatching — tasks that completed during replay are skipped.

No separate cleanup phases are needed. `apply_completed` handles everything atomically via `subsequent`: leaf successes and permanent failures are removed immediately, children transition the parent to `WaitingForChildren`, and retries replace the failed task. The same code path handles both live execution and replay.

`LogApplier` writes all replayed entries to the new log file, producing a complete record. New entries from live execution are appended after.

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

After: Workers produce a single `TaskCompleted` (with children in `subsequent`). That entry flows through both Engine and LogApplier — state and log see the same entries and can never diverge.

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
// RunState — apply_submitted
#[test] fn apply_submitted_creates_pending_entry()
#[test] fn apply_submitted_derives_parent_id_from_origin()
#[test] fn apply_submitted_advances_next_task_id()
#[test] fn apply_submitted_spawned_verifies_parent_in_waiting()
#[test] fn apply_submitted_finally_removes_parent_and_inherits_grandparent()
#[test] fn apply_submitted_retry_removes_failed_task()
#[test] fn apply_submitted_retry_inherits_parent_id()

// RunState — apply_completed
#[test] fn apply_completed_with_children_transitions_to_waiting()
#[test] fn apply_completed_leaf_success_removes_immediately()
#[test] fn apply_completed_permanent_failure_removes_immediately()
#[test] fn apply_completed_failure_with_retry_marks_failed()
#[test] fn apply_completed_inserts_subsequent_tasks()
#[test] fn apply_completed_child_decrements_parent_count()
#[test] fn apply_completed_last_child_captures_removed_parent()
#[test] fn apply_completed_cascades_removal_up_tree()

// Engine — apply()
#[test] fn apply_submitted_queues_dispatch()
#[test] fn apply_submitted_panics_on_duplicate_id()
#[test] fn apply_completed_queues_subsequent_for_dispatch()
#[test] fn apply_completed_panics_on_unknown_task()
#[test] fn apply_cascade_sends_finally_entry_on_tx()
#[test] fn apply_cascade_removes_parent_without_finally_immediately()
#[test] fn apply_cascade_skips_already_removed_parent()
#[test] fn flush_dispatches_up_to_max_concurrency()
#[test] fn flush_dispatches_skips_completed_tasks()
#[test] fn flush_drops_tx_when_empty_and_no_in_flight()

// Engine — replay via seed batch
#[test] fn replay_seed_reconstructs_state()
#[test] fn replay_seed_dispatches_remaining_pending_tasks()
#[test] fn replay_seed_skips_completed_tasks_in_dispatch()
#[test] fn replay_seed_produces_finally_for_cascaded_parents()

// LogApplier
#[test] fn writes_all_entry_variants()

// Coordinator
#[test] fn process_entries_calls_all_appliers()
#[test] fn event_loop_exits_when_channel_closes()

// Workers
#[test] fn worker_produces_task_completed_with_subsequent()
#[test] fn worker_allocates_ids_from_shared_counter()
```
