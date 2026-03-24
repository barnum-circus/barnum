# Add TypeScript Action Dispatch (Rust)

**Parent:** TS_CONFIG.md
**Depends on:** UNIFY_STDIN_ENVELOPE, INLINE_RESOLVED_CONFIG

## Motivation

Add a `TypeScript` variant to `ActionKind`. From Rust's perspective, it's just a different shell command with an extra `stepConfig` field in the envelope.

## Changes

### 1. Add `TypeScriptAction` to config

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

#[serde(tag = "kind")]
pub enum ActionKind {
    Bash(BashAction),
    TypeScript(TypeScriptAction),
}
```

### 2. Extract shared dispatch, add TypeScript arm

Both variants produce a `ShellAction` — they differ only in the script string and whether `step_config` is present. Extract the common body into a helper:

```rust
fn dispatch_shell_action(&self, task_id: LogTaskId, task: Task, script: String, step_config: Option<serde_json::Value>, timeout: Option<Duration>) {
    let action = Box::new(ShellAction {
        script,
        step_name: task.step.clone(),
        working_dir: self.working_dir.clone(),
        config: self.config_json.clone(),
        step_config,
    });
    spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
}
```

TypeScript arm builds the script as:

```rust
format!("{} '{}' '{}' '{}'", self.executor, self.run_handler_path, path, exported_as)
```

### 3. Wire `executor` and `run_handler_path` through

The `executor` CLI flag already exists (hidden, `Option<String>`). Make it required (not `Option`). Add `run_handler_path` the same way. Both go into `RunnerConfig` → `Engine`.

### 4. Add `step_config` to Envelope

`Envelope` gets `#[serde(skip_serializing_if = "Option::is_none")] step_config: Option<&'a serde_json::Value>`. `ShellAction` stores `step_config: Option<serde_json::Value>`. Bash passes `None` (omitted from JSON), TypeScript passes `Some(...)`.

## What this does NOT do

- TypeScript handler interface (ADD_HANDLER_VALIDATION)
- run-handler.ts implementation (ADD_RUN_HANDLER)
- Path resolution (ADD_RUN_HANDLER — JS layer resolves before passing to Rust)
