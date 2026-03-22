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

Every applier sees task IDs only through `StateLogEntry` values. Appliers read IDs from the entries they receive; they never allocate IDs independently.

During live execution, IDs are allocated from a shared `Arc<AtomicU32>` counter. Workers allocate IDs for children and retries. The counter guarantees uniqueness across concurrent allocators. During `apply()`, a local `max_seen_id` records the last `TaskSubmitted` ID seen — IDs are monotonically increasing, so the last one is always the largest. After the batch, `id_counter` is set to at least `max_seen_id + 1` via `fetch_max`.

### Target event loop

```rust
type ChannelMsg = ControlFlow<WorkflowResult, StateLogEntry>;
```

The channel carries `ChannelMsg`. `Continue(entry)` is a normal entry — task workers send `TaskCompleted`, finally workers send `FinallyRun`. `Break(result)` is the shutdown signal — Engine sends `Break(Ok(()))` when the workflow completes successfully, `Break(Err(..))` when a task permanently fails (retries exhausted). The coordinator matches on the message: `Continue` entries flow through `process_entries`, `Break` exits the loop and logs the result. The workflow result is NOT written to the state log — it's derivable from the entries.

```rust
enum RunMode {
    Fresh(FreshMode),
    Resume(ResumeMode),
}

struct FreshMode {
    initial_tasks: Vec<Task>,
}

struct ResumeMode {
    old_log_path: PathBuf,
}

pub fn run(mode: RunMode, runner_config: &RunnerConfig) -> WorkflowResult {
    let seed = match mode {
        RunMode::Fresh(fresh) => {
            let config = /* loaded by caller or passed separately */;
            build_seed_entries(&config, &fresh.initial_tasks)
        }
        RunMode::Resume(resume) => {
            barnum_state::read_entries(&resume.old_log_path)?
        }
    };

    let (tx, rx) = mpsc::channel::<ChannelMsg>();

    let mut appliers: Vec<Box<dyn Applier>> = vec![
        Box::new(Engine::new(runner_config, tx)),
        Box::new(LogApplier::new(&runner_config.state_log_path)?),
    ];

    // Seed is the initial entries (Fresh) or the entire old log (Resume).
    // Applied as one batch — Engine processes all entries before dispatching.
    process_entries(&mut appliers, &seed);

    let result = loop {
        match rx.recv().expect("[P062] channel closed unexpectedly") {
            ControlFlow::Continue(entry) => {
                process_entries(&mut appliers, &[entry]);
            }
            ControlFlow::Break(result) => break result,
        }
    };

    // Log the workflow result. Not written to the state log —
    // it's derivable from the entries.
    log_workflow_result(&result);
    result
}

fn process_entries(appliers: &mut [Box<dyn Applier>], entries: &[StateLogEntry]) {
    for applier in appliers.iter_mut() {
        applier.apply(entries);
    }
}
```

`tx` moves into the Engine — workers get clones when spawned. Each worker produces a single `TaskCompleted` (with outcome: `Succeeded`, `Retrying`, or `PermanentFailure`) and sends it on `tx`. When all senders are dropped (workers done, Engine drops its copy), `rx.recv()` returns `Err` and the loop exits.

Every source sends `ChannelMsg` on the channel. Task workers send `Continue(TaskCompleted(...))`. Finally workers send `Continue(FinallyRun(...))`. Engine sends `Break(Ok(()))` or `Break(Err(..))` for shutdown. The coordinator matches on the message: `Continue` entries are wrapped in `&[entry]` for the batch-based Applier interface, `Break` exits the loop and returns the result.

## StateLogEntry

