# Add TypeScript Action Dispatch (Rust)

**Parent:** TS_CONFIG.md
**Depends on:** UNIFY_STDIN_ENVELOPE, INLINE_RESOLVED_CONFIG

## Motivation

Add a `TypeScript` variant to `ActionKind`. From Rust's perspective, it's just a different shell command with an extra `stepConfig` field in the envelope.

## Changes

### 1. Add `TypeScriptAction` to config

**File:** `crates/barnum_config/src/config.rs`

Before:

```rust
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum ActionKind {
    Bash(BashAction),
}
```

After:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TypeScriptAction {
    pub path: String,
    #[serde(default = "default_exported_as")]
    pub exported_as: String,
    #[serde(default)]
    pub step_config: serde_json::Value,
}

fn default_exported_as() -> String { "default".to_string() }

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum ActionKind {
    Bash(BashAction),
    TypeScript(TypeScriptAction),
}
```

Export `TypeScriptAction` from `lib.rs` alongside `BashAction`.

### 2. Add `step_config` to `Envelope` and `ShellAction`

**File:** `crates/barnum_config/src/runner/action.rs`

Before:

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    value: &'a serde_json::Value,
    config: &'a serde_json::Value,
    step_name: &'a StepName,
}

pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub config: serde_json::Value,
    pub working_dir: PathBuf,
}
```

After:

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    value: &'a serde_json::Value,
    config: &'a serde_json::Value,
    step_name: &'a StepName,
    #[serde(skip_serializing_if = "Option::is_none")]
    step_config: Option<&'a serde_json::Value>,
}

pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub config: serde_json::Value,
    pub working_dir: PathBuf,
    pub step_config: Option<serde_json::Value>,
}
```

Update the `Envelope` construction in `ShellAction::start`:

Before:

```rust
let task_json = serde_json::to_string(&Envelope {
    value: &value,
    config: &self.config,
    step_name: &self.step_name,
})
.unwrap_or_default();
```

After:

```rust
let task_json = serde_json::to_string(&Envelope {
    value: &value,
    config: &self.config,
    step_name: &self.step_name,
    step_config: self.step_config.as_ref(),
})
.unwrap_or_default();
```

### 3. Extract shared dispatch helper, add TypeScript arm

**File:** `crates/barnum_config/src/runner/mod.rs`

Before (`dispatch_task`):

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let effective = EffectiveOptions::resolve(&self.config.options, &step.options);
    let timeout = effective.timeout.map(Duration::from_secs);
    let tx = self.tx.clone();

    match &step.action {
        ActionKind::Bash(BashAction { script }) => {
            info!(step = %task.step, script = %script, "executing command");
            let action = Box::new(ShellAction {
                script: script.clone(),
                step_name: task.step.clone(),
                config: self.config_json.clone(),
                working_dir: self.working_dir.clone(),
            });
            spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
        }
    }
}
```

After:

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let effective = EffectiveOptions::resolve(&self.config.options, &step.options);
    let timeout = effective.timeout.map(Duration::from_secs);

    let (script, step_config) = match &step.action {
        ActionKind::Bash(BashAction { script }) => {
            info!(step = %task.step, script = %script, "executing command");
            (script.clone(), None)
        }
        ActionKind::TypeScript(ts) => {
            let script = format!(
                "{} '{}' '{}' '{}'",
                self.executor, self.run_handler_path, ts.path, ts.exported_as,
            );
            info!(step = %task.step, handler = %ts.path, "dispatching TypeScript handler");
            (script, Some(ts.step_config.clone()))
        }
    };

    let action = Box::new(ShellAction {
        script,
        step_name: task.step.clone(),
        config: self.config_json.clone(),
        working_dir: self.working_dir.clone(),
        step_config,
    });
    spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
}
```

Also update `dispatch_finally` to pass `step_config: None`.

### 4. Wire `executor` and `run_handler_path` into `RunnerConfig` and `Engine`

**File:** `crates/barnum_config/src/runner/mod.rs`

Before:

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub state_log_path: &'a Path,
}
```

```rust
struct Engine<'a> {
    config: &'a Config,
    config_json: serde_json::Value,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    working_dir: PathBuf,
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
}
```

After:

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub state_log_path: &'a Path,
    pub executor: &'a str,
    pub run_handler_path: &'a str,
}
```

```rust
struct Engine<'a> {
    config: &'a Config,
    config_json: serde_json::Value,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    working_dir: PathBuf,
    executor: String,
    run_handler_path: String,
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
}
```

`Engine::new` takes executor/run_handler_path from `RunnerConfig` and `.to_string()`s them.

### 5. CLI: make `executor` required, add `run_handler_path`

**File:** `crates/barnum_cli/src/lib.rs`

Before:

```rust
/// Internal: executor command injected by cli.cjs.
/// Not user-facing — hidden from --help.
#[arg(long, hide = true)]
executor: Option<String>,
```

After:

```rust
/// Executor command for TypeScript handlers (e.g. "pnpm dlx tsx").
#[arg(long, hide = true)]
executor: String,

/// Path to run-handler.ts.
#[arg(long, hide = true)]
run_handler_path: String,
```

**File:** `crates/barnum_cli/src/main.rs`

Before:

```rust
Command::Run {
    config,
    initial_state,
    entrypoint_value,
    wake,
    log_file,
    state_log,
    resume_from,
    executor: _,
} => ...
```

After: stop ignoring `executor`, destructure `run_handler_path`, pass both through to `RunnerConfig`.

### 6. Regenerate schemas

Run `cargo run -p barnum_cli --bin build_schemas` to regenerate all schema artifacts. The TypeScript variant and new CLI flags appear in the generated output.

## What this does NOT do

- TypeScript handler interface (ADD_HANDLER_VALIDATION)
- run-handler.ts implementation (ADD_RUN_HANDLER)
- Path resolution (ADD_RUN_HANDLER — JS layer resolves before passing to Rust)
