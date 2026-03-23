# Executor CLI Flag

**Parent:** JS_ACTION_RESOLUTION.md
**Depends on:** Nothing (purely additive Rust)

## Motivation

The JS executor needs Rust to spawn it. This sub-refactor adds the `--executor` CLI flag that tells barnum what command to use for task dispatch. When present, `dispatch_task` routes all tasks through the executor command instead of matching on `ActionKind`. When absent, current behavior is preserved. This enables incremental migration — the executor path can be tested before the legacy dispatch code is removed.

## Current State

### CLI (`crates/barnum_cli/src/lib.rs:52-83`)

The `Run` variant has no `--executor` flag.

### RunnerConfig (`crates/barnum_config/src/runner/mod.rs:39-48`)

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub invoker: &'a Invoker<TroupeCli>,
    pub state_log_path: &'a Path,
}
```

### Engine (`crates/barnum_config/src/runner/mod.rs:454-465`)

```rust
struct Engine<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    invoker: Invoker<TroupeCli>,
    working_dir: PathBuf,
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
}
```

### dispatch_task (`crates/barnum_config/src/runner/mod.rs:696-730`)

Matches on `ActionKind` to construct either `PoolAction` or `ShellAction`:

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    let tx = self.tx.clone();
    match &step.action {
        ActionKind::Pool(..) => {
            // ... construct PoolAction, call spawn_worker
        }
        ActionKind::Command(CommandAction { script }) => {
            // ... construct ShellAction, call spawn_worker
        }
    }
}
```

### ShellAction (`crates/barnum_config/src/runner/action.rs:205-280`)

```rust
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
}

impl Action for ShellAction {
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle {
        // Constructs: {"kind": step_name, "value": value}
        let task_json = serde_json::to_string(&serde_json::json!({
            "kind": &self.step_name,
            "value": &value,
        })).unwrap_or_default();

        // Spawns sh -c <script>, pipes task_json to stdin
        // ...
    }
}
```

## Proposed Changes

### 1. Add `--executor` to CLI

**File:** `crates/barnum_cli/src/lib.rs`

```rust
Run {
    #[arg(long, required_unless_present = "resume_from")]
    config: Option<String>,

    // ... existing fields ...

    /// Executor command for task dispatch (e.g., "npx tsx /path/to/executor.ts").
    /// When provided, all tasks are dispatched through this command instead of
    /// the built-in Pool/Command handlers.
    #[arg(long)]
    executor: Option<String>,

    #[arg(long, conflicts_with = "config")]
    resume_from: Option<PathBuf>,
}
```

The `--executor` flag is optional. When absent, barnum uses the existing `ActionKind` match dispatch. When present, barnum routes all tasks through the executor command.

Note: `--executor` must also work with `--resume-from`. The resume path needs the executor command because it's not stored in the state log.

### 2. Add executor_script to RunnerConfig

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub invoker: &'a Invoker<TroupeCli>,
    pub executor_script: Option<&'a str>,
    pub state_log_path: &'a Path,
}
```

### 3. Thread executor_script through Engine

**File:** `crates/barnum_config/src/runner/mod.rs`

Add `executor_script: Option<String>` and `config_json: serde_json::Value` to Engine. Pre-serialize the config once at construction so it's reused across all task dispatches.

```rust
struct Engine<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    invoker: Invoker<TroupeCli>,
    executor_script: Option<String>,
    config_json: serde_json::Value,    // pre-serialized config for envelope
    working_dir: PathBuf,
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
}
```

In `Engine::new`:

```rust
fn new(
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    invoker: Invoker<TroupeCli>,
    executor_script: Option<String>,
    working_dir: PathBuf,
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
) -> Self {
    let config_json = serde_json::to_value(config).unwrap_or_default();
    Self {
        config,
        schemas,
        step_map: config.step_map(),
        state: RunState::new(),
        invoker,
        executor_script,
        config_json,
        working_dir,
        tx,
        max_concurrency,
        in_flight: 0,
        dropped_count: 0,
    }
}
```

### 4. Add envelope context to ShellAction

**File:** `crates/barnum_config/src/runner/action.rs`

Add an optional field to ShellAction that carries the pre-serialized context for the enriched envelope. When present, `start()` pipes the full envelope instead of `{ kind, value }`.

```rust
/// Pre-serialized context for the enriched envelope.
/// When present, ShellAction pipes { action, task, step, config } instead of { kind, value }.
pub struct EnvelopeContext {
    pub action_json: serde_json::Value,
    pub step_json: serde_json::Value,
    pub config_json: serde_json::Value,
}

pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
    /// When Some, pipe enriched envelope. When None, pipe { kind, value }.
    pub envelope_context: Option<EnvelopeContext>,
}
```

In `ShellAction::start`:

```rust
fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle {
    let stdin_json = match &self.envelope_context {
        Some(ctx) => serde_json::to_string(&serde_json::json!({
            "action": ctx.action_json,
            "task": { "kind": &self.step_name, "value": &value },
            "step": ctx.step_json,
            "config": ctx.config_json,
        })),
        None => serde_json::to_string(&serde_json::json!({
            "kind": &self.step_name,
            "value": &value,
        })),
    }
    .unwrap_or_default();

    // ... rest of start() unchanged (spawn sh -c, pipe stdin, read stdout)
}
```

Existing callers (Command dispatch in legacy mode, `dispatch_finally`) pass `envelope_context: None` — their behavior is unchanged.

### 5. Dual-mode dispatch_task

**File:** `crates/barnum_config/src/runner/mod.rs`

When `executor_script` is `Some`, dispatch all tasks through the executor. When `None`, use the existing `ActionKind` match.

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    let tx = self.tx.clone();

    if let Some(executor) = &self.executor_script {
        // Executor mode: route all tasks through the JS executor
        info!(step = %task.step, "dispatching task via executor");
        let action = Box::new(ShellAction {
            script: executor.clone(),
            step_name: task.step.clone(),
            working_dir: self.working_dir.clone(),
            envelope_context: Some(EnvelopeContext {
                action_json: serde_json::to_value(&step.action).unwrap_or_default(),
                step_json: serde_json::to_value(step).unwrap_or_default(),
                config_json: self.config_json.clone(),
            }),
        });
        spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
    } else {
        // Legacy mode: match on ActionKind (existing code, unchanged)
        match &step.action {
            ActionKind::Pool(PoolActionConfig {
                pool, root, timeout: pool_timeout, ..
            }) => {
                let docs = generate_step_docs(step, self.config);
                info!(step = %task.step, "submitting task to pool");
                let action = Box::new(PoolAction {
                    root: root.clone(),
                    pool: pool.clone(),
                    invoker: self.invoker.clone(),
                    docs,
                    step_name: task.step.clone(),
                    pool_timeout: *pool_timeout,
                });
                spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
            }
            ActionKind::Command(CommandAction { script }) => {
                info!(step = %task.step, script = %script, "executing command");
                let action = Box::new(ShellAction {
                    script: script.clone(),
                    step_name: task.step.clone(),
                    working_dir: self.working_dir.clone(),
                    envelope_context: None,
                });
                spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
            }
        }
    }
}
```

### 6. Thread --executor through CLI main

**File:** `crates/barnum_cli/src/main.rs`

In `run_command`, pass the executor to `RunnerConfig`:

```rust
fn run_command(
    config: &str,
    initial_state: Option<&str>,
    entrypoint_value: Option<&str>,
    wake: Option<&str>,
    log_file: Option<&PathBuf>,
    state_log: Option<&PathBuf>,
    executor: Option<&str>,
    log_level: LogLevel,
) -> io::Result<()> {
    // ... existing code ...

    let runner_config = RunnerConfig {
        working_dir: &config_dir,
        wake_script: wake,
        invoker: &invoker,
        executor_script: executor,
        state_log_path: &state_log_path,
    };

    run(&cfg, &schemas, &runner_config, initial_tasks)
}
```

Same for `resume_command`.

### 7. dispatch_finally unchanged

`dispatch_finally` continues to construct `ShellAction` with `envelope_context: None`. Finally hooks receive `{ kind, value }` on stdin, as today.

```rust
fn dispatch_finally(&self, parent_id: LogTaskId, task: Task) {
    // ... existing code ...
    let action = Box::new(ShellAction {
        script: script.as_str().to_owned(),
        step_name: task.step.clone(),
        working_dir: self.working_dir.clone(),
        envelope_context: None,  // finally hooks get { kind, value }
    });
    // ...
}
```

## Testing

### Unit test: envelope context serialization

Add a test that constructs a `ShellAction` with `EnvelopeContext` and verifies the stdin JSON shape is correct. Uses a script like `cat` to echo stdin to stdout, then parse the result.

### Integration test: executor round-trip

Write a minimal executor script (e.g., `echo '[]'`) and run barnum with `--executor "sh -c 'cat >/dev/null && echo []'"`. Verify the workflow completes with zero follow-up tasks.

### Existing tests pass unchanged

When `--executor` is not provided, all existing tests pass without modification. The legacy dispatch path is unchanged.

## Backward Compatibility

When `--executor` is absent, behavior is identical to today. No existing configs, tests, or workflows are affected. The flag is purely additive.
