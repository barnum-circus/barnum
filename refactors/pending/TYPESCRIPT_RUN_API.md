# TypeScript Run API

**Status:** Pending

## Motivation

Users who write TypeScript configs with `defineConfig` should also be able to invoke barnum programmatically from TypeScript. The package should export typed functions that spawn the barnum binary, where the parameter types are generated from the same Rust structs the CLI uses at runtime.

The generation pipeline mirrors what already exists for the config schema: Rust types derive `JsonSchema` (schemars), a binary emits the schemars `RootSchema`, `emit_zod` renders it to a TypeScript file with Zod schemas, and CI verifies the checked-in file matches the generated output.

## Current State

### Config schema generation pipeline

`barnum_config` defines `ConfigFile` with `#[derive(JsonSchema)]`. The binary `build_barnum_schema` (`crates/barnum_config/src/bin/build_barnum_schema.rs`) calls `config_schema()` to get a `RootSchema`, then calls `emit_zod()` (`crates/barnum_config/src/zod.rs:18`) to render it as `libs/barnum/barnum-config-schema.zod.ts`. CI verifies this file is in sync.

### CLI types (`crates/barnum_cli/src/main.rs:20-149`)

The CLI types live in `main.rs` as private types in a binary crate:

- `LogLevel` (lines 21-36) — `#[derive(ValueEnum)]`, six variants: Off, Error, Warn, Info, Debug, Trace
- `Cli` (lines 40-55) — `#[derive(Parser)]`, global fields: `root: Option<PathBuf>`, `log_level: LogLevel`, `command: Command`
- `Command` (lines 57-110) — `#[derive(Subcommand)]`, three variants: `Run { ... }`, `Config { command: ConfigCommand }`, `Version { json: bool }`
- `ConfigCommand` (lines 112-141) — `#[derive(Subcommand)]`, four variants: `Docs`, `Validate`, `Graph`, `Schema`
- `SchemaType` (lines 144-149) — `#[derive(ValueEnum)]`, two variants: Zod, Json

None of these currently derive `Serialize` or `JsonSchema`. They're private to the binary, so no other crate can reference them.

### Package exports (`libs/barnum/package.json`)

Task 1 (barrel file) is already done — `index.ts` re-exports from `barnum-config-schema.zod.ts` and the root export points at it.

## Proposed Changes

### Task 1: Extract CLI types to a library ~~Create barrel file~~

Done. `libs/barnum/index.ts` exists and the root export points at it.

### Task 2: Extract CLI types into `barnum_cli`'s library target

The CLI types are currently private in `main.rs`. To generate schemas from them, they need to be importable by a binary. The simplest path: add `src/lib.rs` to `barnum_cli` and move the type definitions there. `main.rs` imports them with `use barnum_cli::*`.

**File:** `crates/barnum_cli/src/lib.rs` (new)

```rust
use clap::{Parser, Subcommand, ValueEnum};
use schemars::JsonSchema;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Default, ValueEnum, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

#[derive(Parser, Serialize, JsonSchema)]
#[command(name = "barnum")]
#[command(about = "Barnum - workflow engine for agents")]
pub struct Cli {
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,

    #[arg(short, long, global = true, default_value = "info")]
    pub log_level: LogLevel,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Serialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Command {
    #[serde(rename_all = "camelCase")]
    Run {
        #[arg(long, required_unless_present = "resume_from")]
        pub config: Option<String>,

        #[arg(long, conflicts_with = "resume_from")]
        pub initial_state: Option<String>,

        #[arg(long, conflicts_with = "resume_from")]
        pub entrypoint_value: Option<String>,

        #[arg(long)]
        pub pool: Option<String>,

        #[arg(long)]
        pub wake: Option<String>,

        #[arg(long)]
        #[serde(serialize_with = "serialize_pathbuf_option")]
        pub log_file: Option<PathBuf>,

        #[arg(long)]
        #[serde(serialize_with = "serialize_pathbuf_option")]
        pub state_log: Option<PathBuf>,

        #[arg(long, conflicts_with = "config")]
        #[serde(serialize_with = "serialize_pathbuf_option")]
        pub resume_from: Option<PathBuf>,
    },

    Config {
        #[command(subcommand)]
        pub command: ConfigCommand,
    },

    Version {
        #[arg(long)]
        pub json: bool,
    },
}

#[derive(Subcommand, Serialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum ConfigCommand {
    Docs {
        #[arg(long)]
        pub config: String,
    },
    Validate {
        #[arg(long)]
        pub config: String,
    },
    Graph {
        #[arg(long)]
        pub config: String,
    },
    Schema {
        #[arg(long = "type", default_value = "zod")]
        #[serde(rename = "type")]
        pub schema_type: SchemaType,
    },
}

#[derive(Debug, Clone, Copy, Default, ValueEnum, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SchemaType {
    #[default]
    Zod,
    Json,
}
```

