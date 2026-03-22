# Zod Config Schema

**Status:** Phase 1 — Architecture document

## Motivation

Barnum configs are JSON/JSONC files. The `barnum-config-schema.json` provides editor validation in VS Code, but users writing configs programmatically in TypeScript have no compile-time type checking or runtime validation. The existing `TYPESCRIPT_CONFIG.md` proposes generating `.d.ts` files via Specta or ts-rs, which gives compile-time types but no runtime validation.

Zod schemas provide both. A Zod schema is a runtime JavaScript object that validates data AND infers TypeScript types from the schema definition. Generating Zod from the same schemars tree that already produces JSON Schema means:

- One source of truth (Rust types with schemars derives) produces two outputs
- TypeScript users get `z.infer<typeof ConfigFile>` for free
- Runtime validation via `ConfigFile.parse(config)` catches errors before the workflow runs
- No new Rust derive macros needed (Specta/ts-rs become unnecessary for this use case)

This supersedes the approach in `TYPESCRIPT_CONFIG.md`.

## Current State

### Schema generation pipeline

`crates/barnum_config/src/config.rs:545-547`:
```rust
pub fn config_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(ConfigFile)
}
```

`crates/barnum_config/src/bin/build_barnum_schema.rs` calls `config_schema()`, serializes the `RootSchema` to JSON, and writes it to `libs/barnum/barnum-config-schema.json`.

### CI verification

`.github/workflows/ci.yml:437-444`:
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

### Pre-commit hook

`.githooks/pre-commit:20-21`:
```bash
echo "Regenerating config schema..."
cargo run -p barnum_config --bin build_barnum_schema 2>/dev/null
```

### npm package

`libs/barnum/package.json` ships `barnum-config-schema.json` in the `files` array. No TypeScript types are exported.

## Proposed Changes

### Task 1: Add `--type` flag to `build_barnum_schema`

**Goal:** The existing binary accepts `--type json|zod` (defaults to `zod`) and writes the corresponding output file.

**File:** `crates/barnum_config/src/bin/build_barnum_schema.rs`

Before:
```rust
fn main() {
    let schema = config_schema();
    let json = serde_json::to_string_pretty(&schema).unwrap_or_else(|e| { ... });
    let output_path = workspace_root.join("libs/barnum/barnum-config-schema.json");
    fs::write(&output_path, &json).unwrap_or_else(|e| { ... });
}
```

After:
```rust
use barnum_config::{config_schema, zod::schema_to_zod};

enum OutputType { Json, Zod }

fn parse_args() -> OutputType {
    let args: Vec<String> = std::env::args().collect();
    match args.iter().position(|a| a == "--type") {
        Some(i) => match args.get(i + 1).map(String::as_str) {
            Some("json") => OutputType::Json,
            Some("zod") => OutputType::Zod,
            Some(other) => {
                eprintln!("Unknown type: {other}. Expected 'json' or 'zod'.");
                std::process::exit(1);
            }
            None => {
                eprintln!("--type requires a value: 'json' or 'zod'");
                std::process::exit(1);
            }
        },
        None => OutputType::Zod,
    }
}

fn main() {
    let output_type = parse_args();
    let schema = config_schema();

    let (content, filename) = match output_type {
        OutputType::Json => {
            let json = serde_json::to_string_pretty(&schema).unwrap_or_else(|e| {
                eprintln!("Failed to serialize schema: {e}");
                std::process::exit(1);
            });
            (json, "barnum-config-schema.json")
        }
        OutputType::Zod => {
            let zod = schema_to_zod(&schema);
            (zod, "barnum-config-schema.zod.ts")
        }
    };

    // ... resolve workspace_root same as today ...
    let output_path = workspace_root.join("libs/barnum").join(filename);
    fs::write(&output_path, &content).unwrap_or_else(|e| {
        eprintln!("Failed to write {filename}: {e}");
        std::process::exit(1);
    });
    println!("Written: {}", output_path.display());
}
```

No new dependencies for arg parsing — two positional checks are enough.

### Task 2: Implement Zod code generation

**Goal:** A `zod` module in `barnum_config` that converts a schemars `RootSchema` into a TypeScript string containing Zod schemas.

