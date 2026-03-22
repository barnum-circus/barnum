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

During live execution, IDs are allocated from a shared `Arc<AtomicU32>` counter. Workers allocate IDs for children and retries. The counter guarantees uniqueness across concurrent allocators. During `apply()`, a local `max_seen_id` tracks the highest ID in the batch; after the batch, `id_counter` is set to at least `max_seen_id + 1` via `fetch_max`.

### Target event loop

```rust
type ChannelMsg = ControlFlow<WorkflowResult, StateLogEntry>;
```

The channel carries `ChannelMsg`. `Continue(entry)` is a normal entry — workers send one `TaskCompleted`, Engine sends `FinallyRun` entries. `Break(result)` is the shutdown signal — Engine sends `Break(Ok(()))` when the workflow completes successfully, `Break(Err(..))` when a task permanently fails (retries exhausted). The coordinator matches on the message: `Continue` entries flow through `process_entries`, `Break` exits the loop and logs the result. The workflow result is NOT written to the state log — it's derivable from the entries.

```rust
enum RunMode {
    Fresh { initial_tasks: Vec<Task> },
    Resume { old_log_path: PathBuf },
}

pub fn run(mode: RunMode, runner_config: &RunnerConfig) -> WorkflowResult {
    let seed = match mode {
        RunMode::Fresh { initial_tasks } => {
            let config = /* loaded by caller or passed separately */;
            build_seed_entries(&config, &initial_tasks)
        }
        RunMode::Resume { old_log_path } => {
            barnum_state::read_entries(&old_log_path)?
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

Every source sends `ChannelMsg` on the channel. Workers send `Continue(TaskCompleted(...))`. Engine sends `Continue(FinallyRun(...))` for finally entries, and `Break(Ok(()))` or `Break(Err(..))` for shutdown. The coordinator matches on the message: `Continue` entries are wrapped in `&[entry]` for the batch-based Applier interface, `Break` exits the loop and returns the result.

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
    Spawned { parent_id: LogTaskId },
    /// Retry of a failed task. parent_id derived from the replaced task.
    Retry { replaces: LogTaskId },
}

struct TaskCompleted {
    task_id: LogTaskId,
    outcome: TaskOutcome,
}

/// The outcome of running a task. Each variant captures everything
/// needed — no separate "subsequent" field. Workers determine the
/// outcome (children, retry, or permanent failure) based on the
/// result and step config captured in their closure. Workers do NOT
/// know about finallys — that's an Engine concern via FinallyRun.
enum TaskOutcome {
    /// Task succeeded. Children may be empty (leaf success).
    Succeeded {
        finally_value: StepInputValue,
        children: Vec<TaskSubmitted>,
    },
    /// Task failed, retrying. The retry replaces this task.
    Retrying {
        reason: FailureReason,
        retry: TaskSubmitted,
    },
    /// Task failed permanently (retries exhausted or non-retryable).
    PermanentFailure {
        reason: FailureReason,
    },
}

/// Records that a finally script ran for a parent whose children all
/// completed. Atomic: present in the log = done, absent = re-run on
/// resume. The Engine runs the finally script synchronously post-batch
/// (expected to be fast) and emits this event with the children produced.
struct FinallyRun {
    finally_for: LogTaskId,
    children: Vec<TaskSubmitted>,
}
```

Each `TaskOutcome` variant is self-contained. `Succeeded` carries the finally value and children (empty vec for leaf tasks). `Retrying` carries the failure reason and the retry task. `PermanentFailure` carries just the reason. No impossible states — success can't have a retry, failure can't have children.

`apply_completed` matches on the single `outcome` field. For `Succeeded` with non-empty children, the parent transitions to `WaitingForChildren(N)`. For `Succeeded` with empty children or `PermanentFailure`, the task is removed and the Engine walks up the parent chain for finally detection. For `Retrying`, the task is marked `Failed` and the retry's `apply_submitted` replaces it.

`FinallyRun` is the Engine's record that a finally mechanism fired. The Engine runs the script synchronously post-batch, emits `FinallyRun` on `tx`. It comes back through the coordinator and flows through all appliers. During replay, `FinallyRun` entries are in the batch — the Engine marks them as handled and skips re-running. On resume, if a parent's children are all done but no `FinallyRun` is in the log, the Engine re-runs the finally.