`main.rs` becomes a thin wrapper that imports from the lib and runs the CLI.

**Complication: clap + serde + schemars on the same types.** Clap's `#[derive(Parser)]` and serde's `#[derive(Serialize)]` coexist on the same struct — clap reads `#[arg]` and `#[command]` attributes, serde reads `#[serde]` attributes. schemars' `#[derive(JsonSchema)]` follows serde's conventions for field naming and enum representation. These derive macros don't conflict.

**Complication: `PathBuf` fields.** schemars renders `PathBuf` as `{"type": "string"}`, which is correct. serde serializes `PathBuf` using its `Display` impl. No special handling needed for schema generation.

**Complication: clap `Subcommand` vs serde enum representation.** clap's `#[derive(Subcommand)]` makes the enum a CLI subcommand; serde's `#[serde(tag = "kind")]` makes it an internally tagged enum. Both derive macros process their own attributes independently. The schemars output follows the serde representation, which is what we want for TypeScript: a discriminated union on `"kind"`.

### Task 3: Add CLI schema generation binary

**File:** `crates/barnum_cli/src/bin/build_cli_schema.rs` (new)

```rust
use barnum_cli::Command;
use barnum_config::zod::emit_zod;

fn main() {
    let root = schemars::schema_for!(Command);
    let zod = emit_zod(&root);

    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let out = workspace_root.join("libs/barnum/barnum-cli-schema.zod.ts");

    std::fs::write(&out, &zod).unwrap();
    println!("Written: {}", out.display());
}
```

**Why `schema_for!(Command)` and not `schema_for!(Cli)`?** The global fields (`root`, `log_level`) aren't subcommand-specific — they belong on every TypeScript function as optional parameters. Generating the schema for `Command` gives us the discriminated union of subcommands. The global fields are added separately in the hand-written TypeScript functions (two fields, stable, not worth generating).

Alternatively, `schema_for!(Cli)` produces a schema for the whole CLI struct, and the TypeScript side destructures it. Either works. Using `Command` keeps the generated types focused on the part that varies (subcommands) while the stable global options are in the hand-written wrapper.

**Cargo.toml change:** Add a `[[bin]]` section:

```toml
[[bin]]
name = "build_cli_schema"
path = "src/bin/build_cli_schema.rs"
```