**File:** `crates/barnum_config/src/zod.rs` (new)

The module exports one function:

```rust
use schemars::schema::RootSchema;

/// Convert a schemars RootSchema into a TypeScript file containing Zod schemas.
pub fn schema_to_zod(root: &RootSchema) -> String { ... }
```

#### Schema tree walking

The schemars `RootSchema` has two parts:
- `definitions`: a map of type names to their schemas (e.g., `ActionFile`, `Options`, `StepFile`)
- `schema`: the root schema object (for `ConfigFile`)

The generator:
1. Collects definitions in topological order (a definition that references another must come after it)
2. Emits each definition as a `const` binding
3. Emits the root schema as the final export
4. Emits inferred TypeScript types via `z.infer`

#### Mapping rules

| JSON Schema pattern | Zod output |
|---|---|
| `{"type": "string"}` | `z.string()` |
| `{"type": "integer"}` | `z.number().int()` |
| `{"type": "number"}` | `z.number()` |
| `{"type": "boolean"}` | `z.boolean()` |
| `{"type": "null"}` | `z.null()` |
| `{"type": ["string", "null"]}` | `z.string().nullable()` |
| `{"type": ["integer", "null"]}` | `z.number().int().nullable()` |
| `{"type": "array", "items": X}` | `z.array(X)` |
| `{"type": "object", "properties": {...}}` | `z.object({...})` |
| `{"$ref": "#/definitions/Foo"}` | `Foo` (variable reference) |
| `{"oneOf": [...]}` with `kind` discriminator | `z.discriminatedUnion("kind", [...])` |
| `{"anyOf": [ref, {"type": "null"}]}` | `Ref.nullable()` |
| `{"allOf": [{"$ref": ...}]}` | The referenced schema (schemars wraps refs in `allOf` to attach metadata) |
| `{"enum": ["Pool"]}` (single-value enum) | `z.literal("Pool")` |
| `"additionalProperties": false` | `.strict()` |
| `"default": value` | `.default(value)` |
| `"description": "..."` | `.describe("...")` |
| Property not in `required` array | `.optional()` |

#### Handling schemars idioms

Schemars produces several patterns that need special treatment:

**`allOf` with single `$ref`.** Schemars wraps `$ref` in `allOf` when it needs to attach metadata (description, default) to a reference:
```json
"options": {
  "description": "Global runtime options.",
  "default": {...},
  "allOf": [{"$ref": "#/definitions/Options"}]
}
```
The generator unwraps this to `Options.default({...}).describe("Global runtime options.")`.

**Nullable optional via `anyOf`.** Optional fields with complex types use `anyOf` with a null branch:
```json
"finally": {
  "default": null,
  "anyOf": [
    {"$ref": "#/definitions/FinallyHook"},
    {"type": "null"}
  ]
}
```
The generator recognizes `anyOf` where one branch is `{"type": "null"}` and emits `FinallyHook.nullable()`. Combined with `default: null` and the field being optional, the full output is `FinallyHook.nullable().optional().default(null)`.

**Discriminated unions via `oneOf`.** Tagged enums (`#[serde(tag = "kind")]`) produce `oneOf` where each variant has a `kind` property with a single-value `enum`:
```json
"oneOf": [
  {"properties": {"kind": {"enum": ["Pool"]}, ...}},
  {"properties": {"kind": {"enum": ["Command"]}, ...}}
]
```
The generator detects the common discriminator property and emits `z.discriminatedUnion("kind", [...])`.

**Untagged enums via `anyOf`.** `SchemaRef` uses `#[serde(untagged)]` which produces `anyOf` without a discriminator. The generator falls back to `z.union([...])`.

#### Generated output

For the current schema, the output looks like:

```typescript
// Generated from Rust types — do not edit manually.
// Regenerate with: cargo run -p barnum_config --bin build_barnum_schema

import { z } from "zod";

const MaybeLinked_for_String = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Inline"),
    value: z.string().describe("The content value, provided directly in the config file."),
  }).describe("Inline content."),
  z.object({
    kind: z.literal("Link"),
    path: z.string().describe(
      "Relative path to the file (resolved relative to the config file's directory)."
    ),
  }).describe("Link to a file whose contents will be loaded at runtime."),
]).describe(
  "Content that can be inline or linked to a file."
);

const PreHook = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Command"),
    script: z.string().describe(
      "Shell script to execute. Receives the task's value as JSON on stdin, "
      + "must write the (possibly modified) value as JSON on stdout."
    ),
  }).describe("Run a shell command as the pre hook."),
]);

const PostHook = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Command"),
    script: z.string().describe(
      "Shell script to execute. Receives the action outcome as JSON on stdin, "
      + "must write the (possibly modified) outcome as JSON on stdout."
    ),
  }).describe("Run a shell command as the post hook."),
]);

const FinallyHook = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Command"),
    script: z.string().describe(
      "Shell script to execute. Receives the task's original value as JSON "
      + "on stdin, must write a JSON array of follow-up tasks on stdout."
    ),
  }).describe("Run a shell command as the finally hook."),
]);

const SchemaRef = z.union([
  z.object({
    link: z.string().describe(
      "Relative path to the JSON Schema file (e.g., \"schemas/task.json\")."
    ),
  }).describe("Reference to an external JSON Schema file."),
  z.any().describe("Inline JSON Schema object (any valid JSON Schema)."),
]);

const ActionFile = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Pool"),
    instructions: MaybeLinked_for_String.describe(
      "Markdown prompt shown to the agent processing this task."
    ),
  }).describe("Send the task to the agent pool."),
  z.object({
    kind: z.literal("Command"),
    script: z.string().describe("Shell script to execute."),
  }).describe("Run a local shell command instead of sending to an agent."),
]).describe("How a step processes tasks.");

const Options = z.object({
  timeout: z.number().int().nullable().optional().default(null)
    .describe("Timeout in seconds for each task (None = no timeout)."),
  max_retries: z.number().int().optional().default(0)
    .describe("Maximum retries per task (default: 0)."),
  max_concurrency: z.number().int().nullable().optional().default(null)
    .describe("Maximum concurrent tasks (None = unlimited)."),
  retry_on_timeout: z.boolean().optional().default(true)
    .describe("Whether to retry when agent times out (default: true)."),
  retry_on_invalid_response: z.boolean().optional().default(true)
    .describe("Whether to retry when agent returns invalid response (default: true)."),
}).strict().describe("Global runtime options for task execution.");

const StepOptions = z.object({
  timeout: z.number().int().nullable().optional().default(null)
    .describe("Timeout in seconds for tasks on this step."),
  max_retries: z.number().int().nullable().optional().default(null)
    .describe("Maximum retries for tasks on this step."),
  retry_on_timeout: z.boolean().nullable().optional().default(null)
    .describe("Whether to retry when an agent times out on this step."),
  retry_on_invalid_response: z.boolean().nullable().optional().default(null)
    .describe("Whether to retry when an agent returns an invalid response on this step."),
}).strict().describe("Per-step option overrides.");

const StepFile = z.object({
  name: z.string().describe("Unique name for this step."),
  value_schema: SchemaRef.nullable().optional().default(null)
    .describe("JSON Schema that validates the value payload for tasks on this step."),
  pre: PreHook.nullable().optional().default(null)
    .describe("Shell script that runs before the action."),
  action: ActionFile.describe("How this step processes tasks."),
  post: PostHook.nullable().optional().default(null)
    .describe("Shell script that runs after the action completes (or fails)."),
  next: z.array(z.string()).optional().default([])
    .describe("Step names this step is allowed to spawn follow-up tasks on."),
  finally: FinallyHook.nullable().optional().default(null)
    .describe("Shell script that runs after this task and all its descendants complete."),
  options: StepOptions.optional().default({})
    .describe("Per-step options that override the global options."),
}).strict().describe("A named step in the workflow.");

export const configFileSchema = z.object({
  $schema: z.string().nullable().optional()
    .describe("Optional JSON Schema URL for editor validation. Ignored at runtime."),
  options: Options.optional().default({})
    .describe("Global runtime options."),
  entrypoint: z.string().nullable().optional().default(null)
    .describe("Name of the step that starts the workflow."),
  steps: z.array(StepFile)
    .describe("The steps that make up this workflow."),
}).strict().describe("Top-level Barnum configuration file format.");

// Inferred TypeScript types
export type ConfigFile = z.infer<typeof configFileSchema>;
export type StepFile = z.infer<typeof StepFile>;
export type ActionFile = z.infer<typeof ActionFile>;
export type Options = z.infer<typeof Options>;
export type StepOptions = z.infer<typeof StepOptions>;
export type PreHook = z.infer<typeof PreHook>;
export type PostHook = z.infer<typeof PostHook>;
export type FinallyHook = z.infer<typeof FinallyHook>;

/**
 * Identity function that provides type checking for config objects.
 * Validates at runtime via Zod and returns the parsed config.
 */
export function defineConfig(config: z.input<typeof configFileSchema>): ConfigFile {
  return configFileSchema.parse(config);
}
```

