# Unify Stdin Envelope

**Depends on:** FLATTEN_AND_RENAME_ACTION
**Blocks:** ADD_TYPESCRIPT_DISPATCH

## Motivation

The stdin envelope that Rust sends to action subprocesses currently contains two fields: `kind` (the step name) and `value` (the task payload). TypeScript handlers need richer context — the full config, step name, and eventually step configuration. Rather than having Bash and TypeScript actions receive different envelopes, unify the format now. The envelope matches the handler context shape: `{ value, config, stepName }` for Bash. When TypeScript actions land later, they add `stepConfig` to the same envelope.

## Current state

**File:** `crates/barnum_config/src/runner/action.rs:170-174`

```rust
let task_json = serde_json::to_string(&serde_json::json!({
    "kind": &self.step_name,
    "value": &value,
}))
.unwrap_or_default();
```

This produces:

```json
{"kind": "Analyze", "value": {"file": "src/main.rs"}}
```

`ShellAction` stores three fields:

```rust
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
}
```

The `Action` trait passes `value` as a parameter to `start`:

```rust
pub trait Action: Send {
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle;
}
```

## Changes

### 1. New `Envelope` struct

**File:** `crates/barnum_config/src/runner/action.rs`

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    value: &'a serde_json::Value,
    config: &'a serde_json::Value,
    step_name: &'a StepName,
}
```

This produces:

```json
{
  "value": {"file": "src/main.rs"},
  "config": { ... },
  "stepName": "Analyze"
}
```

The `Envelope` struct is file-private — callers don't need to know about it. ADD_TYPESCRIPT_DISPATCH will add a `step_config: Option<&'a serde_json::Value>` field with `#[serde(skip_serializing_if = "Option::is_none")]` so that Bash actions omit it and TypeScript actions include it.

### 2. Add serialized config to `ShellAction`

**File:** `crates/barnum_config/src/runner/action.rs`

```rust
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
    pub config: Arc<serde_json::Value>,
}
```

`config` is `Arc<serde_json::Value>` to avoid cloning the full config on every dispatch. The Engine serializes the config once and shares the reference.

### 3. Update `ShellAction::start`

**File:** `crates/barnum_config/src/runner/action.rs`

Replace the inline `serde_json::json!` with the `Envelope` struct:

```rust
fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle {
    let envelope = Envelope {
        value: &value,
        config: &self.config,
        step_name: &self.step_name,
    };
    let task_json = serde_json::to_string(&envelope).unwrap_or_default();
    // ... rest unchanged
}
```

The `Action` trait signature is unchanged — `value` is still the varying parameter per-task, while `config` and `step_name` are determined at dispatch time.

### 4. Serialized config in `Engine`

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
struct Engine<'a> {
    config: &'a Config,
    config_json: Arc<serde_json::Value>,
    // ... existing fields unchanged
}
```

`Engine::new` serializes the config once:

```rust
fn new(config: &'a Config, working_dir: PathBuf, tx: mpsc::Sender<WorkerResult>, max_concurrency: usize) -> Self {
    let config_json = Arc::new(
        serde_json::to_value(config).expect("[P081] config serialization")
    );
    Self {
        config,
        config_json,
        // ...
    }
}
```

### 5. Pass config to dispatch sites

**File:** `crates/barnum_config/src/runner/mod.rs`

Both `dispatch_task` and `dispatch_finally` pass `Arc::clone(&self.config_json)` when constructing `ShellAction`:

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);

    match &step.action {
        ActionKind::Bash(BashAction { script }) => {
            let action = Box::new(ShellAction {
                script: script.clone(),
                step_name: task.step.clone(),
                working_dir: self.working_dir.clone(),
                config: Arc::clone(&self.config_json),
            });
            spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
        }
    }
}
```

`dispatch_finally` follows the same pattern.

### 6. Update demo bash scripts

The demo scripts that read stdin use `.value.*` paths (e.g., `jq -r '.value.folder'`). None of them read `.kind`. The stdin field rename from `kind` to `stepName` doesn't break any existing scripts. The new `config` and `stepName` fields are additive — scripts that only read `.value` continue working.

The `command-script` demo scripts (`list-files.sh`, `process.sh`) read `.value` and are unaffected. The troupe-based demos (`linear`, `branching`, etc.) pipe stdin to `$TASK` and pass it through to troupe — they also only access `.value`.

No demo bash script changes are needed.

### 7. Update integration tests

The integration test scripts (`cli_integration.rs`) use commands like `echo '[]'` that ignore stdin entirely. The `FileWriterAgent` test helper reads troupe protocol envelopes (a different format), not barnum action stdin. No test changes are needed for the envelope format.

The action stdin format change is invisible to the test suite because no test inspects or parses the stdin payload.

### 8. Update documentation

The task format reference at `docs-website/docs/reference/task-format.md` documents the stdin contract. Update it to reflect `{value, config, stepName}`.

## Tests

### Envelope serialization test

```rust
#[test]
fn envelope_serializes_to_camel_case() {
    let config = serde_json::json!({"steps": []});
    let value = serde_json::json!({"file": "src/main.rs"});
    let step_name = StepName::new("Analyze");

    let envelope = Envelope {
        value: &value,
        config: &config,
        step_name: &step_name,
    };

    let json: serde_json::Value = serde_json::to_value(&envelope).unwrap();
    assert_eq!(json["stepName"], "Analyze");
    assert_eq!(json["value"]["file"], "src/main.rs");
    assert!(json.get("config").is_some());
    assert!(json.get("kind").is_none()); // old field absent
}
```

### Integration: verify end-to-end

Existing integration tests exercise Bash actions with the new envelope format. If they pass, the format is correct. A targeted test can verify the envelope reaches the script by having a Bash script write `$(cat)` to a file and checking its structure:

```rust
#[test]
fn bash_action_receives_unified_envelope() {
    // Config with a script that writes stdin to a temp file
    // Verify the file contains stepName, value, config (not kind)
}
```

## What this does NOT do

- Does not add `stepConfig` — that field arrives with ADD_TYPESCRIPT_DISPATCH
- Does not change the stdout format (follow-up tasks remain `[{"kind": "...", "value": ...}]`)
- Does not change the `Action` trait signature
- Does not change the state log format
