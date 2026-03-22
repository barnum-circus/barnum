# TypeScript Config API

**Status:** Phase 1 — Architecture document

## Motivation

Barnum configs are JSON/JSONC files. The JSON Schema provides editor validation in VS Code, but users writing configs programmatically (generating steps in a loop, sharing step definitions across workflows) have no compile-time type checking. Adding TypeScript type definitions to the `@barnum/barnum` npm package lets users write `import type { ConfigFile } from "@barnum/barnum"` and get editor completion, type errors, and autocomplete on every field.

## Current State

### Config types in Rust

`crates/barnum_config/src/config.rs` defines the config types. Each struct derives `schemars::JsonSchema` for JSON Schema generation and `serde::Serialize`/`Deserialize` for JSON parsing:

```rust
// crates/barnum_config/src/config.rs (approximate)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConfigFile {
    #[serde(rename = "$schema", default, skip_serializing)]
    pub schema_url: Option<String>,
    pub entrypoint: Option<StepName>,
    #[serde(default)]
    pub options: Options,
    pub steps: Vec<StepFile>,
}
```

Other config types in the same file: `StepFile`, `ActionFile` (enum: Pool | Command), `Options`, `StepOptions`, `PreHook`, `PostHook`, `FinallyHook`, `MaybeLinked<T>`, `SchemaRef`.

### JSON Schema generation

`crates/barnum_config/src/bin/build_barnum_schema.rs` generates `libs/barnum/barnum-config-schema.json` via schemars. This is a 408-line JSON Schema (draft-07). The file is checked in and verified by CI.

### npm package structure

`libs/barnum/package.json`:
```json
{
  "name": "@barnum/barnum",
  "version": "0.2.4",
  "main": "index.js",
  "bin": { "barnum": "cli.js" },
  "files": [
    "index.js",
    "cli.js",
    "artifacts/**/*",
    "barnum-config-schema.json"
  ]
}
```

`libs/barnum/index.js` resolves the platform-specific binary path. `libs/barnum/cli.js` spawns the binary. No `.d.ts` files exist.

### CI verification

`.github/workflows/ci.yml` regenerates the JSON Schema and diffs it against the checked-in version. The pre-commit hook in `hooks/pre-commit` regenerates it automatically.

## Proposed Changes

### Task 1: Add `ts-rs` to Rust config types

**Goal:** Generate TypeScript type definitions directly from Rust structs using `ts-rs`, avoiding the JSON Schema intermediate step entirely.

#### 1.1: Add `ts-rs` dependency

**File:** `crates/barnum_config/Cargo.toml`

```toml
# Before
[dependencies]
serde = { version = "1", features = ["derive"] }
schemars = { version = "0.8", features = ["preserve_order"] }

# After
[dependencies]
serde = { version = "1", features = ["derive"] }
schemars = { version = "0.8", features = ["preserve_order"] }
ts-rs = { version = "10", features = ["serde-compat"] }
```

`serde-compat` is required because the config types use serde attributes (`#[serde(tag = "kind")]`, `#[serde(rename)]`, `#[serde(default)]`) that affect the serialized shape.

#### 1.2: Derive `TS` on config types

**File:** `crates/barnum_config/src/config.rs`

Add `#[derive(TS)]` and `#[ts(export)]` to each config type that should appear in the TypeScript definitions.

Before:
```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConfigFile {
    #[serde(rename = "$schema", default, skip_serializing)]
    pub schema_url: Option<String>,
    pub entrypoint: Option<StepName>,
    #[serde(default)]
    pub options: Options,
    pub steps: Vec<StepFile>,
}
```

After:
```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct ConfigFile {
    #[serde(rename = "$schema", default, skip_serializing)]
    pub schema_url: Option<String>,
    pub entrypoint: Option<StepName>,
    #[serde(default)]
    pub options: Options,
    pub steps: Vec<StepFile>,
}
```

Same change for: `StepFile`, `ActionFile`, `Options`, `StepOptions`, `PreHook`, `PostHook`, `FinallyHook`, `MaybeLinked<T>`, `SchemaRef`, `StepName`, `StepInputValue`.

**Complication: `$schema` field.** The `$schema` field uses `#[serde(rename = "$schema")]`. `ts-rs` should respect this and generate `"$schema"?: string | null` in the TypeScript output. Verify this.

**Complication: `MaybeLinked<T>`.** This is a generic enum with `#[serde(tag = "kind")]`. `ts-rs` handles generics:
```rust
#[derive(Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum MaybeLinked<T> {
    Inline { value: T },
    Link { path: String },
}
```
Generates: `type MaybeLinked<T> = { kind: "Inline"; value: T } | { kind: "Link"; path: string }` (discriminated union). Verify the output matches the JSON schema's `oneOf`.

