# Zod Config Schema

**Status:** Phase 1 — Architecture document

## Motivation

Barnum configs are JSON/JSONC files. The `barnum-config-schema.json` provides editor validation in VS Code, but users writing configs programmatically in TypeScript get no compile-time type checking or runtime validation.

Zod schemas provide both. A Zod schema is a runtime JavaScript object that validates data and infers TypeScript types from the schema definition. The Rust config types (with `#[derive(JsonSchema)]`) are the source of truth. schemars introspects those types into an in-memory `RootSchema` tree. Today we have one renderer for that tree (JSON Schema). This adds a second renderer (Zod TypeScript).

## Current State

### Source of truth: Rust types

`crates/barnum_config/src/config.rs` defines `ConfigFile`, `StepFile`, `ActionFile`, `Options`, `StepOptions`, `PreHook`, `PostHook`, `FinallyHook`. Each derives `schemars::JsonSchema`. `ActionFile` has two variants: `Pool` and `Command`.

### Schema introspection

`crates/barnum_config/src/config.rs:545-547`:
```rust
pub fn config_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(ConfigFile)
}
```

This returns a Rust struct (`RootSchema`) containing the full type tree. It is not JSON — it's an in-memory representation of the types, their fields, descriptions, defaults, and relationships.

### Current renderer: JSON

`crates/barnum_config/src/bin/build_barnum_schema.rs` calls `config_schema()`, serializes the `RootSchema` to JSON via `serde_json::to_string_pretty`, and writes `libs/barnum/barnum-config-schema.json`.

### CI verification

`.github/workflows/ci.yml:437-444` regenerates the JSON schema and diffs it against the checked-in version. If they differ, CI fails.

### Pre-commit hook

`.githooks/pre-commit:20-21` regenerates the JSON schema and re-stages it.

### npm package

`libs/barnum/package.json` ships `barnum-config-schema.json`. No TypeScript types are exported.

## Proposed Changes

### Task 1: Implement Zod renderer

**Goal:** A `zod` module in `barnum_config` that walks a schemars `RootSchema` and emits a TypeScript file containing Zod schemas.

**File:** `crates/barnum_config/src/zod.rs` (new)

```rust
use schemars::schema::RootSchema;

/// Render a schemars RootSchema as a TypeScript file containing Zod schemas.
pub fn emit_zod(root: &RootSchema) -> String { ... }
```

This function takes a Rust struct, not a JSON string. It walks the schemars type tree and prints Zod syntax.

#### Rendering algorithm

The `RootSchema` has:
- `definitions`: a map of type names to schemas (e.g., `ActionFile`, `Options`, `StepFile`)
- `schema`: the root schema object (`ConfigFile`)

The renderer:
1. Topologically sorts definitions (a definition referencing another comes after it)
2. Emits each definition as a `const` binding
3. Emits the root schema as an exported `const`
4. Emits inferred TypeScript types via `z.infer`
5. Emits a `defineConfig` helper

#### Mapping rules

| schemars pattern | Zod output |
|---|---|
| `InstanceType::String` | `z.string()` |
| `InstanceType::Integer` | `z.number().int()` |
| `InstanceType::Number` | `z.number()` |
| `InstanceType::Boolean` | `z.boolean()` |
| `InstanceType::Null` | `z.null()` |
| `SingleOrVec::Vec([String, Null])` | `z.string().nullable()` |
| `array.items = Some(X)` | `z.array(X)` |
| `object.properties = {...}` | `z.object({...})` |
| `reference = "#/definitions/Foo"` | `Foo` (variable reference) |
| `subschemas.one_of` with `kind` discriminator | `z.discriminatedUnion("kind", [...])` |
| `subschemas.any_of = [ref, null]` | `Ref.nullable()` |
| `subschemas.all_of = [ref]` | Unwrap to the referenced schema (schemars wraps `$ref` in `allOf` to attach metadata) |
| `enum_values = ["Pool"]` | `z.literal("Pool")` |
| `object.additional_properties = false` | `.strict()` |
| `metadata.default = value` | `.default(value)` |
| `metadata.description = "..."` | `.describe("...")` |
| Property not in `required` | `.optional()` |
| `number.minimum = 0` on integer | `.int().nonnegative()` |

#### Handling schemars idioms

schemars wraps `$ref` in `allOf` when attaching metadata to a reference:
```
SchemaObject {
    metadata: Some(Metadata { description: "Global runtime options.", default: Some({...}) }),
    subschemas: Some(SubschemaValidation { all_of: Some([Schema::Object(SchemaObject { reference: Some("#/definitions/Options") })]) }),
}
```
The renderer unwraps this to `Options.default({...}).describe("Global runtime options.")`.

Nullable optional fields use `anyOf` with a null branch:
```
SubschemaValidation { any_of: Some([ref_to_FinallyHook, Schema { instance_type: Null }]) }
```
The renderer detects the null branch and emits `FinallyHook.nullable()`.

Tagged enums (`#[serde(tag = "kind")]`) produce `oneOf` where each variant has a `kind` property with a single-value `enum`. The renderer detects the common discriminator and emits `z.discriminatedUnion("kind", [...])`.