And add `schemars` + `serde` to dependencies (they're workspace deps already available).

### Task 4: Generated CLI schema file

**File:** `libs/barnum/barnum-cli-schema.zod.ts` (generated)

The exact output depends on what schemars produces, but based on the existing `emit_zod` behavior with tagged enums and the types in Task 2, it would look approximately like:

```typescript
import { z } from "zod";

const LogLevel = z.enum(["off", "error", "warn", "info", "debug", "trace"]);

const SchemaType = z.enum(["zod", "json"]);

const ConfigCommand = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Docs"),
    config: z.string(),
  }),
  z.object({
    kind: z.literal("Validate"),
    config: z.string(),
  }),
  z.object({
    kind: z.literal("Graph"),
    config: z.string(),
  }),
  z.object({
    kind: z.literal("Schema"),
    type: SchemaType.optional().default("zod"),
  }),
]);

export const commandSchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("Run"),
    config: z.string().optional(),
    initialState: z.string().optional(),
    entrypointValue: z.string().optional(),
    pool: z.string().optional(),
    wake: z.string().optional(),
    logFile: z.string().optional(),
    stateLog: z.string().optional(),
    resumeFrom: z.string().optional(),
  }),
  z.object({
    kind: z.literal("Config"),
    command: ConfigCommand,
  }),
  z.object({
    kind: z.literal("Version"),
    json: z.boolean().optional().default(false),
  }),
]);

export type Command = z.infer<typeof commandSchema>;
export type LogLevel = z.infer<typeof LogLevel>;
export type SchemaType = z.infer<typeof SchemaType>;
export type ConfigCommand = z.infer<typeof ConfigCommand>;
```

This file is checked in and regenerated by `cargo run -p barnum_cli --bin build_cli_schema`. CI verifies it's in sync.

### Task 5: Hand-written TypeScript functions

**File:** `libs/barnum/run.ts` (new)

The functions that spawn the binary are hand-written because the mapping from a TypeScript object to CLI args involves string manipulation (camelCase to kebab-case, flattening nested objects to positional subcommands) that doesn't belong in the generated schema file.

```typescript
import { spawn, type ChildProcess } from "node:child_process";
import { chmodSync } from "node:fs";
import { createRequire } from "node:module";
import type { Command } from "./barnum-cli-schema.zod.js";

const require = createRequire(import.meta.url);
const binaryPath: string = require("./index.js");

export interface GlobalOptions {
  root?: string;
  logLevel?: string;
}

function ensureExecutable(bin: string): void {
  try { chmodSync(bin, 0o755); } catch {}
}

function pushGlobal(args: string[], opts: GlobalOptions): void {
  if (opts.root)     args.push("--root", opts.root);
  if (opts.logLevel) args.push("--log-level", opts.logLevel);
}

function camelToKebab(s: string): string {
  return s.replace(/[A-Z]/g, m => `-${m.toLowerCase()}`);
}

/** Spawn `barnum run` with the given options. */
export function barnumRun(
  opts: Extract<Command, { kind: "Run" }>,
  global?: GlobalOptions,
): ChildProcess {
  ensureExecutable(binaryPath);
  const args: string[] = [];
  pushGlobal(args, global ?? {});
  args.push("run");

  for (const [key, value] of Object.entries(opts)) {
    if (key === "kind" || value == null) continue;
    args.push(`--${camelToKebab(key)}`, String(value));
  }

  return spawn(binaryPath, args, { stdio: "inherit" });
}

/** Spawn `barnum` with an arbitrary command. */
export function barnum(command: Command, global?: GlobalOptions): ChildProcess {
  switch (command.kind) {
    case "Run":
      return barnumRun(command, global);
    case "Config":
      return barnumConfig(command.command, global);
    case "Version":
      return barnumVersion(command, global);
  }
}

// ... barnumConfig, barnumVersion follow the same pattern
```

Each function extracts the relevant fields from the typed command object and maps them to CLI args. The `Command` type (from the generated schema) is the parameter type, so TypeScript callers get full type checking against the actual CLI interface.

### Task 6: Update barrel file and package.json

**File:** `libs/barnum/index.ts`

Add re-exports for the new generated types and hand-written functions:

```typescript
export * from "./barnum-config-schema.zod.js";
export * from "./barnum-cli-schema.zod.js";
export { barnum, barnumRun, type GlobalOptions } from "./run.js";
```

**File:** `libs/barnum/package.json`

Add `barnum-cli-schema.zod.ts` and `run.ts` to `files`.

### Task 7: CI verification

Extend the existing CI check that verifies `barnum-config-schema.zod.ts` is in sync. Add a second check:

```bash
cargo run -p barnum_cli --bin build_cli_schema
git diff --exit-code libs/barnum/barnum-cli-schema.zod.ts
```

Also extend the pre-commit hook to regenerate both files.

**File:** `CLAUDE.md` generated artifacts section — add:

```
- `libs/barnum/barnum-cli-schema.zod.ts` — regenerate with `cargo run -p barnum_cli --bin build_cli_schema`
```

## Open Questions

1. **`schema_for!(Command)` vs `schema_for!(Cli)`.** Generating for `Command` keeps the discriminated union clean — TypeScript callers switch on `kind`. Generating for `Cli` would produce a flat struct with `root`, `logLevel`, and `command` fields. The former is more ergonomic (global options go in a separate `GlobalOptions` param); the latter is a more faithful mirror of the Rust types. Leaning toward `Command` since the global options are two stable fields that don't need generation.

2. **`emit_zod` changes needed.** The existing `emit_zod` was built for the config schema. The CLI schema uses the same schemars primitives (tagged enums, optional fields, string enums) so it should work without changes. If it doesn't handle some schemars pattern the CLI types produce, we'd need to extend it. The risk is low since the config schema already exercises discriminated unions, optional fields, and string enums.

3. **Field naming: `rename_all = "camelCase"` on each variant vs a different approach.** Putting `#[serde(rename_all = "camelCase")]` on each struct variant means schemars sees camelCase field names and the generated Zod schema uses camelCase property names. This is the right convention for TypeScript consumers. The hand-written `camelToKebab` in `run.ts` converts back to CLI flag names. An alternative: leave field names as snake_case in the schema and convert to camelCase in TypeScript, but that pushes Rust naming conventions onto TypeScript users.
