# TypeScript CLI API

**Status:** Pending (Task 1 — barrel file — already on master)

## Motivation

The npm package should export typed functions for invoking the barnum binary. A TypeScript caller should be able to `import { barnumRun } from "@barnum/barnum"` and get type-checked parameters matching the Rust CLI. The types and arg-building code should be generated from the Rust structs using the same schemars pipeline that already generates the Zod config schema.

## Current State

`crates/barnum_config/src/zod.rs:18` defines `emit_zod(root: &RootSchema) -> String`, which walks a schemars `RootSchema` and emits Zod TypeScript. `crates/barnum_config/src/bin/build_barnum_schema.rs` calls `config_schema()` to get the `RootSchema` and feeds it to `emit_zod`. The output lands at `libs/barnum/barnum-config-schema.zod.ts`.

`crates/barnum_cli/src/main.rs:40-141` defines the CLI via clap derives: `Cli` (globals: `root`, `log_level`), `Command` (subcommands: `Run`, `Config`, `Version`), and nested `ConfigCommand` (`Docs`, `Validate`, `Graph`, `Schema`).

`libs/barnum/index.js` resolves the platform-specific binary path and exports it as a string.

## Completed

### Task 1: Barrel file

`index.ts` re-exports from `barnum-config-schema.zod.ts`. `package.json` root export points at the barrel.

## Remaining Work

### Task 2: Derive `JsonSchema` on CLI structs

**File:** `crates/barnum_cli/src/main.rs`

Add `#[derive(JsonSchema)]` (from schemars) to `Cli`, `Command`, `LogLevel`, `ConfigCommand`, `SchemaType`. schemars needs `Serialize` too, so add both. These structs currently only derive clap traits.

The schemars representation of `Command` will be a `oneOf` (each variant becomes a tagged object). `LogLevel` and `SchemaType` become string enums. Field names stay snake_case in the schema — the emitter converts to camelCase for TS and kebab-case for CLI flags.

One complication: the clap struct has `Cli.command: Command` as a subcommand, not a regular field. schemars will represent it as a nested object/enum, which is the right shape — the emitter flattens this into separate functions per terminal command.

The `Cli` struct's `root: Option<PathBuf>` and `log_level: LogLevel` fields need schemars to see them. `PathBuf` maps to `string` in schemars. `LogLevel` maps to a string enum.

Expose a public function (like `config_schema()` exists for config):

```rust
pub fn cli_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(Cli)
}
```

### Task 3: Write `emit_cli_ts` emitter

**File:** `crates/barnum_config/src/cli_ts.rs` (new)

A second schemars-to-TypeScript emitter, parallel to `emit_zod`, that generates CLI spawn functions. It walks the `RootSchema` and emits:

1. A `GlobalOptions` interface from the `Cli` struct's non-subcommand fields
2. An options interface per terminal command (e.g., `RunOptions extends GlobalOptions`)
3. A spawn function per terminal command (e.g., `barnumRun(options: RunOptions): ChildProcess`)
4. A discriminated union `BarnumCommand` and top-level `barnum()` dispatcher

The emitter maps schemars types to TypeScript types: `string` → `string`, `boolean` → `boolean`, string enum → string literal union, `integer`/`number` → `number`. Optional fields get `?`.

For arg building, the emitter converts each property name: `log_level` → `--log-level` for the CLI flag, `logLevel` for the TS field. It knows boolean fields use `SetTrue` semantics (flag with no value) vs string fields (flag with value) from the schemars type.

Special case: any field named `config` whose schemars type is `string` gets widened to `string | ConfigFile` in the generated TS. The spawn function stringifies objects with `JSON.stringify` before passing to the CLI. This is a hardcoded override in the emitter.

Generated output (sketch of what `emit_cli_ts` produces):

