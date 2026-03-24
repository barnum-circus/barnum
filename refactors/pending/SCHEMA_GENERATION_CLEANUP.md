# Schema Generation Cleanup

## Motivation

The codebase generates TypeScript types and JSON schemas from Rust types, but the current system has gaps:

1. **Missing resolved type schemas.** Config-file types (`ConfigFile`, `StepFile`, `ActionFile`) have JSON Schema + Zod output. The resolved types (`Config`, `Step`, `ActionKind`, `PoolAction`, `CommandAction`, `Options`) have nothing. These are the runtime types after file references are resolved and options are merged. TypeScript code that interacts with resolved configs (e.g. state log entries, debugging tools) has no generated types.

2. **Redundant generator binary.** `build_cli_schema.rs` generates only `barnum-cli-schema.zod.ts`, but `build_schemas.rs` already generates that same file along with the other two. `build_cli_schema.rs` is dead code.

3. **No single inventory of what gets generated.** The `build_schemas.rs` binary is the canonical source, but the existence of `build_cli_schema.rs` muddies this.

## Current state

### Generator binary

`crates/barnum_cli/src/bin/build_schemas.rs` (59 lines) generates all three artifacts:

| Artifact | Source | Renderer |
|---|---|---|
| `libs/barnum/barnum-config-schema.json` | `config_schema()` → `serde_json::to_string_pretty` | JSON serialization of `RootSchema` |
| `libs/barnum/barnum-config-schema.zod.ts` | `config_schema()` → `emit_zod` + `defineConfig()` append | `zod.rs` Zod renderer |
| `libs/barnum/barnum-cli-schema.zod.ts` | `schemars::schema_for!(Cli)` → `emit_zod` | `zod.rs` Zod renderer |

`crates/barnum_cli/src/bin/build_cli_schema.rs` (38 lines) generates only `barnum-cli-schema.zod.ts`. It duplicates `build_schemas.rs` lines 47-49. Nothing invokes it.

### Resolved types without schemas

`crates/barnum_config/src/resolved.rs` defines the runtime types. None derive `JsonSchema`:

```rust
// resolved.rs — current derives
#[derive(Debug, Serialize, Deserialize)]     // Config
#[derive(Debug, Serialize, Deserialize)]     // Step
#[derive(Debug, Serialize, Deserialize)]     // PoolAction
#[derive(Debug, Serialize, Deserialize)]     // CommandAction
#[derive(Debug, Serialize, Deserialize)]     // ActionKind
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]  // Options
```

### Zod renderer

`crates/barnum_config/src/zod.rs` (625 lines) converts any schemars `RootSchema` into a TypeScript file with Zod schemas. It handles discriminated unions, nullable types, defaults, descriptions, and topological ordering of definitions. Adding a new root type requires calling `schemars::schema_for!(Type)` and passing the result to `emit_zod`.

### Pre-commit hook and CI

The pre-commit hook (`/.githooks/pre-commit:20-27`) runs `cargo run -p barnum_cli --bin build_schemas` and re-stages the three generated files. CI (`/.github/workflows/ci.yml:459-500`) regenerates and diffs to verify they're in sync. Both reference only `build_schemas`, not `build_cli_schema`.

## Proposed changes

### 1. Add `JsonSchema` derives to resolved types

Add `#[derive(JsonSchema)]` to all types in `resolved.rs`:
- `Config`
- `Step`
- `PoolAction`
- `CommandAction`
- `ActionKind`
- `Options`

This requires adding `use schemars::JsonSchema;` to `resolved.rs`. The `schemars` dependency already exists in `barnum_config`'s `Cargo.toml`.

One complication: `Step.finally_hook` is `Option<HookScript>`, and `HookScript` is defined in `barnum_types` via the `define_string_id!` macro. `HookScript` doesn't derive `JsonSchema`. Same for `StepName` and `StepInputValue`. These types need `JsonSchema` derives added, or the resolved types need `#[schemars(with = "String")]` annotations on fields that use them.