**Complication: Enum tagging.** `ActionFile` uses `#[serde(tag = "kind")]`:
```rust
#[derive(Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum ActionFile {
    Pool { instructions: MaybeLinked<String> },
    Command { script: String },
}
```
With `serde-compat`, `ts-rs` should generate a discriminated union:
```typescript
type ActionFile =
  | { kind: "Pool"; instructions: MaybeLinked<string> }
  | { kind: "Command"; script: string };
```

#### 1.3: Mapping reference

| Rust Type | Expected TypeScript | Notes |
|-----------|-------------------|-------|
| `String` | `string` | |
| `Option<T>` | `T \| null` | serde serializes `None` as `null` |
| `Vec<T>` | `Array<T>` | |
| `u32`, `u64` | `number` | precision loss for u64 is acceptable (config values are small) |
| `bool` | `boolean` | |
| `StepName` (newtype around `String`) | `string` | need `#[ts(as = "String")]` or implement `TS` manually |
| `StepInputValue` (newtype around `serde_json::Value`) | `unknown` or `any` | JSON payload, no static type |
| `serde_json::Value` | `unknown` | for `value_schema` and similar |

### Task 2: Create TypeScript generation binary

**Goal:** A Rust binary that writes the generated TypeScript to `libs/barnum/types.d.ts`, matching the pattern of `build_barnum_schema`.

**File:** `crates/barnum_config/src/bin/build_barnum_types.rs` (new)

```rust
//! Build-time TypeScript type generator for Barnum config.
//!
//! Generates TypeScript definitions and writes them to libs/barnum/types.d.ts
//! for inclusion in the npm package.
//!
//! Run with: `cargo run -p barnum_config --bin build_barnum_types`

#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use barnum_config::ConfigFile;
use std::fs;
use std::path::Path;
use ts_rs::TS;

fn main() {
    let ts = ConfigFile::export_to_string()
        .unwrap_or_else(|e| {
            eprintln!("Failed to generate TypeScript: {e}");
            std::process::exit(1);
        });

    // Prepend banner and append defineConfig
    let output = format!(
        "// Generated from Rust types — do not edit manually.\n\n\
         {ts}\n\n\
         /**\n * Identity function that provides type inference for config objects.\n \
         * Returns its argument unchanged.\n */\n\
         export declare function defineConfig(config: ConfigFile): ConfigFile;\n"
    );

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().and_then(|p| p.parent())
        .expect("Cannot find workspace root");
    let output_path = workspace_root.join("libs/barnum/types.d.ts");

    fs::write(&output_path, &output).unwrap_or_else(|e| {
        eprintln!("Failed to write types file: {e}");
        std::process::exit(1);
    });

    println!("Types written to: {}", output_path.display());
}
```

**Complication:** `ts-rs` generates types per-struct via `T::export_to_string()`. For a single file with all types, we may need to call `export_to_string()` on each type and concatenate, or use `ts-rs`'s `export_all_to!` macro. The exact API depends on the `ts-rs` version. With `#[ts(export)]`, running `cargo test` writes individual `.ts` files to `bindings/`. The binary approach above would need to collect all types into one file.

Alternative: Use `ts-rs`'s test-based generation (`cargo test` writes to `bindings/`), then have the binary copy/concatenate those files into `libs/barnum/types.d.ts`. Less elegant but reliable.

**Recommended approach:** Use `ConfigFile::export_to_string_with_all_dependencies()` (if available in ts-rs v10) to get the full type tree from the root `ConfigFile` type. This recursively includes all referenced types (`StepFile`, `ActionFile`, `Options`, etc.) in a single string.

#### 2.1: Register binary in Cargo.toml

**File:** `crates/barnum_config/Cargo.toml`

```toml
[[bin]]
name = "build_barnum_types"
path = "src/bin/build_barnum_types.rs"
```

Alongside the existing:
```toml
[[bin]]
name = "build_barnum_schema"
path = "src/bin/build_barnum_schema.rs"
```

### Task 3: Create `defineConfig.js`

**Goal:** Runtime identity function so `import { defineConfig } from "@barnum/barnum"` works.

**File:** `libs/barnum/defineConfig.js` (new)

```javascript
'use strict';
module.exports.defineConfig = function defineConfig(config) {
  return config;
};
```

This function does nothing at runtime. It exists so TypeScript can infer the type from the call site without requiring an explicit type annotation.

### Task 4: Create `index.d.ts`

**Goal:** TypeScript entry point that re-exports generated types and the defineConfig helper.

**File:** `libs/barnum/index.d.ts` (new)

```typescript
export type {
  ConfigFile,
  StepFile,
  ActionFile,
  Options,
  StepOptions,
  PreHook,
  PostHook,
  FinallyHook,
} from './types';

export { defineConfig } from './types';
```

The exact list of re-exported types depends on what `ts-rs` generates. If newtypes like `StepName` export as `string` (via `#[ts(as = "String")]`), they won't appear here.

