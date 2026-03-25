# Applier Trait Pattern

**Status:** Pending

**Depends on:** WORKER_ENTRY_PRODUCTION (sub-refactor, filed separately)

## History

The `Vec<Box<dyn Applier>>` pattern was designed in `APPLY_PATTERN.md` (now `refactors/past/`) across many commits from January to March 2025. The design specified Engine and LogApplier behind a trait-based vector with a generic coordinator loop.

The actual Phase 3 implementation (commit `d1ba8e0`) diverged. Instead of a trait and vector, `run_loop` holds `&mut Engine` and `&mut BufWriter<File>` as concrete arguments and calls each explicitly. The Applier trait was never implemented in code. `APPLY_PATTERN.md` was moved to `past/` as "Done" despite the mismatch.

## Motivation

The coordinator (`run_loop` at `runner/mod.rs:972`) is coupled to both concrete types:

```rust
fn run_loop(
    engine: &mut Engine<'_>,
    rx: &mpsc::Receiver<WorkerResult>,
    log_writer: &mut io::BufWriter<std::fs::File>,
) -> io::Result<()> {
    loop {
        if engine.is_done() { break; }
        let result = rx.recv().expect("[P062]");
        let entries = engine.process_worker_result(result);
        for entry in &entries {
            write_log(log_writer, entry);
        }
    }
    engine.compute_result()
}
```

The coordinator knows entries come from an engine, that a file writer needs each one, and that done-ness is an engine concept. Adding a third consumer (metrics, visualization, etc.) requires changing the loop. A `Vec<Box<dyn Applier>>` makes the coordinator a generic message pump where adding an applier is construction-time configuration.

Additionally, `Engine::process_worker_result` conflates result interpretation (WorkerResult → StateLogEntry) with state application. Once interpretation moves to workers (sub-refactor WORKER_ENTRY_PRODUCTION), the channel carries entries directly and the coordinator becomes fully generic.

## Current State

### Coordinator (`runner/mod.rs:972`)

`run_loop` receives `WorkerResult` from the channel, calls `engine.process_worker_result()` to convert and apply, then writes entries to the log via a free function. Two different interfaces, two concrete types, manually sequenced.

### Engine (`runner/mod.rs:457`)

`Engine` owns `RunState`, config references, the sender channel, dispatch queue, and concurrency tracking. `process_worker_result` decrements `in_flight`, converts the result to entries, applies entries to state via `RunState::apply_entry`, and flushes dispatches. The state mutation layer (`RunState::apply_entry` at line 272) is already pure and tested independently.

### Log writing (`runner/mod.rs:812`)

A free function `write_log` writes entries to a `BufWriter<File>`. No struct, no trait.

### Channel type

Workers send `WorkerResult` (raw output). The channel is `mpsc::channel::<WorkerResult>()`. The Engine interprets results on the main thread using `convert_task_result` (line 530) and `convert_finally_result` (line 601), which access Engine state for ID allocation (`RunState::next_id`) and grandparent lookup.

## Target Architecture

### Applier Trait

```rust
trait Applier {
    fn apply(&mut self, entries: &[StateLogEntry]);
}
```

One method. The coordinator calls it on every applier for every batch of entries.

### Channel Message

```rust
type ChannelMsg = ControlFlow<io::Result<()>, StateLogEntry>;
```

`Continue(entry)` is a normal state log entry produced by a worker. `Break(result)` is the shutdown signal from Engine. Workers send `Continue(TaskCompleted(...))` or `Continue(FinallyRun(...))`. Engine sends `Break(Ok(()))` or `Break(Err(...))` when the workflow finishes.

This requires the WORKER_ENTRY_PRODUCTION sub-refactor to land first, so workers produce entries directly instead of raw results.

### Coordinator

```rust
pub fn run(
    config: &Config,
    runner_config: &RunnerConfig,
    initial_tasks: Vec<Task>,
) -> io::Result<()> {
    if let Some(script) = runner_config.wake_script {
        call_wake_script(script)?;
    }

    let (tx, rx) = mpsc::channel::<ChannelMsg>();
    let id_counter = Arc::new(AtomicU32::new(0));

    let mut appliers: Vec<Box<dyn Applier>> = vec![
        Box::new(Engine::new(config, tx.clone(), id_counter.clone(), runner_config)),
        Box::new(LogApplier::new(runner_config.state_log_path)?),
    ];

    let seed = build_seed_entries(config, &initial_tasks, &id_counter);
    process_entries(&mut appliers, &seed);

    loop {
        match rx.recv().expect("[P062] channel closed unexpectedly") {
            ControlFlow::Continue(entry) => process_entries(&mut appliers, &[entry]),
            ControlFlow::Break(result) => return result,
        }
    }
}

fn process_entries(appliers: &mut [Box<dyn Applier>], entries: &[StateLogEntry]) {
    for applier in appliers.iter_mut() {
        applier.apply(entries);
    }
}
```

The coordinator constructs the applier vector, builds seed entries, and runs the event loop. It passes entries to each applier through the trait. Adding a third applier (metrics, visualization) means adding one more `Box::new(...)` to the vector.

### Engine

