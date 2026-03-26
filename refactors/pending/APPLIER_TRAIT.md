# Applier Trait Pattern

**Status:** Pending

**Depends on:** WORKER_ENTRY_PRODUCTION (sub-refactor, filed separately)

## History

The `Vec<Box<dyn Applier>>` pattern was designed in `APPLY_PATTERN.md` (now `refactors/past/`) across ~55 commits from January to March 2025. The design specified Engine and LogApplier behind a trait-based vector with a generic coordinator loop.

The Phase 3 implementation (commit `d1ba8e0`) diverged deliberately: "No Arc anywhere: Engine owns all state, workers send raw results." Instead of a trait and vector, `run_loop` holds `&mut Engine<'a>` and `&mut BufWriter<File>` as concrete arguments and calls each explicitly. The Applier trait was never implemented in code. `APPLY_PATTERN.md` was moved to `past/` as "Done" despite the implementation not matching the design.

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

Additionally, `run` and `resume` are separate functions (`runner/mod.rs:825` and `runner/mod.rs:909`) that duplicate the channel setup, engine construction, and event loop. They differ only in how the seed entries are produced. Unifying them eliminates this duplication.

## Current State

### Coordinator (`runner/mod.rs:972`)

`run_loop` receives `WorkerResult` from the channel, calls `engine.process_worker_result()` to convert and apply, then writes entries to the log via a free function. Two different interfaces, two concrete types, manually sequenced.

### Engine (`runner/mod.rs:457`)

`Engine<'a>` borrows `config: &'a Config` and `step_map: HashMap<&'a StepName, &'a Step>`. This lifetime parameter prevents Engine from going into a `Box<dyn Applier>` without also bounding the vector's lifetime.

`process_worker_result` (line 509) decrements `in_flight`, converts the result to entries via `convert_task_result`/`convert_finally_result`, applies entries to state via `RunState::apply_entry`, and flushes dispatches. The state mutation layer (`RunState::apply_entry` at line 272) is already pure and tested independently (`run_state_tests` at line 1011).

`apply_and_dispatch` (line 501) is the closest thing to the Applier pattern — iterates entries through `apply_entry`, then flushes. Used only for seed/replay, not for live results.

### Config handling

Config is handled before Engine exists. `run` (line 854) serializes config to JSON, writes a `StateLogEntry::Config` to the log, then passes `&Config` to `Engine::new`. `resume` (line 920) extracts config from the first log entry, deserializes, then passes `&config` to `Engine::new`. The config entry and seed entries follow different paths.

### Log writing (`runner/mod.rs:812`)

