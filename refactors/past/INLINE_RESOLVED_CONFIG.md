# Inline Resolved Config Types

**Depends on:** DEMOS_TO_TYPESCRIPT
**Blocks:** ADD_TYPESCRIPT_ACTION

## Motivation

The codebase has two parallel type hierarchies: "file" types (`ConfigFile`, `StepFile`, `ActionFile`, `Options`, `StepOptions`, `EffectiveOptions`) and "resolved" types (`Config`, `Step`, `ActionKind`, `Options`). The resolution step (`ConfigFile::resolve()`) transforms one into the other by:

1. Merging global options with per-step overrides into flat `EffectiveOptions`
2. Canonicalizing TypeScript handler paths (after ADD_TYPESCRIPT_ACTION)
3. Wrapping finally hook scripts in `HookScript`
4. Dropping `entrypoint` (consumed before resolution)

After DEMOS_TO_TYPESCRIPT, the TypeScript entry point (`BarnumConfig.fromConfig(...)`) is the primary config path. The JS layer can handle option merging and path resolution before passing config to Rust. The two-tier type system in Rust becomes unnecessary overhead — every struct and enum exists twice for no reason.

## Current state

**File types** (`config.rs`):

| Type | Purpose |
|------|---------|
| `ConfigFile` | Top-level: `options`, `entrypoint`, `steps` (after FLATTEN_AND_RENAME_ACTION deletes `schema_ref`) |
| `Options` | Global defaults: `timeout`, `max_retries`, `max_concurrency`, `retry_on_timeout`, `retry_on_invalid_response` |
| `StepFile` | Step: `name`, `action`, `next`, `finally_hook`, `options` |
| `StepOptions` | Per-step overrides (all fields `Option<T>`) |
| `EffectiveOptions` | Temporary: merges global + step options, all fields concrete |
| `ActionFile` | Enum: `Command(CommandActionFile)` |
| `CommandActionFile` | `script: String` |

**Resolved types** (`resolved.rs`):

| Type | Purpose |
|------|---------|
| `Config` | Top-level: `max_concurrency`, `steps` |
| `Step` | Step: `name`, `action`, `next`, `finally_hook`, `options` |
| `Options` | Flat: `timeout`, `max_retries`, `retry_on_timeout`, `retry_on_invalid_response` |
| `ActionKind` | Enum: `Command(CommandAction)` |
| `CommandAction` | `script: String` |

The resolution method (`ConfigFile::resolve`) merges options, resolves action paths, and wraps hooks. Every field ends up in the resolved type with the same name and roughly the same type — the only real difference is that `Option<T>` overrides become concrete `T` after merging.

## Changes

### 1. Merge the type hierarchies

Replace the two-tier system with a single set of types. The merged types use concrete fields (not `Option<T>` overrides) because option merging happens in JS before the config reaches Rust.

```rust
// config.rs — single set of types

pub struct Config {
    pub options: Options,
    pub steps: Vec<Step>,
}

pub struct Options {
    pub timeout: Option<u64>,
    pub max_retries: u32,
    pub max_concurrency: Option<usize>,
    pub retry_on_timeout: bool,
    pub retry_on_invalid_response: bool,
}

pub struct Step {
    pub name: StepName,
    pub action: Action,
    pub next: Vec<StepName>,
    pub finally_hook: Option<HookScript>,
    pub options: Options,
}

#[serde(tag = "kind")]
pub enum Action {
    Bash(BashAction),
    TypeScript(TypeScriptAction),
}
```

### 2. Move option merging to JS

`BarnumConfig.fromConfig()` currently passes the raw config object to Rust. After this refactor, it resolves per-step option overrides in JS before serializing:

```typescript
// In BarnumConfig.fromConfig() or a preprocessing step
for (const step of config.steps) {
  step.options = {
    timeout: step.options?.timeout ?? config.options?.timeout,
    maxRetries: step.options?.maxRetries ?? config.options?.maxRetries ?? 0,
    maxConcurrency: config.options?.maxConcurrency,
    retryOnTimeout: step.options?.retryOnTimeout ?? config.options?.retryOnTimeout ?? true,
    retryOnInvalidResponse: step.options?.retryOnInvalidResponse ?? config.options?.retryOnInvalidResponse ?? true,
  };
}
```

Rust receives a config where every step has fully-resolved options. No merging logic in Rust.

### 3. Delete resolved.rs

All types live in `config.rs`. The `resolved.rs` file is deleted. All imports of `crate::resolved::*` change to `crate::config::*` (or just `crate::*` depending on re-exports).

### 4. Delete the resolution step

`ConfigFile::resolve()`, `StepFile::resolve()`, `ActionFile::resolve()`, `EffectiveOptions::resolve()` are all deleted. Rust deserializes the config directly into the final types.

### 5. Delete StepOptions and EffectiveOptions

These existed solely to support per-step option overrides with `Option<T>` fields and a merge step. With merging in JS, they're unnecessary.

### 6. Delete ConfigFile

`ConfigFile` held `entrypoint` and the unresolved steps (`schema_ref` is already deleted by FLATTEN_AND_RENAME_ACTION). After this refactor:
- `entrypoint` is handled by JS (it determines the initial tasks before calling `.run()`)
- Steps are already resolved

Rust receives `Config` directly via `--config`.

### 7. Update the runner

The runner currently takes `&Config` (the resolved type). After this refactor, `Config` is the only type, so the runner's interface is unchanged in shape — just the import path changes.

### 8. Update schema generation

`build_schemas` generates JSON Schema and Zod types from the Rust types. After merging, it generates from the unified types. The generated schemas change (no more `ConfigFile` vs `Config` distinction), and the Zod schemas in `libs/barnum/` update accordingly.

### 9. Update tests

Rust tests that construct `ConfigFile` and call `.resolve()` change to construct `Config` directly. Tests that verify resolution behavior (option merging, path canonicalization) are deleted — those concerns move to JS.

## What stays in Rust

- Deserialization of the config JSON (`serde_json::from_str`)
- Validation that step graph references are consistent (all `next` entries point to existing steps)
- The runtime engine (scheduling, concurrency, retries, timeouts)

## What moves to JS

- Option merging (global + per-step overrides)
- Path resolution (TypeScript handler paths)
- `entrypoint` handling
- Config-level validation (`.validate()` method on `BarnumConfig`)

## What this does NOT do

- Does not change the runtime engine or state machine logic
- Does not change the CLI interface (`--config` still accepts JSON)
- Does not remove JSON config support entirely — the CLI can still accept JSON, it just receives pre-resolved JSON from the JS layer