```typescript
import { spawn, type ChildProcess } from "node:child_process";
import { chmodSync } from "node:fs";
import { createRequire } from "node:module";
import type { ConfigFile } from "./barnum-config-schema.zod.js";

const require = createRequire(import.meta.url);
const bin: string = require("./index.js");

function spawnBarnum(args: string[]): ChildProcess {
  try { chmodSync(bin, 0o755); } catch {}
  return spawn(bin, args, { stdio: "inherit" });
}

export interface GlobalOptions {
  /** Root directory. Pools live in `<root>/pools/<id>/`. */
  root?: string;
  /** Log level (debug shows task return values). */
  logLevel?: "off" | "error" | "warn" | "info" | "debug" | "trace";
}

export interface RunOptions extends GlobalOptions {
  config?: string | ConfigFile;
  initialState?: string;
  entrypointValue?: string;
  pool?: string;
  wake?: string;
  logFile?: string;
  stateLog?: string;
  resumeFrom?: string;
}

// ... interfaces for ConfigDocs, ConfigValidate, ConfigGraph, ConfigSchema, Version ...

export function barnumRun(options: RunOptions): ChildProcess {
  const args: string[] = [];
  if (options.root) { args.push("--root", options.root); }
  if (options.logLevel) { args.push("--log-level", options.logLevel); }
  args.push("run");
  if (options.config != null) {
    args.push("--config", typeof options.config === "object"
      ? JSON.stringify(options.config) : options.config);
  }
  if (options.initialState) { args.push("--initial-state", options.initialState); }
  // ... remaining fields ...
  return spawnBarnum(args);
}

// ... barnumConfigDocs, barnumConfigValidate, barnumConfigGraph, barnumConfigSchema, barnumVersion ...

export type BarnumCommand =
  | { command: "run" } & RunOptions
  | { command: "config docs" } & ConfigDocsOptions
  | { command: "config validate" } & ConfigValidateOptions
  | { command: "config graph" } & ConfigGraphOptions
  | { command: "config schema" } & ConfigSchemaOptions
  | { command: "version" } & VersionOptions;

export function barnum(options: BarnumCommand): ChildProcess {
  switch (options.command) {
    case "run": return barnumRun(options);
    case "config docs": return barnumConfigDocs(options);
    case "config validate": return barnumConfigValidate(options);
    case "config graph": return barnumConfigGraph(options);
    case "config schema": return barnumConfigSchema(options);
    case "version": return barnumVersion(options);
  }
}
```

All functions return `ChildProcess`.

### Task 4: Generation binary

**File:** `crates/barnum_cli/src/bin/build_barnum_cli_ts.rs` (new)

Lives in `barnum_cli` (not `barnum_config`) because `Cli` is defined there and `barnum_cli` depends on `barnum_config`, not vice versa. Calls `cli_schema()` and feeds the result to `emit_cli_ts`. Writes output to `libs/barnum/barnum-cli.ts`.

```rust
use barnum_cli::cli_schema;
use barnum_config::cli_ts::emit_cli_ts;

fn main() {
    let root = cli_schema();
    let ts = emit_cli_ts(&root);
    // write to libs/barnum/barnum-cli.ts
}
```

This means `cli_schema()` needs to be a public function exported from `barnum_cli`. Currently `barnum_cli` is a binary crate. It needs to become a binary+library crate (with `src/lib.rs` exporting the schema function) or the CLI structs need to move to a shared location. The simplest path: add a `src/lib.rs` to `barnum_cli` that re-exports `cli_schema()`.

### Task 5: Regeneration infrastructure

Add `libs/barnum/barnum-cli.ts` to:

- `CLAUDE.md` generated artifacts list with regeneration command
- Pre-commit hook (same pattern as schema generation)
- CI verification (regenerate and diff)
- `package.json` `files` array

### Task 6: Update barrel

**File:** `libs/barnum/index.ts`

```typescript
export * from "./barnum-config-schema.zod.js";
export * from "./barnum-cli.js";
```
