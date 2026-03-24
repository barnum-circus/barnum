# Add TypeScript Action Dispatch

**Parent:** TS_CONFIG.md
**Depends on:** UNIFY_STDIN_ENVELOPE, INLINE_RESOLVED_CONFIG

## Motivation

After FLATTEN_AND_RENAME_ACTION and UNIFY_STDIN_ENVELOPE, Rust has one action kind (`Bash`) with a unified envelope `{value, config, stepName}`. This refactor adds the TypeScript action kind on the Rust side: a new config type, config resolution (path canonicalization), and dispatch logic. From Rust's perspective, a TypeScript action is a shell command (`<executor> <run-handler.ts> <handler-path> <export>`) that receives an enriched envelope with `stepConfig` included.

## Current state (after UNIFY_STDIN_ENVELOPE)

```rust
// config.rs (after INLINE_RESOLVED_CONFIG merges the type hierarchies)
pub struct BashAction { pub script: String }

#[serde(tag = "kind")]
pub enum Action {
    Bash(BashAction),
}
```

The `Envelope` struct (from UNIFY_STDIN_ENVELOPE):

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    value: &'a serde_json::Value,
    config: &'a serde_json::Value,
    step_name: &'a StepName,
}
```

`ShellAction` stores `script`, `step_name`, `working_dir`, and `config: Arc<serde_json::Value>`.

## Changes

### 1. TypeScriptAction type

**File:** `crates/barnum_config/src/config.rs`

```rust
/// Run a TypeScript handler file as a subprocess.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TypeScriptAction {
    /// Path to the handler file.
    pub path: String,

    /// Named export to invoke from the handler module.
    #[serde(default = "default_exported_as")]
    pub exported_as: String,

    /// Step configuration passed through to the handler.
    /// Rust stores this as-is and includes it in the envelope.
    #[serde(default)]
    pub step_config: serde_json::Value,
}

fn default_exported_as() -> String { "default".to_string() }
```

One type used in the unified enum (after INLINE_RESOLVED_CONFIG, there's one enum, not two):

```rust
#[serde(tag = "kind")]
pub enum Action {
    Bash(BashAction),
    TypeScript(TypeScriptAction),
}
```

Config `{"kind": "TypeScript", "path": "./handlers/analyze.ts", "stepConfig": {...}}` deserializes with `exported_as = "default"` (serde default).

### 2. Path resolution (JS layer)

**File:** `libs/barnum/run.ts`

Rust receives config as a JSON string via `--config` — it has no concept of "config file directory." The JS layer canonicalizes `path` relative to the caller's directory before passing the config to Rust. This validates the handler file exists at config load time — a typo fails immediately, not at dispatch time.

In `BarnumConfig.fromConfig()` or `.run()`, resolve TypeScript action paths:

```typescript
import { resolve } from "node:path";

// Before passing config to Rust, canonicalize TypeScript handler paths
for (const step of config.steps) {
  if (step.action.kind === "TypeScript") {
    step.action.path = resolve(step.action.path);
  }
}
```

Rust receives absolute paths and passes them through to the subprocess invocation unchanged.

### 3. Add `step_config` to Envelope and ShellAction

**File:** `crates/barnum_config/src/runner/action.rs`

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    step_config: Option<&'a serde_json::Value>,
    value: &'a serde_json::Value,
    config: &'a serde_json::Value,
    step_name: &'a StepName,
}
```

```rust
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
    pub config: Arc<serde_json::Value>,
    pub step_config: Option<serde_json::Value>,
}
```

No separate `TypeScriptShellAction`. Both Bash and TypeScript use the same `ShellAction` struct. The differences:

- **Bash**: `step_config: None` (omitted from envelope), `script` is the user's script
- **TypeScript**: `step_config: Some(...)`, `script` is `"{executor} {run_handler_path} {path} {exported_as}"`

`ShellAction::start` constructs the envelope from stored fields:

```rust
fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle {
    let envelope = Envelope {
        step_config: self.step_config.as_ref(),
        value: &value,
        config: &self.config,
        step_name: &self.step_name,
    };
    let task_json = serde_json::to_string(&envelope).unwrap_or_default();
    // ... rest unchanged
}
```