Each `TaskOrigin` variant carries only non-derivable information. `Spawned { parent_id }` needs the parent explicitly (no other way to know it). `Retry { replaces }` references a task that's still in the map — `apply_submitted` derives `parent_id` from the referenced task's entry. `Seed` has no relationships.

## Applier

```rust
trait Applier {
    fn apply(&mut self, entries: &[StateLogEntry]);
}
```

Both Engine and LogApplier implement this trait. One method. The coordinator calls it on every applier for every batch of entries. Engine processes all entries in the batch before dispatching — no threads are spawned mid-batch.

### Engine

Owns the full execution lifecycle: task state, dispatch, finally execution, and shutdown. Holds a `Sender<ChannelMsg>` — workers get clones (they send their `TaskCompleted`), and the Engine sends `FinallyRun` entries and `Break(result)` for shutdown.

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
    pending_dispatches: VecDeque<PendingTask>,
    dispatched: HashSet<LogTaskId>,
}

impl Engine {
    fn config(&self) -> &Config {
        self.config.as_ref().expect("[P051] config not set")
    }
}

struct PendingFinally {
    parent_id: LogTaskId,
    step: StepName,
    finally_value: StepInputValue,
}

struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
}

struct PendingTask {
    task_id: LogTaskId,
    step: StepName,
    value: StepInputValue,
}
```

`id_counter` is shared between Engine and workers (via `Arc<AtomicU32>`). Workers allocate IDs atomically for children and retries. During `apply()`, a local `max_seen_id` tracks the highest task ID seen in the batch. After the batch, `id_counter` is set to at least `max_seen_id + 1` via `fetch_max` — this initializes the counter correctly after the seed batch (replay). During live execution the counter is already past all seen IDs, so `fetch_max` is a no-op.

**`apply()`**: Processes a batch of entries, then runs finally scripts for any parents whose children all completed, then flushes dispatches. All entries in the batch are processed before any dispatch or finally execution happens — this is critical for replay, where the entire old log is applied as one batch.

```rust
impl Applier for Engine {
    fn apply(&mut self, entries: &[StateLogEntry]) {
        let mut max_seen_id: u32 = 0;
        let mut parents_needing_finally: Vec<PendingFinally> = Vec::new();

        for entry in entries {
            max_seen_id = max_seen_id.max(max_id_in(entry));
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
                    if self.dispatched.remove(&c.task_id) {
                        self.in_flight -= 1;
                    }
                    let parent_id = self.state.apply_completed(c);
                    // For leaf/permanent-failure: walk up the parent chain.
                    if let Some(pid) = parent_id {
                        if let Some(pf) = self.state.walk_up_for_finally(
                            pid, self.config()
                        ) {
                            parents_needing_finally.push(pf);
                        }
                    }
                    // Queue children/retry for dispatch.
                    match &c.outcome {
                        TaskOutcome::Succeeded { children, .. } => {
                            for s in children {
                                self.pending_dispatches.push_back(PendingTask {
                                    task_id: s.task_id,
                                    step: s.step.clone(),
                                    value: s.value.clone(),
                                });
                            }
                        }
                        TaskOutcome::Retrying { retry, .. } => {
                            self.pending_dispatches.push_back(PendingTask {
                                task_id: retry.task_id,
                                step: retry.step.clone(),
                                value: retry.value.clone(),
                            });
                        }
                        TaskOutcome::PermanentFailure { .. } => {}
                    }
                }
                StateLogEntry::FinallyRun(f) => {
                    let grandparent_id = self.state.apply_finally_run(f);
                    // Mark this parent as handled — don't re-run post-batch.
                    parents_needing_finally
                        .retain(|pf| pf.parent_id != f.finally_for);
                    // Queue children for dispatch.
                    for s in &f.children {
                        self.pending_dispatches.push_back(PendingTask {
                            task_id: s.task_id,
                            step: s.step.clone(),
                            value: s.value.clone(),
                        });
                    }
                    // FinallyRun with no children may trigger another
                    // finally up the chain (grandparent reached zero).
                    if let Some(gp_id) = grandparent_id {
                        if let Some(pf) = self.state.walk_up_for_finally(
                            gp_id, self.config()
                        ) {
                            parents_needing_finally.push(pf);
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

        // Post-batch: run finally scripts for parents whose children
        // all completed during this batch. During replay, FinallyRun
        // entries in the batch already cleared these.
        let sent_finally = !parents_needing_finally.is_empty();
        for pf in parents_needing_finally {
            let result = self.pool.run_finally_sync(&pf.step, &pf.finally_value);
            let children = self.build_submitted_from_result(
                &pf, &result, &self.id_counter,
            );
            let entry = StateLogEntry::FinallyRun(FinallyRun {
                finally_for: pf.parent_id,
                children,
            });
            self.tx.send(ControlFlow::Continue(entry))
                .expect("[P050] channel send failed");
        }

        self.id_counter.fetch_max(max_seen_id + 1, Ordering::SeqCst);
        self.flush_dispatches(sent_finally);
    }
}
```

`FinallyRun` entries go on `tx` as individual messages and arrive as subsequent entries in the coordinator loop. Each is wrapped in `&[entry]` and flows through all appliers. Engine does NOT apply `FinallyRun` entries to its own state when producing them — that happens when they come back through the channel.

**`flush_dispatches()`**: Spawns worker threads. Each worker gets a `tx` clone, an `id_counter` clone, and the step config.

```rust
fn flush_dispatches(&mut self, sent_finally: bool) {
    while self.in_flight < self.max_concurrency {
        let Some(task) = self.pending_dispatches.pop_front() else { break };
        // Skip tasks no longer in Pending state (completed during replay).
        if !self.state.tasks.get(&task.task_id)
            .map_or(false, |e| matches!(&e.state, TaskState::Pending { .. })) {
            continue;
        }
        self.in_flight += 1;
        self.dispatched.insert(task.task_id);
        let tx = self.tx.clone();
        let id_counter = self.id_counter.clone();
        // spawn worker thread with task, step config, tx, id_counter
    }

    if !sent_finally && self.pending_dispatches.is_empty() && self.in_flight == 0 {
        let result = self.compute_workflow_result();
        self.tx.send(ControlFlow::Break(result))
            .expect("[P055] shutdown send failed");
    }
}
```

**Workers**: Each worker thread:

1. Runs the task via the pool
2. Interprets the result (`process_submit_result`, post hooks) — step config captured in closure
3. Allocates IDs for children/retries from the shared `id_counter`
4. Produces a single `TaskCompleted` with outcome: `Succeeded`, `Retrying`, or `PermanentFailure`
5. Sends `ControlFlow::Continue(StateLogEntry::TaskCompleted(...))` on `tx`, drops `tx` clone

Workers don't know about finallys — that's an Engine concern. Workers only know about children (they spawned them) and retries (they decided to retry based on step config in their closure).

**RunState internals**:

```rust
impl RunState {
    fn apply_submitted(&mut self, submitted: &TaskSubmitted) {
        let parent_id = match &submitted.origin {
            TaskOrigin::Seed => None,
            TaskOrigin::Spawned { parent_id } => {
                // Parent already transitioned to WaitingForChildren by
                // apply_completed (which set the count from subsequent).
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
    /// and replay. Handles the completion and all tasks in the outcome
    /// atomically. No transient states. Returns the parent_id of the
    /// removed task for leaf/permanent-failure — the Engine uses this
    /// to start the parent-chain walk.
    fn apply_completed(&mut self, completed: &TaskCompleted)
        -> Option<LogTaskId>
    {
        let entry = self.tasks.get_mut(&completed.task_id)
            .expect("[P033] completed task must exist");
        assert!(matches!(&entry.state, TaskState::Pending { .. }),
            "[P034] completed task not in Pending state");

        match &completed.outcome {
            TaskOutcome::Succeeded { finally_value, children }
                if !children.is_empty() =>
            {
                entry.state = TaskState::WaitingForChildren {
                    pending_children_count: NonZeroU16::new(
                        children.len() as u16
                    ).unwrap(),
                    finally_value: finally_value.clone(),
                };
                for submitted in children {
                    self.apply_submitted(submitted);
                }
                None
            }
            TaskOutcome::Succeeded { .. } | TaskOutcome::PermanentFailure { .. } => {
                // Leaf success or permanent failure. Remove the task.
                let removed = self.tasks.remove(&completed.task_id)
                    .expect("[P033]");
                removed.parent_id
            }
            TaskOutcome::Retrying { retry, .. } => {
                entry.state = TaskState::Failed;
                self.apply_submitted(retry);
                None
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
                    TaskState::WaitingForChildren { finally_value, .. } =>
                        finally_value.clone(),
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
            TaskState::WaitingForChildren {
                pending_children_count, ..
            } => {
                let count = pending_children_count.get() - 1;
                if let Some(new_count) = NonZeroU16::new(count) {
                    *pending_children_count = new_count;
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

`walk_up_for_finally` replaces `remove_and_notify_parent`. Instead of accumulating into a deferred queue, it walks up the parent chain synchronously: decrement count, if zero and no finally → remove and continue, if zero and has finally → return `PendingFinally`. At most one `PendingFinally` per call. The Engine accumulates these during the batch and runs finally scripts post-batch.

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

Engine sends `ControlFlow::Break(result)` on `tx` when `pending_dispatches` is empty, `in_flight == 0`, and no finally entries were sent this batch (checked in `flush_dispatches`). The result is `Ok(())` if the workflow completed successfully, `Err(..)` if a task permanently failed (retries exhausted, `Subsequent::None` on a failure). The Engine derives this from state — it's not a separate log entry. The coordinator matches on `Break`, logs the result, and exits the loop. All appliers are dropped, including Engine (which drops `tx`). Worker threads hold `Sender` clones — they've already completed and dropped theirs by the time shutdown fires (in_flight is 0).

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

The current `InFlight` variant is replaced by `in_flight: usize` + `dispatched: HashSet<LogTaskId>` on the Engine. `finally_script` and `retries_remaining` are removed from TaskEntry — the Engine looks up the finally script from config when running finally scripts post-batch, and workers determine whether to retry based on step config captured in their closures.

## Replay

There is no separate replay function. Resume reads the old log and uses it as the seed:

```rust
RunMode::Resume { old_log_path } => {
    barnum_state::read_entries(&old_log_path)?
}
```

The old entries flow through `process_entries` like any other batch. The first entry is `Config` — Engine deserializes and stores it. Subsequent entries (`TaskSubmitted`, `TaskCompleted`, `FinallyRun`) build up RunState. After the batch:

1. `parents_needing_finally` is checked — during replay, `FinallyRun` entries in the batch already cleared these, so no finally scripts are re-run.
2. `id_counter` is initialized to `max_seen_id + 1` via `fetch_max`.
3. `flush_dispatches` dispatches any remaining Pending tasks. Tasks that completed during replay are skipped (state check).

If the old log ended mid-workflow (e.g. a parent's children all completed but no `FinallyRun` was logged — crash before the finally script ran), the replay accumulates the `PendingFinally` and the post-batch step re-runs the finally script.

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
#[test] fn apply_completed_succeeded_empty_children_removes_and_returns_parent()
#[test] fn apply_completed_permanent_failure_removes_and_returns_parent()
#[test] fn apply_completed_retrying_marks_failed_and_inserts_retry()
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
#[test] fn apply_retrying_queues_retry_for_dispatch()
#[test] fn apply_completed_panics_on_unknown_task()
#[test] fn apply_completed_walks_up_for_finally()
#[test] fn apply_finally_run_queues_children_for_dispatch()
#[test] fn apply_finally_run_clears_pending_finally()
#[test] fn apply_finally_run_empty_triggers_walk_up()
#[test] fn apply_initializes_counter_from_max_seen_id()
#[test] fn apply_post_batch_runs_finally_scripts()
#[test] fn apply_post_batch_sends_finally_run_on_tx()
#[test] fn flush_dispatches_up_to_max_concurrency()
#[test] fn flush_dispatches_skips_completed_tasks()
#[test] fn flush_sends_shutdown_when_empty_and_no_in_flight()
#[test] fn flush_skips_shutdown_when_finally_sent()

// Engine — replay via seed batch
#[test] fn replay_seed_reconstructs_state()
#[test] fn replay_seed_dispatches_remaining_pending_tasks()
#[test] fn replay_seed_skips_completed_tasks_in_dispatch()
#[test] fn replay_finally_run_in_batch_prevents_rerun()
#[test] fn replay_missing_finally_run_reruns_script()

// LogApplier
#[test] fn writes_all_entry_variants()

// Coordinator
#[test] fn process_entries_calls_all_appliers()
#[test] fn event_loop_exits_on_break()

// Workers
#[test] fn worker_produces_succeeded_with_children()
#[test] fn worker_produces_succeeded_leaf()
#[test] fn worker_produces_retrying()
#[test] fn worker_produces_permanent_failure()
#[test] fn worker_allocates_ids_from_shared_counter()
```
