# Remove Config from Stdin Envelope

**Blocks:** ADD_TYPESCRIPT_DISPATCH

## Motivation

UNIFY_STDIN_ENVELOPE added the full workflow config to every subprocess's stdin envelope: `{value, config, stepName}`. No action type reads `config` from stdin. Bash scripts read `.value`. TypeScript handlers (when added) will use `stepConfig`, not the full workflow graph. Sending the entire config on every invocation is dead weight.

## Current state

### Envelope (action.rs:162-168)

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    value: &'a serde_json::Value,
    config: &'a serde_json::Value,
    step_name: &'a StepName,
}
```

### ShellAction (action.rs:171-176)

```rust
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub config: serde_json::Value,
    pub working_dir: PathBuf,
}
```

### Engine (mod.rs:453-455)

```rust
struct Engine<'a> {
    config: &'a Config,
    config_json: serde_json::Value,
    // ...
}
```

`config_json` exists solely to be cloned into `ShellAction` for the envelope. The `config: &Config` reference is what the engine actually uses for step lookup and option resolution.

### run / resume (mod.rs:821-831, 919-926)

Both `run` and `resume` serialize the config to `serde_json::Value`, pass it to `Engine::new`, and also write it to the state log. After this change, the state log write is the only consumer of the serialized config.

## Changes

### 1. Remove `config` from Envelope

**File:** `crates/barnum_config/src/runner/action.rs`

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    value: &'a serde_json::Value,
    step_name: &'a StepName,
}
```

### 2. Remove `config` from ShellAction

**File:** `crates/barnum_config/src/runner/action.rs`

```rust
pub struct ShellAction {
    pub script: String,
    pub step_name: StepName,
    pub working_dir: PathBuf,
}
```

Update `ShellAction::start` to construct the envelope without `config`.

### 3. Remove `config_json` from Engine

**File:** `crates/barnum_config/src/runner/mod.rs`

Delete the `config_json: serde_json::Value` field from `Engine`. Remove it from `Engine::new`'s parameters. Remove the clones in `dispatch_task` and `dispatch_finally`.

### 4. Simplify run / resume

**File:** `crates/barnum_config/src/runner/mod.rs`

In `run`: `config_json` is serialized for the state log entry, then no longer passed to `Engine::new`.

```rust
let config_json =
    serde_json::to_value(config).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
let config_entry = StateLogEntry::Config(StateLogConfig { config: config_json });
write_log(&mut log_writer, &config_entry);

let mut engine = Engine::new(config, runner_config.working_dir.to_path_buf(), tx, max_concurrency);
```

Same pattern in `resume`.

### 5. Update docs

**File:** `docs-website/docs/reference/task-format.md`

Update the envelope documentation to reflect `{value, stepName}` (no `config`).

## Tests

Existing integration tests pass without modification. Bash scripts in the demos read `.value` from stdin, not `.config`. The `Envelope` unit tests (if any exist for the current shape) update to remove the `config` field.