### 4. Dispatch

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);

    match &step.action {
        Action::Bash(BashAction { script }) => {
            info!(step = %task.step, script = %script, "executing command");
            let action = Box::new(ShellAction {
                script: script.clone(),
                step_name: task.step.clone(),
                working_dir: self.working_dir.clone(),
                config: Arc::clone(&self.config_json),
                step_config: None,
            });
            spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
        }
        Action::TypeScript(TypeScriptAction { path, exported_as, ref step_config }) => {
            let script = format!(
                "{} {} {path} {exported_as}",
                self.executor, self.run_handler_path,
            );
            info!(step = %task.step, handler = %path, "dispatching TypeScript handler");
            let action = Box::new(ShellAction {
                script,
                step_name: task.step.clone(),
                working_dir: self.working_dir.clone(),
                config: Arc::clone(&self.config_json),
                step_config: Some(step_config.clone()),
            });
            spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
        }
    }
}
```

### 5. RunnerConfig changes

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub state_log_path: &'a Path,
    /// Executor command for TypeScript handlers (e.g., "pnpm dlx tsx").
    pub executor: &'a str,
    /// Path to run-handler.ts.
    pub run_handler_path: &'a str,
}
```

Both fields are required. The JS layer (`run.ts`) always provides both via CLI flags.

### 6. Engine changes

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
struct Engine<'a> {
    // ... existing fields ...
    executor: String,
    run_handler_path: String,
}
```

`Engine::new` takes the executor and run_handler_path from `RunnerConfig`.

### 7. CLI changes

**File:** `crates/barnum_cli/src/main.rs`

Wire `--executor` and `--run-handler-path` CLI flags through to `RunnerConfig`:

```rust
let runner_config = RunnerConfig {
    working_dir: &config_dir,
    wake_script: wake.as_deref(),
    state_log_path: &state_log_path,
    executor: &executor,
    run_handler_path: &run_handler_path,
};
```

### 8. Schemas

Regenerate all schema artifacts. The TypeScript variant appears in the generated schemas. `stepConfig` is typed as an arbitrary JSON value.

## Tests

### Config parsing

```rust
#[test]
fn action_typescript_with_step_config() {
    let json = r#"{
        "steps": [{
            "name": "Test",
            "action": {
                "kind": "TypeScript",
                "path": "./handler.ts",
                "stepConfig": {
                    "instructions": "Do stuff",
                    "pool": "demo"
                }
            },
            "next": [],
            "options": { "maxRetries": 0, "retryOnTimeout": true, "retryOnInvalidResponse": true }
        }]
    }"#;
    let config: Config = serde_json::from_str(json).expect("parse");
    match &config.steps[0].action {
        Action::TypeScript(ts) => {
            assert_eq!(ts.path, "./handler.ts");
            assert_eq!(ts.exported_as, "default");
            assert_eq!(ts.step_config["instructions"], "Do stuff");
        }
        _ => panic!("expected TypeScript action"),
    }
}
```

### Envelope with step_config

```rust
#[test]
fn envelope_includes_step_config_for_typescript() {
    let config = serde_json::json!({"steps": []});
    let value = serde_json::json!({"file": "src/main.rs"});
    let step_config = serde_json::json!({"instructions": "Analyze"});
    let step_name = StepName::new("Analyze");

    let envelope = Envelope {
        step_config: Some(&step_config),
        value: &value,
        config: &config,
        step_name: &step_name,
    };

    let json: serde_json::Value = serde_json::to_value(&envelope).unwrap();
    assert_eq!(json["stepConfig"]["instructions"], "Analyze");
    assert_eq!(json["stepName"], "Analyze");
}

#[test]
fn envelope_omits_step_config_for_bash() {
    let config = serde_json::json!({"steps": []});
    let value = serde_json::json!({"file": "src/main.rs"});
    let step_name = StepName::new("Start");

    let envelope = Envelope {
        step_config: None,
        value: &value,
        config: &config,
        step_name: &step_name,
    };

    let json: serde_json::Value = serde_json::to_value(&envelope).unwrap();
    assert!(json.get("stepConfig").is_none());
}
```

## What this does NOT do

- Does not define the TypeScript handler interface (ADD_HANDLER_VALIDATION)
- Does not implement run-handler.ts (ADD_RUN_HANDLER)
- Does not change run.ts or inject --executor from JS (ADD_RUN_HANDLER)
- Does not implement step constructors or `.validate()`