```rust
enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
    FinallyRun(FinallyRun),
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
    Spawned(SpawnedOrigin),
    /// Retry of a failed task. parent_id derived from the replaced task.
    Retry(RetryOrigin),
}

struct SpawnedOrigin {
    parent_id: LogTaskId,
}

struct RetryOrigin {
    replaces: LogTaskId,
}

struct TaskCompleted {
    task_id: LogTaskId,
    outcome: TaskOutcome,
}

/// Workers determine the outcome based on the result and step config
/// captured in their closure. Workers do NOT know about finallys —
/// that's an Engine concern via FinallyRun.
enum TaskOutcome {
    Succeeded(TaskSuccess),
    Failed(TaskFailure),
}

struct TaskSuccess {
    finally_value: StepInputValue,
    /// Children spawned by this task. Empty = leaf success.
    children: Vec<TaskSubmitted>,
}

struct TaskFailure {
    reason: FailureReason,
    /// Retry task, if retries remain. None = permanent failure.
    retry: Option<TaskSubmitted>,
}

/// Records that a finally script ran for a parent whose children all
/// completed. Produced by a finally worker thread — the Engine dispatches
/// the worker post-batch, the worker runs the script and sends FinallyRun
/// on tx. Present in the log = done, absent = re-dispatch on resume.
struct FinallyRun {
    finally_for: LogTaskId,
    children: Vec<TaskSubmitted>,
}
```

Each `TaskOutcome` variant is self-contained. `Succeeded` carries the finally value and children (empty vec for leaf tasks). `Failed` carries the reason and an optional retry. No impossible states — success can't have a retry, failure can't have children.

`apply_completed` matches on the outcome. For `Succeeded` with non-empty children, the parent transitions to `WaitingForChildren(N)`. For `Succeeded` with empty children or `Failed` with no retry, the task is removed and the Engine walks up the parent chain for finally detection. For `Failed` with a retry, the task is marked `Failed` and the retry's `apply_submitted` replaces it.

`FinallyRun` is produced by a finally worker thread. When a parent's children all complete and the parent has a finally script, the Engine queues a `PendingFinally` in `pending_dispatches`. `flush_dispatches` spawns a finally worker, same as it spawns task workers — both respect `max_concurrency`. The worker runs the script, allocates IDs for children, and sends `FinallyRun` on `tx`. It flows through all appliers like any other entry. During replay, when `apply()` encounters a `FinallyRun` entry, it removes the matching `PendingFinally` from `pending_dispatches` before calling `apply_finally_run`. This means any `PendingFinally` that reaches `flush_dispatches` is valid — if the parent is missing from the map, that's a bug (panic). On resume, if a parent's children are all done but no `FinallyRun` is in the log, the `PendingFinally` stays in the queue and `flush_dispatches` dispatches a new finally worker.

Each `TaskOrigin` variant carries only non-derivable information. `Spawned { parent_id }` needs the parent explicitly (no other way to know it). `Retry { replaces }` references a task that's still in the map — `apply_submitted` derives `parent_id` from the referenced task's entry. `Seed` has no relationships.

## Applier

```rust
trait Applier {
    fn apply(&mut self, entries: &[StateLogEntry]);
}
```

Both Engine and LogApplier implement this trait. One method. The coordinator calls it on every applier for every batch of entries. Engine processes all entries in the batch before dispatching — no threads are spawned mid-batch.

### Engine

Owns the full execution lifecycle: task state, dispatch, finally execution, and shutdown. Holds a `Sender<ChannelMsg>` — task workers get clones (they send `TaskCompleted`), finally workers get clones (they send `FinallyRun`), and the Engine sends `Break(result)` for shutdown.

Config is not passed to the constructor — it arrives as the first `StateLogEntry::Config` entry in the seed batch. Engine validates that Config is the first entry it receives and that there are no duplicates.

```rust
struct Engine {
    state: RunState,
    config: Option<Config>,
    tx: Sender<ChannelMsg>,
    id_counter: Arc<AtomicU32>,
    pool: PoolConnection,
    in_flight: usize,
    max_concurrency: usize,
    pending_dispatches: VecDeque<PendingDispatch>,
}

impl Engine {
    fn config(&self) -> &Config {
        self.config.as_ref().expect("[P051] config not set")
    }
}

enum PendingDispatch {
    Task(PendingTask),
    Finally(PendingFinally),
}

struct PendingTask {
    task_id: LogTaskId,
    step: StepName,
    value: StepInputValue,
}

struct PendingFinally {
    parent_id: LogTaskId,
    step: StepName,
    finally_value: StepInputValue,
}

struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
}
```

