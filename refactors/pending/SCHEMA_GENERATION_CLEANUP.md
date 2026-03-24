# Schema Generation Cleanup

## Motivation

The codebase generates TypeScript types from Rust types via schemars (type reflection) and a custom Zod renderer, but the current system has gaps:

1. **Missing resolved type output.** Config-file types (`ConfigFile`, `StepFile`, `ActionFile`) have Zod + TypeScript output. The resolved types (`Config`, `Step`, `ActionKind`, `PoolAction`, `CommandAction`, `Options`) have nothing. These are the runtime types after file references are resolved and options are merged. TypeScript code that interacts with resolved configs (e.g. state log entries, debugging tools) has no generated types.

2. **Redundant generator binary.** `build_cli_schema.rs` generates only `barnum-cli-schema.zod.ts`, but `build_schemas.rs` already generates that same file along with the other two. `build_cli_schema.rs` is dead code.

3. **No single inventory of what gets generated.** The `build_schemas.rs` binary is the canonical source, but the existence of `build_cli_schema.rs` muddies this.

## Current state

### How Rust types become TypeScript types

The pipeline: Rust types with `#[derive(schemars::JsonSchema)]` produce a schemars `RootSchema` via `schema_for!()`. The `RootSchema` is an intermediate representation — a tree of type metadata. The custom `emit_zod()` renderer in `zod.rs` walks this tree and emits a TypeScript file with Zod validators and inferred types. The schemars derive is purely a reflection mechanism; the output format is Zod, not JSON Schema (except for the one config-file JSON Schema kept for editor `$schema` support).

### Generator binary

`crates/barnum_cli/src/bin/build_schemas.rs` (59 lines) generates all three artifacts:

| Artifact | Source | Renderer |
|---|---|---|
| `libs/barnum/barnum-config-schema.json` | `config_schema()` → `serde_json::to_string_pretty` | JSON Schema (for editor `$schema` validation only) |
| `libs/barnum/barnum-config-schema.zod.ts` | `config_schema()` → `emit_zod` + `defineConfig()` append | Zod renderer |
| `libs/barnum/barnum-cli-schema.zod.ts` | `schemars::schema_for!(Cli)` → `emit_zod` | Zod renderer |

`crates/barnum_cli/src/bin/build_cli_schema.rs` (38 lines) generates only `barnum-cli-schema.zod.ts`. It duplicates `build_schemas.rs` lines 47-49. Nothing invokes it.

### Resolved types without schemars derives

`crates/barnum_config/src/resolved.rs` defines the runtime types. None have schemars derives, so `emit_zod` can't introspect them:

```rust
// resolved.rs — current derives (no schemars)
#[derive(Debug, Serialize, Deserialize)]     // Config
#[derive(Debug, Serialize, Deserialize)]     // Step
#[derive(Debug, Serialize, Deserialize)]     // PoolAction
#[derive(Debug, Serialize, Deserialize)]     // CommandAction
#[derive(Debug, Serialize, Deserialize)]     // ActionKind
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]  // Options
```

### Zod renderer

`crates/barnum_config/src/zod.rs` (625 lines) converts any schemars `RootSchema` into a TypeScript file with Zod validators. It handles discriminated unions, nullable types, defaults, descriptions, and topological ordering of definitions. Adding a new root type requires calling `schemars::schema_for!(Type)` and passing the result to `emit_zod`.

### Pre-commit hook and CI

The pre-commit hook (`/.githooks/pre-commit:20-27`) runs `cargo run -p barnum_cli --bin build_schemas` and re-stages the three generated files. CI (`/.github/workflows/ci.yml:459-500`) regenerates and diffs to verify they're in sync. Both reference only `build_schemas`, not `build_cli_schema`.

## Proposed changes

### 1. Add schemars derives to resolved types

Add `#[derive(schemars::JsonSchema)]` to all types in `resolved.rs` so the Zod renderer can introspect them:
- `Config`
- `Step`
- `PoolAction`
- `CommandAction`
- `ActionKind`
- `Options`

The `schemars` dependency already exists in `barnum_config`'s `Cargo.toml`.