**Note on naming:** The root schema uses `configFileSchema` (camelCase) because `ConfigFile` is reserved for the type export. Internal definitions use PascalCase matching the Rust type names. The generator preserves the schemars definition names as-is (`MaybeLinked_for_String`), which aren't exported.

**Note on `defineConfig`:** This function validates at runtime using Zod's `.parse()`. Unlike the identity-function approach in `TYPESCRIPT_CONFIG.md`, this catches invalid configs at build time (when the TypeScript file is evaluated) rather than at Barnum runtime. The `z.input` type accepts the "loose" input shape (where optional fields can be omitted), while the return type is the "strict" parsed shape (where defaults are filled in).

#### Generator structure

```rust
// zod.rs

use schemars::schema::{
    InstanceType, ObjectValidation, RootSchema, Schema, SchemaObject,
    SingleOrVec, SubschemaValidation,
};
use std::fmt::Write;

pub fn schema_to_zod(root: &RootSchema) -> String {
    let mut out = String::new();
    writeln!(out, "// Generated from Rust types — do not edit manually.").unwrap();
    writeln!(out, "// Regenerate with: cargo run -p barnum_config --bin build_barnum_schema\n").unwrap();
    writeln!(out, "import {{ z }} from \"zod\";\n").unwrap();

    // Emit definitions in dependency order
    let ordered = topological_sort(&root.definitions);
    for name in &ordered {
        let schema = &root.definitions[name.as_str()];
        write!(out, "const {name} = ").unwrap();
        emit_schema(&mut out, schema, &root.definitions);
        writeln!(out, ";\n").unwrap();
    }

    // Emit root schema
    write!(out, "export const configFileSchema = ").unwrap();
    emit_schema_object(&mut out, &root.schema, &root.definitions);
    writeln!(out, ";\n").unwrap();

    // Emit type exports
    // ...

    out
}

fn emit_schema(out: &mut String, schema: &Schema, defs: &Map<String, Schema>) {
    match schema {
        Schema::Bool(true) => write!(out, "z.any()").unwrap(),
        Schema::Bool(false) => write!(out, "z.never()").unwrap(),
        Schema::Object(obj) => emit_schema_object(out, obj, defs),
    }
}

fn emit_schema_object(out: &mut String, obj: &SchemaObject, defs: &Map<String, Schema>) {
    // Handle $ref
    if let Some(ref reference) = obj.reference {
        let name = reference.strip_prefix("#/definitions/").unwrap_or(reference);
        write!(out, "{name}").unwrap();
        return;
    }

    // Handle subschemas (oneOf, anyOf, allOf)
    if let Some(ref subs) = obj.subschemas {
        emit_subschema(out, obj, subs, defs);
        return;
    }

    // Handle instance types
    // ... (string, number, boolean, array, object, nullable)
}
```

The full implementation handles each JSON Schema pattern described in the mapping table. The function is recursive: objects emit their properties by calling `emit_schema` on each property's schema.

#### Topological sort

Definitions reference each other via `$ref`. The generator collects all `$ref` targets from each definition and sorts accordingly. In the current schema, the dependency graph is:

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