`id_counter` is shared between Engine and workers (via `Arc<AtomicU32>`). Workers allocate IDs atomically for children and retries. During `apply()`, a local `max_seen_id` records the last `TaskSubmitted` ID — IDs are monotonically increasing, so the last is always the largest. After the batch, `id_counter` is set to at least `max_seen_id + 1` via `fetch_max` — this initializes the counter correctly after the seed batch (replay). During live execution the counter is already past all seen IDs, so `fetch_max` is a no-op.

**`apply()`**: Processes a batch of entries, then dispatches via `flush_dispatches`. All entries in the batch are processed before any dispatch happens — this is critical for replay, where the entire old log is applied as one batch.

```rust
impl Applier for Engine {
    fn apply(&mut self, entries: &[StateLogEntry]) {
        let mut max_seen_id: u32 = 0;

        for entry in entries {
            match entry {
                StateLogEntry::TaskSubmitted(s) => {
                    max_seen_id = s.task_id.0;
                    assert!(!self.state.tasks.contains_key(&s.task_id),
                        "[P035] duplicate TaskSubmitted for {:?}", s.task_id);
                    self.state.apply_submitted(s);
                    self.pending_dispatches.push_back(
                        PendingDispatch::Task(PendingTask {
                            task_id: s.task_id,
                            step: s.step.clone(),
                            value: s.value.clone(),
                        }));
                }
                StateLogEntry::TaskCompleted(c) => {
                    assert!(self.state.tasks.contains_key(&c.task_id),
                        "[P036] TaskCompleted for unknown task {:?}", c.task_id);
                    self.in_flight = self.in_flight.saturating_sub(1);
                    // Remove the matching PendingTask — during replay, it
                    // was queued by a prior TaskSubmitted/TaskCompleted in
                    // this batch. During live execution, it was already
                    // dispatched (nothing to remove).
                    self.pending_dispatches.retain(|d| !matches!(d,
                        PendingDispatch::Task(pt) if pt.task_id == c.task_id));
                    let parent_id = self.state.apply_completed(c);
                    // For leaf/permanent-failure: walk up the parent chain.
                    if let Some(pid) = parent_id {
                        if let Some(pf) = self.state.walk_up_for_finally(
                            pid, self.config()
                        ) {
                            self.pending_dispatches.push_back(
                                PendingDispatch::Finally(pf));
                        }
                    }
                    // Queue children/retry for dispatch.
                    match &c.outcome {
                        TaskOutcome::Succeeded(success) => {
                            for s in &success.children {
                                self.pending_dispatches.push_back(
                                    PendingDispatch::Task(PendingTask {
                                        task_id: s.task_id,
                                        step: s.step.clone(),
                                        value: s.value.clone(),
                                    }));
                            }
                        }
                        TaskOutcome::Failed(failure) => {
                            if let Some(retry) = &failure.retry {
                                self.pending_dispatches.push_back(
                                    PendingDispatch::Task(PendingTask {
                                        task_id: retry.task_id,
                                        step: retry.step.clone(),
                                        value: retry.value.clone(),
                                    }));
                            }
                        }
                    }
                }
                StateLogEntry::FinallyRun(f) => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    // Remove the matching PendingFinally — it was queued
                    // earlier in this batch when walk_up_for_finally fired.
                    // During replay this prevents stale dispatch; during
                    // live execution there's nothing to remove (already
                    // dispatched).
                    self.pending_dispatches.retain(|d| !matches!(d,
                        PendingDispatch::Finally(pf)
                            if pf.parent_id == f.finally_for));
                    let grandparent_id = self.state.apply_finally_run(f);
                    // Queue children for dispatch.
                    for s in &f.children {
                        self.pending_dispatches.push_back(
                            PendingDispatch::Task(PendingTask {
                                task_id: s.task_id,
                                step: s.step.clone(),
                                value: s.value.clone(),
                            }));
                    }
                    // FinallyRun with no children may trigger another
                    // finally up the chain (grandparent reached zero).
                    if let Some(gp_id) = grandparent_id {
                        if let Some(pf) = self.state.walk_up_for_finally(
                            gp_id, self.config()
                        ) {
                            self.pending_dispatches.push_back(
                                PendingDispatch::Finally(pf));
                        }
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

        self.id_counter.fetch_max(max_seen_id + 1, Ordering::SeqCst);
        self.flush_dispatches();
    }
}
```

