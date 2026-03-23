# TypeScript CLI API

**Status:** Pending (Task 1 done)

## Motivation

The npm package should export typed functions for invoking the barnum binary. Types are generated from Rust CLI structs via schemars. When CLI args change, regenerate and CI catches drift.

## Current State

`barnum_config` owns the schemars-to-TypeScript pipeline: `emit_zod` in `crates/barnum_config/src/zod.rs` walks a `RootSchema` and emits Zod TypeScript. The helper functions it uses (topological sort, discriminated union detection, ref collection, JS formatting) are general-purpose — they work on any schemars `RootSchema`, not just config schemas.

CLI types (`Cli`, `Command`, `ConfigCommand`, `LogLevel`, `SchemaType`) live in `crates/barnum_cli/src/main.rs:20-149` as private types in a binary crate. None derive `JsonSchema` or `Serialize`.

## Proposed Changes

### Task 1: Barrel file

Done.

### Task 2: Extract schemars emitters into their own crate

**New crate:** `crates/schemars_emit` (or `crates/schema_ts`, name TBD)

Move `zod.rs` out of `barnum_config` and into this new crate. All the tree-walking infrastructure comes with it: `topological_sort`, `collect_refs`, `find_discriminator`, `is_null_schema`, the `Emitter` struct, JS formatting helpers.

The new crate exports two public functions:

- `emit_zod(root: &RootSchema) -> String` — existing function, unchanged output. `barnum_config`'s `build_barnum_schema` calls this.
- `emit_cli_ts(root: &RootSchema) -> String` — new function, same tree-walking, different output format. Emits plain TypeScript interfaces and spawn functions instead of Zod schemas.

Both functions share the internal traversal code. The only difference is the rendering: `z.string()` vs `string`, `z.object({})` vs `interface {}`, plus `emit_cli_ts` additionally generates spawn functions.

Update `barnum_config` to depend on `schemars_emit` instead of having `zod.rs` inline. The `build_barnum_schema` binary changes from `use barnum_config::zod::emit_zod` to `use schemars_emit::emit_zod`. `barnum_config` re-exports if needed for backward compat, or just update the call site.

### Task 3: Extract CLI types into `barnum_cli`'s library target

Add `src/lib.rs` to `barnum_cli`. Move type definitions there. `main.rs` imports them with `use barnum_cli::*`.

Add `Serialize` and `JsonSchema` derives to all CLI types. Add `#[serde(tag = "kind")]` to `Command` and `ConfigCommand` for discriminated unions. Add `#[serde(rename_all = "camelCase")]` on struct variants with multi-word fields. Add `#[serde(rename_all = "lowercase")]` on `LogLevel` and `SchemaType`.

### Task 4: CLI schema generation binary

**File:** `crates/barnum_cli/src/bin/build_cli_schema.rs` (new)

```rust
use barnum_cli::Cli;
use schemars_emit::emit_cli_ts;

fn main() {
    let root = schemars::schema_for!(Cli);
    let ts = emit_cli_ts(&root);
    // write to libs/barnum/barnum-cli.ts
}
```

`barnum_cli` depends on `schemars_emit` for the emitter. No dependency on `barnum_config` for schema generation.

### Task 5: `emit_cli_ts` implementation

Lives in the new `schemars_emit` crate alongside `emit_zod`, sharing all tree-walking internals. Walks a schemars `RootSchema` and emits:

1. TypeScript type aliases for string enums
2. TypeScript interfaces for object types (structs)
3. Discriminated union types for tagged enums
4. Spawn functions that map each interface's fields to `--kebab-case` CLI args
5. A top-level `barnum(cli: Cli)` dispatcher and convenience functions like `barnumRun`

Generated output (approximate shape):

```typescript
import { spawn, type ChildProcess } from "node:child_process";
import { chmodSync } from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const binaryPath: string = require("./index.js");

function spawnBarnum(args: string[]): ChildProcess {
  try { chmodSync(binaryPath, 0o755); } catch {}
  return spawn(binaryPath, args, { stdio: "inherit" });
}

export type LogLevel = "off" | "error" | "warn" | "info" | "debug" | "trace";
export type SchemaType = "zod" | "json";

export interface RunCommand {
  kind: "Run";
  config?: string;
  initialState?: string;
  entrypointValue?: string;
  pool?: string;
  wake?: string;
  logFile?: string;
  stateLog?: string;
  resumeFrom?: string;
}

export interface ConfigDocsCommand { kind: "Docs"; config: string; }
export interface ConfigValidateCommand { kind: "Validate"; config: string; }
export interface ConfigGraphCommand { kind: "Graph"; config: string; }
export interface ConfigSchemaCommand { kind: "Schema"; type?: SchemaType; }

export type ConfigCommand =
  | ConfigDocsCommand
  | ConfigValidateCommand
  | ConfigGraphCommand
  | ConfigSchemaCommand;

export interface ConfigCommandWrapper { kind: "Config"; command: ConfigCommand; }
export interface VersionCommand { kind: "Version"; json?: boolean; }
export type Command = RunCommand | ConfigCommandWrapper | VersionCommand;

export interface Cli {
  root?: string;
  logLevel?: LogLevel;
  command: Command;
}

function camelToKebab(s: string): string {
  return s.replace(/[A-Z]/g, m => `-${m.toLowerCase()}`);
}

function pushFields(args: string[], obj: Record<string, unknown>, skip: string[]): void {
  for (const [key, value] of Object.entries(obj)) {
    if (skip.includes(key) || value == null) continue;
    if (typeof value === "boolean") {
      if (value) args.push(`--${camelToKebab(key)}`);
    } else {
      args.push(`--${camelToKebab(key)}`, String(value));
    }
  }
}

export function barnum(cli: Cli): ChildProcess {
  const args: string[] = [];
  if (cli.root) args.push("--root", cli.root);
  if (cli.logLevel) args.push("--log-level", cli.logLevel);

  switch (cli.command.kind) {
    case "Run": {
      args.push("run");
      pushFields(args, cli.command, ["kind"]);
      return spawnBarnum(args);
    }
    case "Config": {
      args.push("config");
      const sub = cli.command.command;
      args.push(sub.kind.toLowerCase());
      pushFields(args, sub, ["kind"]);
      return spawnBarnum(args);
    }
    case "Version": {
      args.push("version");
      pushFields(args, cli.command, ["kind"]);
      return spawnBarnum(args);
    }
  }
}

export function barnumRun(
  opts: Omit<RunCommand, "kind">,
  global?: { root?: string; logLevel?: LogLevel },
): ChildProcess {
  return barnum({ ...global, command: { kind: "Run", ...opts } });
}
```

### Task 6: Update barrel and package.json

**File:** `libs/barnum/index.ts`

```typescript
export * from "./barnum-config-schema.zod.js";
export * from "./barnum-cli.js";
```

Add `barnum-cli.ts` to `files` in `package.json`.

### Task 7: CI verification + pre-commit hook

Add to CI:

```bash
cargo run -p barnum_cli --bin build_cli_schema
git diff --exit-code libs/barnum/barnum-cli.ts
```

Extend pre-commit hook to regenerate both files.

Add to `CLAUDE.md` generated artifacts:

```
- `libs/barnum/barnum-cli.ts` — regenerate with `cargo run -p barnum_cli --bin build_cli_schema`
```