### Task 5: Update `package.json`

**File:** `libs/barnum/package.json`

Before:
```json
{
  "name": "@barnum/barnum",
  "version": "0.2.4",
  "main": "index.js",
  "bin": { "barnum": "cli.js" },
  "files": [
    "index.js",
    "cli.js",
    "artifacts/**/*",
    "barnum-config-schema.json"
  ]
}
```

After:
```json
{
  "name": "@barnum/barnum",
  "version": "0.2.4",
  "main": "index.js",
  "types": "index.d.ts",
  "bin": { "barnum": "cli.js" },
  "files": [
    "index.js",
    "index.d.ts",
    "types.d.ts",
    "defineConfig.js",
    "cli.js",
    "artifacts/**/*",
    "barnum-config-schema.json"
  ]
}
```

Changes:
- Added `"types": "index.d.ts"`.
- Added `index.d.ts`, `types.d.ts`, `defineConfig.js` to `files`.

### Task 6: Update `index.js` exports

**File:** `libs/barnum/index.js`

Before (`libs/barnum/index.js:1-23`):
```javascript
'use strict';
const path = require('path');
let binary;
// ... platform detection ...
module.exports = binary;
```

After:
```javascript
'use strict';
const path = require('path');
let binary;
// ... platform detection (unchanged) ...

module.exports = {
  binary,
  defineConfig: require('./defineConfig').defineConfig,
};
```

**File:** `libs/barnum/cli.js`

Before (`libs/barnum/cli.js:4`):
```javascript
var bin = require('.');
```

After:
```javascript
var bin = require('.').binary;
```

### Task 7: Update CI to verify `types.d.ts`

**File:** `.github/workflows/ci.yml`

Add a step after the existing schema verification:

```yaml
- name: Verify generated TypeScript types
  run: |
    cargo run -p barnum_config --bin build_barnum_types
    git diff --exit-code libs/barnum/types.d.ts
```

No npm dependencies needed — the generation is pure Rust.

### Task 8: Update pre-commit hook

**File:** `hooks/pre-commit`

Add after the existing schema regeneration:

```bash
cargo run -p barnum_config --bin build_barnum_types
git add libs/barnum/types.d.ts
```

## User Workflow (after all tasks)

Install types for editor support:
```bash
pnpm add -D @barnum/barnum
```

Write a typed config:
```typescript
// workflow.ts
import { defineConfig } from "@barnum/barnum";

export default defineConfig({
  entrypoint: "Analyze",
  options: { max_retries: 3, max_concurrency: 5 },
  steps: [
    {
      name: "Analyze",
      action: { kind: "Pool", instructions: { kind: "Inline", value: "Analyze the code." } },
      next: ["Implement"],
    },
    {
      name: "Implement",
      action: { kind: "Pool", instructions: { kind: "Link", path: "implement.md" } },
      next: [],
    },
  ],
});
```

## Open Questions

1. **`ts-rs` vs Specta — extensibility is the deciding factor.** When pluggable action kinds land (`PLUGGABLE_ACTION_KINDS.md`), `ActionFile` becomes an open discriminated union: users register custom executor types that need to appear in the generated TypeScript. The static `types.d.ts` in the npm package will only contain the built-in action kinds (Pool, Command). Per-project type generation must include user-defined action kinds in the union. Specta's type registry approach (collecting types into a central registry at generation time) is better suited for this than `ts-rs`'s per-struct `#[derive(TS)]`, because user-defined executor types live outside the barnum crate and can't have barnum's derive macros added to them. With Specta, a generation binary can accept a list of user-registered types and include them in the output. This is the primary constraint on the tool choice — whichever handles extensible type collections wins.

2. **`ts-rs` version and API.** The exact API for exporting all dependent types into a single string varies between ts-rs versions. v7 uses `TS::export_to_string()` per type; v10 may have `export_to_string_with_all_dependencies()`. Need to verify which version provides the cleanest single-file output.

2. **Newtype handling.** `StepName` is a newtype around `String`. Options:
   - `#[ts(as = "String")]` — generates `string`, losing the nominal type.
   - `type StepName = string` — preserves the name but it's just an alias. TypeScript has no runtime distinction.
   - Recommend `#[ts(as = "String")]` for simplicity since `StepName` adds no TypeScript-visible semantics.

3. **`serde_json::Value` mapping.** Fields like `value_schema` accept arbitrary JSON. `ts-rs` maps `serde_json::Value` to `unknown` by default (or `any` depending on config). `unknown` is safer. Verify the default and adjust if needed.

4. **`$schema` field.** The `ConfigFile` struct has a `$schema` field renamed via `#[serde(rename = "$schema")]`. The generated TypeScript needs `"$schema"?: string | null`. Verify `ts-rs` handles the dollar-sign rename correctly — it may need `#[ts(rename = "$schema")]` explicitly.