**`flush_dispatches()`**: Dispatches both task workers and finally workers from a single queue, respecting `max_concurrency` for both. Every entry in the queue is expected to be valid — stale entries are removed during entry processing (TaskCompleted removes PendingTask, FinallyRun removes PendingFinally). Panics if an entry is stale.

```rust
fn flush_dispatches(&mut self) {
    while self.in_flight < self.max_concurrency {
        let Some(dispatch) = self.pending_dispatches.pop_front() else { break };
        match dispatch {
            PendingDispatch::Task(task) => {
                let entry = self.state.tasks.get(&task.task_id)
                    .expect("[P064] PendingTask but task not in map");
                assert!(matches!(&entry.state, TaskState::Pending(..)),
                    "[P065] PendingTask for {:?} not in Pending state",
                    task.task_id);
                self.in_flight += 1;
                let tx = self.tx.clone();
                let id_counter = self.id_counter.clone();
                // spawn task worker thread with task, step config, tx, id_counter
            }
            PendingDispatch::Finally(pf) => {
                assert!(self.state.tasks.contains_key(&pf.parent_id),
                    "[P063] PendingFinally for {:?} but parent not in map",
                    pf.parent_id);
                self.in_flight += 1;
                let tx = self.tx.clone();
                let id_counter = self.id_counter.clone();
                // spawn finally worker thread with pf, finally script config,
                // tx, id_counter
            }
        }
    }

    if self.pending_dispatches.is_empty() && self.in_flight == 0 {
        let result = self.compute_workflow_result();
        self.tx.send(ControlFlow::Break(result))
            .expect("[P055] shutdown send failed");
    }
}
```

**Task workers**: Each task worker thread:

1. Runs the task via the pool
2. Interprets the result (`process_submit_result`, post hooks) — step config captured in closure
3. Allocates IDs for children/retries from the shared `id_counter`
4. Produces a single `TaskCompleted` with outcome: `Succeeded` or `Failed`
5. Sends `Continue(StateLogEntry::TaskCompleted(...))` on `tx`, drops `tx` clone

**Finally workers**: Each finally worker thread:

1. Runs the finally script via the pool with the `finally_value`
2. Allocates IDs for children from the shared `id_counter`
3. Produces a `FinallyRun` with the children
4. Sends `Continue(StateLogEntry::FinallyRun(...))` on `tx`, drops `tx` clone

Both worker types are dispatched through `flush_dispatches`, count toward `in_flight`, and respect `max_concurrency`.

**RunState internals**:

```rust
impl RunState {
    fn apply_submitted(&mut self, submitted: &TaskSubmitted) {
        let parent_id = match &submitted.origin {
            TaskOrigin::Seed => None,
            TaskOrigin::Spawned(spawned) => {
                // Parent already transitioned to WaitingForChildren by
                // apply_completed (which set the count from children).
                // Just verify it exists and is in the expected state.
                let parent = self.tasks.get(&spawned.parent_id)
                    .expect("[P046] spawned child's parent must exist");
                assert!(matches!(&parent.state,
                    TaskState::WaitingForChildren(..)),
                    "[P049] spawned child's parent not in WaitingForChildren state");
                Some(spawned.parent_id)
            }
            TaskOrigin::Retry(retry) => {
                // Replace the failed task. Inherit its parent.
                let old = self.tasks.remove(&retry.replaces)
                    .expect("[P042] retry target must exist");
                assert!(matches!(old.state, TaskState::Failed),
                    "[P045] retry target not in Failed state");
                old.parent_id
            }
        };

        self.tasks.insert(submitted.task_id, TaskEntry {
            step: submitted.step.clone(),
            parent_id,
            state: TaskState::Pending(PendingState {
                value: submitted.value.clone(),
            }),
        });
    }

    /// Called for every completion — both success and failure, both live
    /// and replay. Handles the completion and all tasks in the outcome
    /// atomically. No transient states. Returns the parent_id of the
    /// removed task for leaf/permanent-failure — the Engine uses this
    /// to start the parent-chain walk.
    fn apply_completed(&mut self, completed: &TaskCompleted)
        -> Option<LogTaskId>
    {
        let entry = self.tasks.get_mut(&completed.task_id)
            .expect("[P033] completed task must exist");
        assert!(matches!(&entry.state, TaskState::Pending(..)),
            "[P034] completed task not in Pending state");

        match &completed.outcome {
            TaskOutcome::Succeeded(success) if !success.children.is_empty() => {
                entry.state = TaskState::WaitingForChildren(WaitingState {
                    pending_children_count: NonZeroU16::new(
                        success.children.len() as u16
                    ).unwrap(),
                    finally_value: success.finally_value.clone(),
                });
                for submitted in &success.children {
                    self.apply_submitted(submitted);
                }
                None
            }
            TaskOutcome::Failed(failure) if failure.retry.is_some() => {
                entry.state = TaskState::Failed;
                self.apply_submitted(failure.retry.as_ref().unwrap());
                None
            }
            TaskOutcome::Succeeded(_) | TaskOutcome::Failed(_) => {
                // Leaf success or permanent failure. Remove the task.
                let removed = self.tasks.remove(&completed.task_id)
                    .expect("[P033]");
                removed.parent_id
            }
        }
    }

    /// Processes a FinallyRun event. Removes the parent whose finally
    /// ran. If the finally produced children, inserts them under the
    /// grandparent. If no children, decrements grandparent's count.
    /// Returns grandparent_id if the grandparent reached zero children
    /// (so Engine can continue the walk-up).
    fn apply_finally_run(&mut self, finally_run: &FinallyRun)
        -> Option<LogTaskId>
    {
        let parent = self.tasks.remove(&finally_run.finally_for)
            .expect("[P058] FinallyRun target must exist");
        let grandparent_id = parent.parent_id;

        if finally_run.subsequent.is_empty() {
            // No children from the finally. Notify grandparent.
            if let Some(gp_id) = grandparent_id {
                return self.decrement_child_count(gp_id);
            }
            None
        } else {
            // Children replace the parent under the grandparent.
            // Count adjustment: -1 (parent removed) + N (new children).
            for submitted in &finally_run.subsequent {
                self.apply_submitted(submitted);
            }
            if let Some(gp_id) = grandparent_id {
                self.adjust_child_count(gp_id,
                    finally_run.subsequent.len() as i16 - 1);
            }
            None
        }
    }

    /// Walk up the parent chain from a parent whose child was just
    /// removed. Decrements the parent's child count. If the parent
    /// reaches zero children:
    ///   - Has a finally script → return PendingFinally (stop walking)
    ///   - No finally script → remove the parent, continue to grandparent
    /// Returns None if no ancestor needs a finally (all ancestors still
    /// have live children, or no ancestor has a finally up to root).
    fn walk_up_for_finally(
        &mut self,
        mut parent_id: LogTaskId,
        config: &Config,
    ) -> Option<PendingFinally> {
        loop {
            let zero = self.decrement_child_count(parent_id);
            if zero.is_none() {
                return None; // parent still has children
            }

            let entry = self.tasks.get(&parent_id)
                .expect("[P059] parent must exist");
            let step = &entry.step;
            let has_finally = config.step_has_finally(step);

            if has_finally {
                let finally_value = match &entry.state {
                    TaskState::WaitingForChildren(w) =>
                        w.finally_value.clone(),
                    _ => panic!("[P041] parent not in WaitingForChildren"),
                };
                return Some(PendingFinally {
                    parent_id,
                    step: step.clone(),
                    finally_value,
                });
            }

            // No finally — remove this ancestor and continue up.
            let removed = self.tasks.remove(&parent_id)
                .expect("[P059]");
            match removed.parent_id {
                Some(gp_id) => parent_id = gp_id,
                None => return None, // reached root
            }
        }
    }

    /// Decrements a task's pending_children_count. Returns Some(task_id)
    /// if the count reached zero, None otherwise.
    fn decrement_child_count(&mut self, task_id: LogTaskId)
        -> Option<LogTaskId>
    {
        let entry = self.tasks.get_mut(&task_id)
            .expect("[P060] task must exist");
        match &mut entry.state {
            TaskState::WaitingForChildren(w) => {
                let count = w.pending_children_count.get() - 1;
                if let Some(new_count) = NonZeroU16::new(count) {
                    w.pending_children_count = new_count;
                    None
                } else {
                    Some(task_id)
                }
            }
            _ => panic!("[P061] decrement on non-WaitingForChildren task"),
        }
    }
}
```