One complication: `Step.finally_hook` is `Option<HookScript>`, and `HookScript` is defined in `barnum_types` via the `define_string_id!` macro. `HookScript` doesn't have a schemars derive. Same for `StepName` and `StepInputValue`. These types need schemars derives added, or the resolved types need `#[schemars(with = "String")]` annotations on fields that use them.

The cleanest path: add `schemars` as an optional dependency on `barnum_types` behind a feature flag, and add the derive on `StepName`, `HookScript`, `StepInputValue`, and `LogTaskId`. The `define_string_id!` macro (from the `string_id` crate) would need to support this — check whether it already does or whether we need a manual impl. If the macro doesn't support it, write manual impls (they should all schema as `{ "type": "string" }`).

Alternatively, since these are all newtypes over `String` or `serde_json::Value`, we can use `#[schemars(with = "String")]` on fields in `resolved.rs` to avoid touching `barnum_types` at all. This is simpler but less accurate (loses the newtype semantics in the generated types).

### 2. Export a `resolved_schema()` function

In `crates/barnum_config/src/resolved.rs` or `lib.rs`, add:

```rust
pub fn resolved_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(Config)
}
```

Export it from `lib.rs` alongside the existing `config_schema()`. This is the schemars intermediate representation that feeds into `emit_zod`.

### 3. Generate resolved Zod schema

Add one new generated file to `build_schemas.rs`:

| New artifact | Source | Renderer |
|---|---|---|
| `libs/barnum/barnum-resolved-schema.zod.ts` | `resolved_schema()` → `emit_zod` | Zod renderer |

Update the pre-commit hook to re-stage the new file. Update CI to include it in the diff check.

### 4. Delete `build_cli_schema.rs`

Remove `crates/barnum_cli/src/bin/build_cli_schema.rs`. Nothing references it.

### 5. Add `Task` to generated output

The `Task` type in `types.rs` is the agent response format — agents return `[{"kind": "StepName", "value": {...}}]`. This type doesn't have a schemars derive either. It should, and its Zod output should be included in the resolved schema file (since `Task` is the runtime type agents produce, not a config-file type).

`Task` uses `StepName` and `StepInputValue` from `barnum_types`, so the same schemars question from section 1 applies here.

## Open questions

1. **`barnum_types` and schemars**: Should we add `schemars` as a dependency to `barnum_types` and add derives on all its types? Or use `#[schemars(with = ...)]` annotations in `resolved.rs` to avoid the dependency? The former is cleaner long-term; the latter is a smaller change.

2. **Naming**: The resolved Zod file will export `configSchema` (from `Config` type). The config-file Zod already exports `configFileSchema` (from `ConfigFile`). These names are distinct, but should we rename the resolved root type to something more explicit like `ResolvedConfig` to avoid confusion?

3. **Should `Task` go in the resolved schema or its own file?** `Task` isn't part of `Config`, so `schema_for!(Config)` won't include it. Options: (a) create a wrapper struct purely for schema generation that includes both, (b) generate `Task` Zod separately, (c) include `Task` as a definition in the resolved schema via a custom schema root.

## Tasks

### Task 1: Add schemars derives to `barnum_types` types

Add `schemars` dependency to `barnum_types/Cargo.toml`. Add schemars derives on `StepName`, `HookScript`, `StepInputValue`, and `LogTaskId`. For the `define_string_id!` types, add manual impls if the macro doesn't support it (they should all produce `{ "type": "string" }`).

### Task 2: Add schemars derives to resolved types

Add schemars derives to `Config`, `Step`, `PoolAction`, `CommandAction`, `ActionKind`, `Options` in `resolved.rs`. Add `resolved_schema()` function. Export from `lib.rs`.

### Task 3: Add schemars derive to `Task`

Add schemars derive to `Task` in `types.rs`. Decide how to include it in generated Zod output.

### Task 4: Generate resolved Zod schema

Update `build_schemas.rs` to generate `barnum-resolved-schema.zod.ts`. Update pre-commit hook and CI.

### Task 5: Delete `build_cli_schema.rs`

Remove the redundant binary.
