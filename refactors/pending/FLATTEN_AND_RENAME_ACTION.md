# Flatten Action Params and Rename Command to Bash

**Blocks:** DEMOS_TO_TYPESCRIPT (this must land first)

## Motivation

The action enum uses adjacently tagged serde (`tag = "kind", content = "params"`), producing configs like:

```json
{"kind": "Command", "params": {"script": "echo hello"}}
```

The `params` wrapper was useful when action kinds were dynamically registered. They're not anymore — only `Command` exists (and soon `Bash` + `TypeScript`). The nesting is noise. The target is:

```json
{"kind": "Bash", "script": "echo hello"}
```

Additionally, all config/resolved structs serialize field names as snake_case (e.g. `max_retries`, `retry_on_timeout`). The JSON convention should be camelCase (`maxRetries`, `retryOnTimeout`). Adding `#[serde(rename_all = "camelCase")]` to every serialized struct fixes this universally.

## Current state

**ActionFile enum** (`config.rs:135-141`):
```rust
#[serde(tag = "kind", content = "params")]
pub enum ActionFile {
    Command(CommandActionFile),
}
```

**ActionKind enum** (`resolved.rs:59-65`):
```rust
#[serde(tag = "kind", content = "params")]
pub enum ActionKind {
    Command(CommandAction),
}
```

**Structs with snake_case fields that serialize to JSON:**

| Struct | File | Snake_case fields |
|--------|------|-------------------|
| `ConfigFile` | `config.rs:17` | `schema_ref` |
| `Options` | `config.rs:42` | `max_retries`, `max_concurrency`, `retry_on_timeout`, `retry_on_invalid_response` |
| `StepFile` | `config.rs:85` | `finally_hook` (has explicit rename to `finally`) |
| `StepOptions` | `config.rs:168` | `max_retries`, `retry_on_timeout`, `retry_on_invalid_response` |
| `EffectiveOptions` | `config.rs:189` | `max_retries`, `retry_on_timeout`, `retry_on_invalid_response` |
| `Config` | `resolved.rs:15` | `max_concurrency` |
| `Options` (resolved) | `resolved.rs:69` | `max_retries`, `retry_on_timeout`, `retry_on_invalid_response` |
| Step (resolved) | `resolved.rs` | `finally_hook` (has explicit rename to `finally`) |

## Changes

### 1. Rename Command to Bash

Mechanical rename across all files:

- `CommandActionFile` → `BashActionFile`
- `CommandAction` → `BashAction`
- `ActionFile::Command` → `ActionFile::Bash`
- `ActionKind::Command` → `ActionKind::Bash`

Grep for `CommandActionFile`, `CommandAction`, `ActionFile::Command`, `ActionKind::Command` and update every occurrence. This affects config types, resolved types, the resolve method, dispatch, tests, and demo configs.

### 2. Remove params nesting

Change both enums from adjacently tagged to internally tagged:

```rust
// config.rs
// Before: #[serde(tag = "kind", content = "params")]
// After:
#[serde(tag = "kind")]
pub enum ActionFile {
    Bash(BashActionFile),
}

// resolved.rs
// Before: #[serde(tag = "kind", content = "params")]
// After:
#[serde(tag = "kind")]
pub enum ActionKind {
    Bash(BashAction),
}
```

### 3. Add rename_all = "camelCase" to all serialized structs

Every struct that derives `Serialize`/`Deserialize` and has snake_case fields gets `#[serde(rename_all = "camelCase")]`. Fields with explicit `#[serde(rename = "...")]` (like `finally_hook` → `finally` and `schema_ref` → `$schema`) keep their per-field renames — those override the container rule.

```rust
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    pub max_retries: u32,          // → "maxRetries"
    pub max_concurrency: Option<usize>, // → "maxConcurrency"
    pub retry_on_timeout: bool,    // → "retryOnTimeout"
    pub retry_on_invalid_response: bool, // → "retryOnInvalidResponse"
}
```

Apply to: `ConfigFile`, `Options` (config), `StepFile`, `StepOptions`, `EffectiveOptions`, `Config` (resolved), `Options` (resolved), `Step` (resolved), `CommandAction`/`BashAction`, `CommandActionFile`/`BashActionFile`.

Internal-only structs (`RunnerConfig`, `ShellAction`, `ActionHandle`) that don't serialize to user-facing JSON don't need it.

### 4. Update demo configs

All demo `*.json` and `*.jsonc` files:

- Change `"kind": "Command"` to `"kind": "Bash"`
- Remove `"params"` wrapper: `{"kind": "Bash", "params": {"script": "..."}}` → `{"kind": "Bash", "script": "..."}`
- Rename snake_case keys to camelCase: `max_retries` → `maxRetries`, etc.

### 5. Update tests

All test files that construct JSON configs or Rust config types:

- Update enum variant names
- Update JSON strings to remove `params` nesting
- Update JSON field names to camelCase
- Update Rust struct field references if needed

### 6. Update run-demo.ts files

The `run-demo.ts` files load `config.json` — they don't need changes themselves, but the JSON they load changes.

### 7. Regenerate schemas

```bash
cargo run -p barnum_cli --bin build_schemas
```

The generated schemas will reflect the new enum tagging and camelCase field names.

### 8. Update docs generation

`crates/barnum_config/src/docs.rs` generates markdown documentation from config types. Verify it handles the new serialization format correctly (camelCase field names in generated docs).

## Sequencing

This is a single atomic change — rename, flatten, and camelCase all happen together since they all affect the same JSON format. Landing them separately would mean updating all configs/tests twice.

## What this does NOT do

- Does not add the TypeScript action kind (that's ADD_TYPESCRIPT_ACTION)
- Does not change the stdin envelope format for actions (still `{"kind": ..., "value": ...}`)
- Does not change `FinallyHook`/`HookScript` types (their serde is separate)