Untagged enums (`#[serde(untagged)]`) produce `anyOf` without a discriminator. The renderer falls back to `z.union([...])`.

#### Generated output

For the current config types:

```typescript
import { z } from "zod";

const MaybeLinked_for_String = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Inline"),
    value: z.string(),
  }),
  z.object({
    kind: z.literal("Link"),
    path: z.string(),
  }),
]);

const PreHook = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Command"),
    script: z.string(),
  }),
]);

const PostHook = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Command"),
    script: z.string(),
  }),
]);

const FinallyHook = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Command"),
    script: z.string(),
  }),
]);

const SchemaRef = z.union([
  z.object({ link: z.string() }),
  z.any(),
]);

const ActionFile = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Pool"),
    instructions: MaybeLinked_for_String,
  }),
  z.object({
    kind: z.literal("Command"),
    script: z.string(),
  }),
]);

const Options = z.object({
  timeout: z.number().int().nonnegative().nullable().optional().default(null),
  max_retries: z.number().int().nonnegative().optional().default(0),
  max_concurrency: z.number().int().nonnegative().nullable().optional().default(null),
  retry_on_timeout: z.boolean().optional().default(true),
  retry_on_invalid_response: z.boolean().optional().default(true),
}).strict();

const StepOptions = z.object({
  timeout: z.number().int().nonnegative().nullable().optional().default(null),
  max_retries: z.number().int().nonnegative().nullable().optional().default(null),
  retry_on_timeout: z.boolean().nullable().optional().default(null),
  retry_on_invalid_response: z.boolean().nullable().optional().default(null),
}).strict();

const StepFile = z.object({
  name: z.string(),
  value_schema: SchemaRef.nullable().optional().default(null),
  pre: PreHook.nullable().optional().default(null),
  action: ActionFile,
  post: PostHook.nullable().optional().default(null),
  next: z.array(z.string()).optional().default([]),
  finally: FinallyHook.nullable().optional().default(null),
  options: StepOptions.optional().default({}),
}).strict();

export const configFileSchema = z.object({
  "$schema": z.string().optional().describe("JSON Schema URL for editor validation of JSONC configs. Not used with Zod."),
  options: Options.optional().default({}),
  entrypoint: z.string().nullable().optional().default(null),
  steps: z.array(StepFile),
}).strict();

export type ConfigFile = z.infer<typeof configFileSchema>;
export type StepFile = z.infer<typeof StepFile>;
export type ActionFile = z.infer<typeof ActionFile>;
export type Options = z.infer<typeof Options>;

export function defineConfig(config: z.input<typeof configFileSchema>): ConfigFile {
  return configFileSchema.parse(config);
}
```

Descriptions are omitted from this example for readability. The renderer includes `.describe("...")` calls using the doc comments from the Rust types.

#### Topological sort

Definitions reference each other. The dependency graph for the current types:

```
MaybeLinked_for_String  (no deps)
PreHook                 (no deps)
PostHook                (no deps)
FinallyHook             (no deps)
SchemaRef               (no deps)
Options                 (no deps)
StepOptions             (no deps)
ActionFile              → MaybeLinked_for_String
StepFile                → SchemaRef, PreHook, ActionFile, PostHook, FinallyHook, StepOptions
ConfigFile (root)       → Options, StepFile
```

#### Generator structure

```rust
// zod.rs

use schemars::schema::{
    InstanceType, RootSchema, Schema, SchemaObject, SingleOrVec,
};
use std::fmt::Write;

pub fn emit_zod(root: &RootSchema) -> String {
    let mut out = String::new();
    writeln!(out, "import {{ z }} from \"zod\";\n").unwrap();

    let ordered = topological_sort(&root.definitions);
    for name in &ordered {
        let schema = &root.definitions[name.as_str()];
        write!(out, "const {name} = ").unwrap();
        emit_schema(&mut out, schema, &root.definitions);
        writeln!(out, ";\n").unwrap();
    }

    write!(out, "export const configFileSchema = ").unwrap();
    emit_schema_object(&mut out, &root.schema, &root.definitions);
    writeln!(out, ";\n").unwrap();

    // Type exports and defineConfig
    // ...

    out
}
```

The full implementation is ~250 lines handling each schemars pattern from the mapping table. Each `emit_*` function is recursive: objects emit their properties by calling `emit_schema` on each property's schema.

### Task 2: Update binary to generate both files

**Goal:** `build_barnum_schema` generates both `barnum-config-schema.json` and `barnum-config-schema.zod.ts`. No flags.

**File:** `crates/barnum_config/src/bin/build_barnum_schema.rs`

```rust
use barnum_config::{config_schema, zod::emit_zod};

fn main() {
    let root = config_schema();
    let libs = workspace_root().join("libs/barnum");

    let json = serde_json::to_string_pretty(&root).unwrap_or_else(|e| {
        eprintln!("Failed to serialize JSON schema: {e}");
        std::process::exit(1);
    });
    write_file(&libs.join("barnum-config-schema.json"), &json);

    let zod = emit_zod(&root);
    write_file(&libs.join("barnum-config-schema.zod.ts"), &zod);
}
```