`walk_up_for_finally` replaces `remove_and_notify_parent`. It walks up the parent chain synchronously: decrement count, if zero and no finally → remove and continue, if zero and has finally → return `PendingFinally`. At most one `PendingFinally` per call. The Engine pushes these into `pending_dispatches` and dispatches them via `flush_dispatches` post-batch, respecting `max_concurrency` like any other dispatch.

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

Writes every entry it receives — `Config`, `TaskSubmitted`, `TaskCompleted`, and `FinallyRun` — including replayed entries during Resume. The new log is a complete record — it starts with a copy of the old log (replayed as part of the seed batch) and then appends new entries from live execution.

### Termination

Engine sends `ControlFlow::Break(result)` on `tx` when `pending_dispatches` is empty and `in_flight == 0` (checked in `flush_dispatches`). Both task workers and finally workers go through `flush_dispatches` and count toward `in_flight`, so shutdown naturally waits for all workers to complete. The result is `Ok(())` if the workflow completed successfully, `Err(..)` if a task permanently failed (retries exhausted). The Engine derives this from state — it's not a separate log entry. The coordinator matches on `Break`, logs the result, and exits the loop. All appliers are dropped, including Engine (which drops `tx`). Worker threads hold `Sender` clones — they've already completed and dropped theirs by the time shutdown fires (in_flight is 0).

### TaskEntry and TaskState

```rust
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    state: TaskState,
}

enum TaskState {
    Pending(PendingState),
    WaitingForChildren(WaitingState),
    /// Task failed and a retry follows. Transient: only exists between
    /// apply_completed setting it and retry's apply_submitted removing
    /// the task (both happen within the same apply_completed call).
    Failed,
}

struct PendingState {
    value: StepInputValue,
}

struct WaitingState {
    pending_children_count: NonZeroU16,
    finally_value: StepInputValue,
}
```

TaskState has three variants. `Pending` and `WaitingForChildren` are the stable live states. `Failed` is transient: it exists only when a retry follows (`failure.retry.is_some()`), and the retry's `apply_submitted` removes the task within the same `apply_completed` call. There is no `Succeeded` variant — `apply_completed` transitions directly to `WaitingForChildren` for tasks with children, or removes leaf tasks and permanent failures immediately.

The current `InFlight` variant is replaced by `in_flight: usize` on the Engine. `finally_script` and `retries_remaining` are removed from TaskEntry — the Engine looks up the finally script from config when needed, and workers determine whether to retry based on step config captured in their closures.

## Replay

There is no separate replay function. Resume reads the old log and uses it as the seed:

```rust
RunMode::Resume { old_log_path } => {
    barnum_state::read_entries(&old_log_path)?
}
```

The old entries flow through `process_entries` like any other batch. The first entry is `Config` — Engine deserializes and stores it. Subsequent entries (`TaskSubmitted`, `TaskCompleted`, `FinallyRun`) build up RunState. `in_flight` stays at 0 throughout the seed batch via `saturating_sub` — nothing was actually dispatched. After the batch:

1. `id_counter` is initialized to `max_seen_id + 1` via `fetch_max`.
2. `flush_dispatches` dispatches any remaining Pending tasks and finally workers. Tasks that completed during replay were removed from `pending_dispatches` when `apply()` processed their `TaskCompleted`. Finallys whose `FinallyRun` was in the batch were removed when `apply()` processed the `FinallyRun`. Everything left in the queue is valid.