The topological sort ensures each definition appears after its dependencies.

### Task 3: Register the `zod` module

**File:** `crates/barnum_config/src/lib.rs`

Add:
```rust
pub mod zod;
```

### Task 4: Update pre-commit hook

**File:** `.githooks/pre-commit`

Before:
```bash
echo "Regenerating config schema..."
cargo run -p barnum_config --bin build_barnum_schema 2>/dev/null
```

After:
```bash
echo "Regenerating config schemas..."
cargo run -p barnum_config --bin build_barnum_schema -- --type json 2>/dev/null
cargo run -p barnum_config --bin build_barnum_schema -- --type zod 2>/dev/null
```

### Task 5: Update CI verification

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
  run: |
    cargo run -p barnum_config --bin build_barnum_schema -- --type json
    cargo run -p barnum_config --bin build_barnum_schema -- --type zod
- name: Check schemas are up to date
  run: |
    if ! git diff --exit-code libs/barnum/barnum-config-schema.json libs/barnum/barnum-config-schema.zod.ts; then
      echo "::error::Generated schemas differ from checked-in versions. Run build_barnum_schema for both types and commit."
      exit 1
    fi
```

### Task 6: Update npm package

**File:** `libs/barnum/package.json`

Before:
```json
{
  "name": "@barnum/barnum",
  "main": "index.js",
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
  "main": "index.js",
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

`zod` is an optional peer dependency: users who only use JSONC configs with JSON Schema validation don't need it. Users who `import { defineConfig } from "@barnum/barnum"` need Zod in their project.

### Task 7: Retire TYPESCRIPT_CONFIG.md

Move `refactors/pending/TYPESCRIPT_CONFIG.md` to `refactors/past/`. The Zod approach supersedes Specta/ts-rs because:
- Zod provides runtime validation in addition to types
- Generation reuses the existing schemars infrastructure (no new Rust derive macros)
- The npm package ships a single `.ts` file instead of needing separate `.d.ts` + `.js` files

## User Workflow

Install the package and Zod:
```bash
pnpm add -D @barnum/barnum zod
```

Write a typed config:
```typescript
// workflow.config.ts
import { defineConfig } from "@barnum/barnum/barnum-config-schema.zod";

export default defineConfig({
  entrypoint: "Analyze",
  options: { max_retries: 3, max_concurrency: 5 },
  steps: [
    {
      name: "Analyze",
      action: {
        kind: "Pool",
        instructions: { kind: "Inline", value: "Analyze the code." },
      },
      next: ["Implement"],
    },
    {
      name: "Implement",
      action: {
        kind: "Command",
        script: "jq '.value | .plan' | bash",
      },
    },
  ],
});
```

Editor provides completion on every field. `defineConfig` validates the config object at evaluation time and reports Zod errors with paths (e.g., `steps[0].action.kind: Expected "Pool" | "Command"`).

## Open Questions

1. **Import path.** The example above imports from `@barnum/barnum/barnum-config-schema.zod`. This requires the `exports` field in `package.json` for subpath exports, or users import the full path. An alternative is re-exporting from `index.js`, but that forces all users to load Zod even if they only use the CLI binary.

2. **`.ts` distribution.** Shipping raw `.ts` in the npm package requires consumers to have a TypeScript-aware build pipeline. Most modern setups (Vite, tsx, ts-node, Bun) handle this. If broader compatibility is needed, the Rust generator could emit `.js` + `.d.ts` directly (the Zod schema as JS is identical to the TS, minus the `z.infer` type exports, which move to a `.d.ts`). This is mechanical and could be added later.

3. **Description truncation.** Some doc comments in the Rust types are long (multi-paragraph). The generated `.describe()` calls include the full text. This is correct but verbose. The generator could truncate to the first sentence, but that loses information. Recommendation: include the full text. Users can collapse it in their editor.

4. **`minimum: 0.0` on integers.** The JSON Schema has `"minimum": 0.0` on unsigned integer fields (from Rust's `u32`/`u64`). The Zod equivalent is `.nonnegative()`. The generator should detect `minimum: 0` on integer types and emit `.int().nonnegative()` instead of just `.int()`.