The cleanest path: add `schemars` as an optional dependency on `barnum_types` behind a feature flag, and derive `JsonSchema` on `StepName`, `HookScript`, `StepInputValue`, and `LogTaskId`. The `define_string_id!` macro (from the `string_id` crate) would need to support this — check whether it already does or whether we need a manual `impl JsonSchema`. If the macro doesn't support it, use `#[schemars(transparent)]` or manual impls.

Alternatively, since these are all newtypes over `String` or `serde_json::Value`, we can use `#[schemars(with = "String")]` on fields in `resolved.rs` to avoid touching `barnum_types` at all. This is simpler but less accurate (loses the newtype semantics in the schema).

### 2. Export a `resolved_schema()` function

In `crates/barnum_config/src/resolved.rs` or `lib.rs`, add:

```rust
pub fn resolved_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(Config)
}
```

Export it from `lib.rs` alongside the existing `config_schema()`.

### 3. Generate resolved type Zod schema

Add one new generated file to `build_schemas.rs`:

| New artifact | Source | Renderer |
|---|---|---|
| `libs/barnum/barnum-resolved-schema.zod.ts` | `resolved_schema()` → `emit_zod` | Zod renderer |

No JSON Schema file for resolved types — JSON Schema is only needed for editor validation of config files, which doesn't apply here. The Zod schema provides TypeScript types and runtime validation.

Update the pre-commit hook to re-stage the new file. Update CI to include it in the diff check.

### 4. Delete `build_cli_schema.rs`

Remove `crates/barnum_cli/src/bin/build_cli_schema.rs`. Nothing references it.

### 5. Add `Task` schema

The `Task` type in `types.rs` is the agent response format — agents return `[{"kind": "StepName", "value": {...}}]`. This type doesn't derive `JsonSchema` either. It should, and its schema should be included in the resolved Zod output (since `Task` is the runtime type agents produce, not a config-file type).

`Task` uses `StepName` and `StepInputValue` from `barnum_types`, so the same `JsonSchema` question from section 1 applies here.

## Open questions

1. **`barnum_types` and `JsonSchema`**: Should we add `schemars` as a dependency to `barnum_types` and derive `JsonSchema` on all its types? Or use `#[schemars(with = ...)]` annotations in `resolved.rs` to avoid the dependency? The former is cleaner long-term; the latter is a smaller change.

2. **Naming**: The resolved config Zod file exports `configSchema` (from `Config` type). The config-file Zod already exports `configFileSchema` (from `ConfigFile`). These names are distinct, but should we rename the resolved root type to something more explicit like `ResolvedConfig` to avoid confusion?

3. **Should `Task` go in the resolved schema or its own schema?** `Task` isn't part of `Config`, so `schema_for!(Config)` won't include it. Options: (a) create a wrapper struct `ResolvedTypes { config: Config, task: Task }` purely for schema generation, (b) generate `Task` schema separately, (c) include `Task` as a definition in the resolved schema via a custom schema root.

## Tasks

### Task 1: Add `JsonSchema` to `barnum_types` types

Add `schemars` dependency to `barnum_types/Cargo.toml`. Derive `JsonSchema` on `StepName`, `HookScript`, `StepInputValue`, and `LogTaskId`. For the `define_string_id!` types, add manual `impl JsonSchema` if the macro doesn't support it (they should schema as `{ "type": "string" }`).

### Task 2: Add `JsonSchema` derives to resolved types

Add `#[derive(JsonSchema)]` to `Config`, `Step`, `PoolAction`, `CommandAction`, `ActionKind`, `Options` in `resolved.rs`. Add `resolved_schema()` function. Export from `lib.rs`.

### Task 3: Add `JsonSchema` to `Task`

Add `#[derive(JsonSchema)]` to `Task` in `types.rs`. Decide how to include it in generated output.

### Task 4: Generate resolved Zod schema

Update `build_schemas.rs` to generate `barnum-resolved-schema.zod.ts` (Zod only, no JSON Schema). Update pre-commit hook and CI.

### Task 5: Delete `build_cli_schema.rs`

Remove the redundant binary.