Engine's `apply` iterates entries through `RunState::apply_entry` (pure state mutation), then calls `flush_dispatches` to spawn workers. Two layers: the inner function per entry has no side effects beyond mutating `RunState`; the outer flush produces I/O.

```rust
impl Applier for Engine {
    fn apply(&mut self, entries: &[StateLogEntry]) {
        for entry in entries {
            // Completion entries mean a worker finished.
            if matches!(entry, StateLogEntry::TaskCompleted(_) | StateLogEntry::FinallyRun(_)) {
                self.in_flight = self.in_flight.saturating_sub(1);
            }
            // Track permanent failures for the workflow result.
            if let StateLogEntry::TaskCompleted(c) = entry {
                if matches!(&c.outcome, barnum_state::TaskOutcome::Failed(f) if f.retry.is_none()) {
                    self.dropped_count += 1;
                }
            }
            // Pure state mutation — no I/O, no dispatch.
            self.state.apply_entry(entry, self.config);
        }
        // Sync the shared counter past all IDs seen in this batch.
        self.id_counter.fetch_max(self.state.next_task_id, Ordering::SeqCst);
        // Side effects: spawn workers, detect shutdown.
        self.flush_dispatches();
    }
}
```

`flush_dispatches` is unchanged in structure. When `pending_dispatches.is_empty() && in_flight == 0`, Engine sends `ControlFlow::Break(self.compute_result())` on `tx`.

Unit testing works at the `RunState::apply_entry` level: construct a `RunState`, feed entries, assert state. The existing `run_state_tests` module (`runner/mod.rs:1011`) already tests this way and requires no changes. For tests that want to verify Engine-level behavior without real workers, construct an Engine with a channel and inspect what it sends.

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

Writes every entry it receives, including replayed entries during resume (the new log starts with a complete copy of the old log).

### Resume

```rust
pub fn resume(old_log_path: &Path, runner_config: &RunnerConfig) -> io::Result<()> {
    let old_entries: Vec<StateLogEntry> = barnum_state::read_entries(
        File::open(old_log_path)?
    ).collect::<Result<Vec<_>, _>>()?;

    let config: Config = /* extract and deserialize from first entry */;

    let (tx, rx) = mpsc::channel::<ChannelMsg>();
    let id_counter = Arc::new(AtomicU32::new(0));

    let mut appliers: Vec<Box<dyn Applier>> = vec![
        Box::new(Engine::new(&config, tx.clone(), id_counter.clone(), runner_config)),
        Box::new(LogApplier::new(runner_config.state_log_path)?),
    ];

    // Old log entries are the seed. Engine replays state;
    // LogApplier writes them to the new log file.
    process_entries(&mut appliers, &old_entries);

    loop {
        match rx.recv().expect("[P062]") {
            ControlFlow::Continue(entry) => process_entries(&mut appliers, &[entry]),
            ControlFlow::Break(result) => return result,
        }
    }
}
```

During the seed batch, `in_flight` stays at 0 (completion entries decrement via `saturating_sub` from 0). After the batch, `flush_dispatches` dispatches remaining pending work. LogApplier writes all old entries to the new log, producing a complete copy before any live entries append.

## Sub-refactors

### WORKER_ENTRY_PRODUCTION.md (prerequisite)

Move result interpretation from Engine to workers. Workers capture step config at dispatch time and produce `StateLogEntry` instead of `WorkerResult`. Channel type changes to `ControlFlow<io::Result<()>, StateLogEntry>`. Includes changing `RunState::next_task_id: u32` to a shared `Arc<AtomicU32>` so workers can allocate IDs for children and retries.

Specific changes:

1. **Shared ID counter**: `Arc<AtomicU32>` replaces `RunState::next_task_id`. `RunState::next_id()` becomes a method on Engine (or a free function) that calls `fetch_add(1)`. `advance_id_to` becomes `fetch_max`. Workers receive a clone at dispatch time.

2. **Task workers**: At dispatch time, capture: step (for `process_submit_result`), effective options, valid next step names (for child validation). After running the action, workers call `process_submit_result`, allocate child/retry IDs from the shared counter, and send `Continue(StateLogEntry::TaskCompleted(...))`.

3. **Finally workers**: At dispatch time, capture: grandparent ID (from `state.tasks.get(&parent_id).parent_id`). After running the finally script, allocate child IDs and send `Continue(StateLogEntry::FinallyRun(...))` with children whose origin references the grandparent.

4. **Engine simplification**: `convert_task_result` and `convert_finally_result` are deleted. `process_worker_result` becomes `apply` — it receives entries from the channel and applies them to state.

This sub-refactor lands independently. The coordinator still holds concrete types (`Engine` + `BufWriter`), but the channel carries entries. Filed separately as `refactors/pending/WORKER_ENTRY_PRODUCTION.md`.

## Open Questions

1. **Error handling in LogApplier**: The current `write_log` logs errors and continues. The old `APPLY_PATTERN.md` design used `expect` (panics on write failure). Which behavior? Panicking seems correct — a log write failure means resume data is incomplete.

2. **Config entry flow**: Currently `run` writes the Config entry before constructing Engine. In the Applier pattern, Config could flow through appliers as part of the seed batch (as in the old design, where Engine validates it's the first entry). This would unify the path and make resume simpler (old entries already start with Config). Should the Config entry go through `process_entries`?
