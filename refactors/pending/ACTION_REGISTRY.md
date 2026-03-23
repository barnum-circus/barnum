# Action Registry

## Motivation

`Engine::dispatch_task` matches on `ActionKind` variants to construct the appropriate `Action` impl. Adding a new action kind requires adding a match arm in the Engine. The Engine should be kind-agnostic: it looks up a factory by name and delegates construction.

This is the mechanical infrastructure step toward pluggable action kinds. The registry is pre-constructed with two entries (Pool and Command) and is not externally extensible in this version.

## Current State

### dispatch_task (`crates/barnum_config/src/runner/mod.rs:703-731`)

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    let tx = self.tx.clone();

    match &step.action {
        ActionKind::Pool(..) => {
            let docs = generate_step_docs(step, self.config);
            info!(step = %task.step, "submitting task to pool");
            let action = Box::new(PoolAction {
                root: self.pool.root.clone(),
                invoker: self.pool.invoker.clone(),
                docs,
                step_name: task.step.clone(),
                pool_timeout: step.options.timeout,
            });
            spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
        }
        ActionKind::Command(CommandAction { script }) => {
            info!(step = %task.step, script = %script, "executing command");
            let action = Box::new(ShellAction {
                script: script.clone(),
                step_name: task.step.clone(),
                working_dir: self.pool.working_dir.clone(),
            });
            spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
        }
    }
}
```

This match is the only place in the Engine that knows about specific action kinds. Everything downstream (spawn_worker, run_action, ActionHandle) is already generic.

### dispatch_finally (`mod.rs:734-757`)

Always creates a ShellAction from the step's `finally_hook`. No kind dispatch. This stays unchanged.

### Engine fields (`mod.rs:464-474`)

```rust
struct Engine<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    pool: PoolConnection,  // holds root, working_dir, invoker
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
}
```

`PoolConnection` bundles three fields: `root: PathBuf`, `working_dir: PathBuf`, `invoker: Invoker<TroupeCli>`. After this refactor, `root` and `invoker` move into `PoolActionFactory`, `working_dir` moves into `CommandActionFactory` and stays on Engine (for `dispatch_finally`). `PoolConnection` is deleted.

## Proposed Changes

### 1. ActionKind::kind_name()

**File:** `crates/barnum_config/src/resolved.rs`

```rust
impl ActionKind {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Pool(..) => "Pool",
            Self::Command(..) => "Command",
        }
    }
}
```

### 2. ActionFactory trait

**File:** `crates/barnum_config/src/runner/action.rs`

```rust
pub(super) trait ActionFactory: Send + Sync {
    fn build(&self, step: &Step, config: &Config) -> Box<dyn Action>;
}
```

The factory holds captured state (pool connection details, working directory) and uses the step and config to construct the Action.

### 3. Factory implementations

**File:** `crates/barnum_config/src/runner/action.rs`

New imports needed: `use crate::docs::generate_step_docs;` and `use crate::resolved::{ActionKind, CommandAction, Config, Step};`.

```rust
pub(super) struct PoolActionFactory {
    root: PathBuf,
    invoker: Invoker<TroupeCli>,
}

impl ActionFactory for PoolActionFactory {
    fn build(&self, step: &Step, config: &Config) -> Box<dyn Action> {
        let docs = generate_step_docs(step, config);
        Box::new(PoolAction {
            root: self.root.clone(),
            invoker: self.invoker.clone(),
            docs,
            step_name: step.name.clone(),
            pool_timeout: step.options.timeout,
        })
    }
}

pub(super) struct CommandActionFactory {
    working_dir: PathBuf,
}

impl ActionFactory for CommandActionFactory {
    #[expect(clippy::expect_used)]
    fn build(&self, step: &Step, _config: &Config) -> Box<dyn Action> {
        let ActionKind::Command(CommandAction { script }) = &step.action else {
            panic!("[P081] CommandActionFactory called with non-Command action");
        };
        Box::new(ShellAction {
            script: script.clone(),
            step_name: step.name.clone(),
            working_dir: self.working_dir.clone(),
        })
    }
}
```

### 4. ActionRegistry

**File:** `crates/barnum_config/src/runner/action.rs`

```rust
pub(super) struct ActionRegistry {
    factories: HashMap<String, Box<dyn ActionFactory>>,
}

impl ActionRegistry {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    pub fn register(&mut self, kind: &str, factory: impl ActionFactory + 'static) {
        self.factories.insert(kind.to_owned(), Box::new(factory));
    }

    #[expect(clippy::expect_used)]
    pub fn build(&self, step: &Step, config: &Config) -> Box<dyn Action> {
        let kind = step.action.kind_name();
        self.factories
            .get(kind)
            .unwrap_or_else(|| panic!("[P082] unknown action kind: {kind}"))
            .build(step, config)
    }
}
```

### 5. Engine changes

**File:** `crates/barnum_config/src/runner/mod.rs`

Engine replaces `pool: PoolConnection` with `registry: ActionRegistry` and `working_dir: PathBuf`:

```rust
struct Engine<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    registry: ActionRegistry,
    working_dir: PathBuf,
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
}
```

`dispatch_task` becomes kind-agnostic:

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    info!(step = %task.step, kind = step.action.kind_name(), "dispatching task");
    let action = self.registry.build(step, self.config);
    spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
}
```

`dispatch_finally` uses `self.working_dir` instead of `self.pool.working_dir`:

```rust
fn dispatch_finally(&self, parent_id: LogTaskId, task: Task) {
    // ... same as before, but:
    let action = Box::new(ShellAction {
        script: script.as_str().to_owned(),
        step_name: task.step.clone(),
        working_dir: self.working_dir.clone(),
    });
    // ...
}
```

### 6. Registry construction

**File:** `crates/barnum_config/src/runner/mod.rs`

In both `run` and `resume`, replace `PoolConnection` construction with registry construction:

```rust
let registry = {
    let mut r = ActionRegistry::new();
    r.register("Pool", PoolActionFactory {
        root: runner_config.troupe_root.to_path_buf(),
        invoker: Clone::clone(runner_config.invoker),
    });
    r.register("Command", CommandActionFactory {
        working_dir: runner_config.working_dir.to_path_buf(),
    });
    r
};
let working_dir = runner_config.working_dir.to_path_buf();

let mut engine = Engine::new(config, schemas, registry, working_dir, tx, max_concurrency);
```

### 7. Delete PoolConnection

**File:** `crates/barnum_config/src/runner/mod.rs`

`PoolConnection` (lines 56-60) and its `#[derive(Clone)]` are deleted. All its data now lives in the factories or on Engine directly.

## What doesn't change

- Config types (`ActionKind` enum, `config.rs`, JSON schema, generated artifacts)
- `ActionHandle`, `run_action`, `spawn_worker` (already generic)
- `dispatch_finally` (always ShellAction, just uses `self.working_dir` instead of `self.pool.working_dir`)
- `Config::has_pool_actions()` in `resolved.rs` (still matches on the enum, lives in the config layer)
- Tests (RunState tests don't touch dispatch; `build_payload_includes_task_and_docs` tests submit.rs)
- JS layer

## Relationship to PLUGGABLE_ACTION_KINDS

PLUGGABLE_ACTION_KINDS describes the full vision: JS-side resolution of arbitrary kind strings into execution primitives, custom kind registration, open config format. This refactor is the first concrete step: making the Engine kind-agnostic by routing through a registry. The registry is closed (pre-constructed with two built-in entries) and will be opened in later work.