A free function `write_log` writes entries to a `BufWriter<File>`. It logs errors but continues (doesn't panic).

### Resume log copy (`runner/mod.rs:946`)

Resume copies the old log's raw bytes to the new file, then replays entries through the engine (but not through the log writer — entries are already in the file). This is an optimization over re-serializing every entry.

## Target Architecture

### Design invariant: IDs come from entries

Every applier sees task IDs only through `StateLogEntry` values. Appliers read IDs from the entries they receive and never allocate IDs independently. During live execution, IDs are allocated from a shared `Arc<AtomicU32>` by workers. During `apply()`, Engine tracks the maximum ID seen and syncs the atomic counter via `fetch_max` after the batch — this initializes the counter correctly after the seed batch (replay). During live execution, the counter is already past all seen IDs, so `fetch_max` is a no-op.

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

`run` and `resume` unify into a single function. They differ only in how seed entries are produced:

```rust
enum RunMode {
    Fresh { initial_tasks: Vec<Task> },
    Resume { old_log_path: PathBuf },
}

pub fn run(mode: RunMode, runner_config: &RunnerConfig) -> io::Result<()> {
    if let Some(script) = runner_config.wake_script {
        call_wake_script(script)?;
    }

    let (tx, rx) = mpsc::channel::<ChannelMsg>();
    let id_counter = Arc::new(AtomicU32::new(0));

    let seed: Vec<StateLogEntry> = match mode {
        RunMode::Fresh { initial_tasks } => {
            build_seed_entries(&initial_tasks, &id_counter)
        }
        RunMode::Resume { old_log_path } => {
            let file = File::open(&old_log_path)?;
            barnum_state::read_entries(file).collect::<Result<Vec<_>, _>>()?
        }
    };

    let mut appliers: Vec<Box<dyn Applier>> = vec![
        Box::new(Engine::new(tx.clone(), id_counter.clone(), runner_config)),
        Box::new(LogApplier::new(runner_config.state_log_path)?),
    ];

    // Seed is initial entries (Fresh) or the entire old log (Resume).
    // Applied as one batch — Engine processes all entries before dispatching.
    // Config is the first entry in the seed; Engine deserializes and stores it.
    // LogApplier writes every entry, producing a complete log from the start.
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

The coordinator constructs the applier vector and runs the event loop. It passes entries to each applier through the trait. Adding a third applier (metrics, visualization) means adding one more `Box::new(...)` to the vector. `run` and `resume` share the same event loop — no duplication.

### Config through appliers

Config is the first entry in the seed batch and flows through all appliers like any other entry. For fresh runs, `build_seed_entries` prepends a `StateLogEntry::Config(...)` before the `TaskSubmitted` entries. For resume, the old log already starts with Config.

Engine stores `config: Option<Config>` instead of `config: &'a Config`. When Engine's `apply()` encounters `StateLogEntry::Config`, it deserializes and stores the config. Engine validates that Config is the first entry it receives and panics on duplicates. A `config()` accessor expects on `None`.

This eliminates the lifetime parameter on Engine. `Engine` becomes `'static`, and `Box<dyn Applier>` needs no lifetime bound. LogApplier writes the Config entry to the log like any other entry.

### Engine

Engine's `apply` iterates entries through `RunState::apply_entry` (pure state mutation), then calls `flush_dispatches` to spawn workers. Two layers: the inner function per entry has no side effects beyond mutating `RunState`; the outer flush produces I/O.

```rust
struct Engine {
    config: Option<Config>,
    config_json: Option<serde_json::Value>,
    state: RunState,
    tx: Sender<ChannelMsg>,
    id_counter: Arc<AtomicU32>,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
    working_dir: PathBuf,
    executor: String,
    run_handler_path: String,
}

impl Applier for Engine {
    fn apply(&mut self, entries: &[StateLogEntry]) {
        for entry in entries {
            match entry {
                StateLogEntry::Config(c) => {
                    assert!(self.config.is_none(), "[P052] duplicate Config entry");
                    assert!(self.state.tasks.is_empty(), "[P053] Config must be first entry");
                    self.config_json = Some(c.config.clone());
                    self.config = Some(
                        serde_json::from_value(c.config.clone())
                            .expect("[P051] config deserialization failed")
                    );
                    self.max_concurrency = self.config().options.max_concurrency
                        .unwrap_or(DEFAULT_MAX_CONCURRENCY);
                }
                _ => {
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
                    self.state.apply_entry(entry, self.config());
                }
            }
        }
        // Sync the shared counter past all IDs seen in this batch.
        self.id_counter.fetch_max(self.state.next_task_id, Ordering::SeqCst);
        // Side effects: spawn workers, detect shutdown.
        self.flush_dispatches();
    }
}

impl Engine {
    fn config(&self) -> &Config {
        self.config.as_ref().expect("[P051] config not set")
    }
}
```

`flush_dispatches` is unchanged in structure. When `pending_dispatches.is_empty() && in_flight == 0`, Engine sends `ControlFlow::Break(self.compute_result())` on `tx`.

During replay, `in_flight` stays at 0 throughout the seed batch — completion entries decrement via `saturating_sub` from 0, so nothing goes negative. After the batch, `flush_dispatches` dispatches any remaining pending work. Tasks that completed during replay had their `PendingDispatch::Task` removed when `apply_entry` processed their `TaskCompleted`. Finallys whose `FinallyRun` was in the batch were removed when `apply_entry` processed the `FinallyRun`. Any pending dispatch that survives to `flush_dispatches` is valid work that needs to run.

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

Writes every entry it receives. During resume, LogApplier writes all replayed entries to the new log file. This replaces the current raw-byte-copy optimization (`writer.write_all(&old_content)` at `runner/mod.rs:949`) with entry-by-entry re-serialization through the applier chain. The tradeoff is consistency (all entries always flow through all appliers) vs performance (re-serialization is slower than a byte copy). For correctness, consistency wins — the raw copy is a special case that bypasses the applier chain and would need separate maintenance.

### Termination

Engine detects workflow completion inside `flush_dispatches`: when `pending_dispatches.is_empty() && in_flight == 0`, it sends `Break(self.compute_result())` on `tx`. The coordinator receives `Break` and returns the result.

Both task workers and finally workers go through `flush_dispatches` and count toward `in_flight`, so shutdown naturally waits for all workers to complete. The result is `Ok(())` if no tasks were permanently dropped, `Err(..)` otherwise — derived from `dropped_count`.

### Unit testing

`RunState::apply_entry` is the inner pure function. It takes a single entry and mutates state — no channels, no dispatch, no I/O. Tests call it directly:

```rust
let mut state = RunState::new();
state.apply_entry(&seed(0, "A"), &cfg);
state.apply_entry(&success_with_children(0, vec![spawned(1, "B", 0)]), &cfg);
assert!(matches!(
    &state.tasks[&LogTaskId(0)].state,
    TaskState::WaitingForChildren(_)
));
```

The existing `run_state_tests` module (`runner/mod.rs:1011`) already tests at this level — 25 tests covering submitted/completed/finally/walk-up/replay scenarios. These require no changes.

Engine-level tests use a real channel but verify behavior through the receiver:

```rust
let (tx, rx) = mpsc::channel();
let counter = Arc::new(AtomicU32::new(0));
let mut engine = Engine::new(tx, counter, &runner_config);
engine.apply(&[config_entry, seed_entry, completed_entry]);
// Verify: state mutations, what was sent on tx (Break if done), etc.
```

## Sub-refactors

### WORKER_ENTRY_PRODUCTION.md (prerequisite)

Move result interpretation from Engine to workers so the channel carries `StateLogEntry` directly. Includes the shared `Arc<AtomicU32>` for worker-side ID allocation. Filed separately. This sub-refactor lands independently — the coordinator still holds concrete types, but the channel carries entries instead of raw results.

## Changes from current code

| Aspect | Current | Target |
|--------|---------|--------|
| Engine lifetime | `Engine<'a>` borrows `&'a Config` | `Engine` owns `Option<Config>`, no lifetime |
| Config handling | Written before Engine exists, separate from seed | First entry in seed batch, flows through appliers |
| Coordinator | `run_loop` with concrete Engine + BufWriter | `process_entries` over `Vec<Box<dyn Applier>>` |
| run/resume | Two separate functions with duplicated setup | Single `run(mode: RunMode, ...)` function |
| Channel type | `mpsc::channel::<WorkerResult>()` | `mpsc::channel::<ChannelMsg>()` (after WORKER_ENTRY_PRODUCTION) |
| Done detection | Coordinator calls `engine.is_done()` | Engine sends `Break(result)` on channel |
| Log writing | Free function, logs errors and continues | LogApplier struct, panics on write failure |
| Resume log copy | Raw byte copy of old file | Re-serialization through LogApplier |

## Open Questions

1. **`step_map` ownership**: Currently `step_map: HashMap<&'a StepName, &'a Step>` borrows from Config. With owned Config, this becomes `HashMap<StepName, Step>` (cloned) or a method that rebuilds it on demand. Since step lookups happen at dispatch time (not in hot loops), a method like `config.step(&name) -> &Step` may be simpler than maintaining a separate map.