### Task 3: Update `barnum config schema` to default to Zod

**Goal:** The `barnum config schema` CLI command outputs the Zod schema by default. `--type json` switches to JSON Schema.

**File:** `crates/barnum_cli/src/main.rs`

Before (`main.rs:112-137`):
```rust
#[derive(Subcommand)]
enum ConfigCommand {
    // ...
    /// Print the JSON schema for config files
    Schema,
}
```

After:
```rust
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum SchemaType {
    #[default]
    Zod,
    Json,
}

#[derive(Subcommand)]
enum ConfigCommand {
    // ...
    /// Print the config schema (Zod by default, JSON with --type json)
    Schema {
        /// Output format: zod (default) or json
        #[arg(long, default_value = "zod")]
        r#type: SchemaType,
    },
}
```

Before (`main.rs:251-256`):
```rust
ConfigCommand::Schema => {
    let schema = config_schema();
    let json = serde_json::to_string_pretty(&schema)
        .map_err(|e| io::Error::other(format!("[E059] failed to serialize schema: {e}")))?;
    println!("{json}");
}
```

After:
```rust
ConfigCommand::Schema { r#type } => {
    let root = config_schema();
    match r#type {
        SchemaType::Zod => {
            let zod = barnum_config::zod::emit_zod(&root);
            print!("{zod}");
        }
        SchemaType::Json => {
            let json = serde_json::to_string_pretty(&root)
                .map_err(|e| io::Error::other(format!("[E059] failed to serialize schema: {e}")))?;
            println!("{json}");
        }
    }
}
```

### Task 4: Register the `zod` module

**File:** `crates/barnum_config/src/lib.rs`

Add `pub mod zod;`.

### Task 5: Update CI to verify both files

**File:** `.github/workflows/ci.yml`

Before:
```yaml
- name: Generate Barnum config schema
  run: cargo run -p barnum_config --bin build_barnum_schema
- name: Check schema is up to date
  run: |
    if ! git diff --exit-code libs/barnum/barnum-config-schema.json; then
      echo "::error::Generated schema differs from checked-in version."
      exit 1
    fi
```

After:
```yaml
- name: Generate Barnum config schemas
  run: cargo run -p barnum_config --bin build_barnum_schema
- name: Check schemas are up to date
  run: |
    if ! git diff --exit-code libs/barnum/barnum-config-schema.json libs/barnum/barnum-config-schema.zod.ts; then
      echo "::error::Generated schemas differ from checked-in versions. Run 'cargo run -p barnum_config --bin build_barnum_schema' and commit."
      exit 1
    fi
```

### Task 6: Update pre-commit hook

**File:** `.githooks/pre-commit`

Before:
```bash
echo "Regenerating config schema..."
cargo run -p barnum_config --bin build_barnum_schema 2>/dev/null
```

After:
```bash
echo "Regenerating config schemas..."
cargo run -p barnum_config --bin build_barnum_schema 2>/dev/null
```

The re-stage step also needs to include the new file:

```bash
echo "$STAGED_FILES" | xargs git add
git add libs/barnum/barnum-config-schema.zod.ts
```

### Task 7: Update npm package

**File:** `libs/barnum/package.json`

Add `barnum-config-schema.zod.ts` to `files`, add `types` pointing to it, and add `zod` as an optional peer dependency:

```json
{
  "types": "barnum-config-schema.zod.ts",
  "files": [
    "index.js",
    "cli.js",
    "artifacts/**/*",
    "barnum-config-schema.json",
    "barnum-config-schema.zod.ts"
  ],
  "peerDependencies": {
    "zod": "^3.0.0"
  },
  "peerDependenciesMeta": {
    "zod": {
      "optional": true
    }
  }
}
```

With `"types"` set, `import { ConfigFile } from "@barnum/barnum"` resolves the Zod schema's type exports directly.

### Task 8: Update docs-website

**Goal:** The schema reference page should indicate that Barnum provides a Zod schema as the primary TypeScript API, with JSON Schema available for JSONC editor validation.

**File:** `docs-website/docs/reference/` (specific file TBD based on current docs structure)

The docs should cover:
- Import path: `import { defineConfig, configFileSchema } from "@barnum/barnum/barnum-config-schema.zod"`
- `defineConfig` validates at runtime and returns a typed config
- `configFileSchema` is the raw Zod schema for custom composition
- JSON Schema (`barnum-config-schema.json`) still exists for `$schema` references in JSONC configs

### Task 9: Update `CLAUDE.md` generated artifacts list

**File:** `.claude/CLAUDE.md`

The generated artifacts section currently lists only the JSON schema. Add the Zod schema:

```markdown
Current generated artifacts:
- `libs/barnum/barnum-config-schema.json` — regenerate with `cargo run -p barnum_config --bin build_barnum_schema`
- `libs/barnum/barnum-config-schema.zod.ts` — regenerate with `cargo run -p barnum_config --bin build_barnum_schema`
```

## What breaks

Nothing. This is additive. The JSON schema continues to work for `$schema` references in JSONC configs. The Zod schema is a new file generated alongside it.