If the old log ended mid-workflow (e.g. a parent's children all completed but no `FinallyRun` was logged — crash before the finally worker completed), `walk_up_for_finally` pushed a `PendingFinally` into `pending_dispatches` during the batch and no `FinallyRun` cleared it — so `flush_dispatches` dispatches a new finally worker.

No separate cleanup phases are needed. `apply_completed` handles everything atomically: leaf successes and permanent failures are removed immediately, children transition the parent to `WaitingForChildren`, and retries replace the failed task. The same code path handles both live execution and replay.

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

After: Workers produce a single `TaskCompleted` (with children/retry in `subsequent`). That entry flows through both Engine and LogApplier — state and log see the same entries and can never diverge. Finally handling is a post-batch Engine concern: `FinallyRun` entries flow through the same applier chain.

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
let result = loop {
    match rx.recv().expect("[P062]") {
        ControlFlow::Continue(entry) => process_entries(&mut appliers, &[entry]),
        ControlFlow::Break(result) => break result,
    }
};
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
#[test] fn apply_submitted_spawned_verifies_parent_in_waiting()
#[test] fn apply_submitted_retry_removes_failed_task()
#[test] fn apply_submitted_retry_inherits_parent_id()

// RunState — apply_completed
#[test] fn apply_completed_succeeded_with_children_transitions_to_waiting()
#[test] fn apply_completed_succeeded_leaf_removes_and_returns_parent()
#[test] fn apply_completed_permanent_failure_removes_and_returns_parent()
#[test] fn apply_completed_failed_with_retry_marks_failed_and_inserts_retry()
#[test] fn apply_completed_inserts_children_via_apply_submitted()

// RunState — apply_finally_run
#[test] fn apply_finally_run_removes_parent()
#[test] fn apply_finally_run_inserts_children_under_grandparent()
#[test] fn apply_finally_run_empty_subsequent_decrements_grandparent()
#[test] fn apply_finally_run_returns_grandparent_when_count_zero()

// RunState — walk_up_for_finally
#[test] fn walk_up_returns_pending_finally_for_ancestor_with_finally()
#[test] fn walk_up_removes_intermediate_no_finally_ancestors()
#[test] fn walk_up_returns_none_when_parent_has_remaining_children()
#[test] fn walk_up_returns_none_when_no_ancestor_has_finally()

// RunState — decrement_child_count
#[test] fn decrement_returns_some_when_count_reaches_zero()
#[test] fn decrement_returns_none_when_count_positive()

// Engine — apply()
#[test] fn apply_submitted_queues_dispatch()
#[test] fn apply_submitted_panics_on_duplicate_id()
#[test] fn apply_succeeded_queues_children_for_dispatch()
#[test] fn apply_failed_with_retry_queues_retry_for_dispatch()
#[test] fn apply_completed_panics_on_unknown_task()
#[test] fn apply_completed_walks_up_for_finally()
#[test] fn apply_finally_run_queues_children_for_dispatch()
#[test] fn apply_finally_run_queues_walk_up_finally()
#[test] fn apply_finally_run_removes_pending_finally_from_queue()
#[test] fn apply_initializes_counter_from_max_seen_id()
#[test] fn apply_completed_removes_pending_task_from_queue()
#[test] fn flush_dispatches_up_to_max_concurrency()
#[test] fn flush_panics_on_stale_task()
#[test] fn flush_panics_on_stale_finally()
#[test] fn flush_sends_shutdown_when_empty_and_no_in_flight()
#[test] fn flush_respects_max_concurrency_for_finallys()

// Engine — replay via seed batch
#[test] fn replay_seed_reconstructs_state()
#[test] fn replay_seed_dispatches_remaining_pending_tasks()
#[test] fn replay_seed_skips_completed_tasks_in_dispatch()
#[test] fn replay_finally_run_in_batch_prevents_rerun()
#[test] fn replay_missing_finally_run_dispatches_worker()

// LogApplier
#[test] fn writes_all_entry_variants()

// Coordinator
#[test] fn process_entries_calls_all_appliers()
#[test] fn event_loop_exits_on_break()

// Workers
#[test] fn worker_produces_succeeded_with_children()
#[test] fn worker_produces_succeeded_leaf()
#[test] fn worker_produces_failed_with_retry()
#[test] fn worker_produces_failed_permanent()
#[test] fn worker_allocates_ids_from_shared_counter()
```
