# TypeScript Run API

**Status:** Pending

## Motivation

The npm package should export typed functions that spawn the barnum binary. The parameter types are generated from the Rust CLI structs via schemars. When CLI args change, regenerate the file and CI catches drift.

## Current State

### Config schema generation pipeline

`barnum_config` defines `ConfigFile` with `#[derive(JsonSchema)]`. `build_barnum_schema` (`crates/barnum_config/src/bin/build_barnum_schema.rs`) calls `config_schema()`, pipes the `RootSchema` through `emit_zod()` (`crates/barnum_config/src/zod.rs:18`), and writes `libs/barnum/barnum-config-schema.zod.ts`. CI verifies this file is in sync.

### CLI types (`crates/barnum_cli/src/main.rs:20-149`)

Private types in a binary crate: `LogLevel`, `Cli`, `Command`, `ConfigCommand`, `SchemaType`. None derive `Serialize` or `JsonSchema`. Not importable from other crates.

### Package exports

Barrel file (`index.ts`) done — re-exports from `barnum-config-schema.zod.ts`.

## Proposed Changes

### Task 1: Barrel file

Done.

### Task 2: Extract CLI types into `barnum_cli`'s library target

Add `src/lib.rs` to `barnum_cli`. Move the type definitions there. `main.rs` imports them with `use barnum_cli::*`.

Add `Serialize` and `JsonSchema` derives to all CLI types. Add `#[serde(tag = "kind")]` to `Command` and `ConfigCommand` so schemars emits discriminated unions. Add `#[serde(rename_all = "camelCase")]` on struct variants with multi-word fields so TypeScript gets camelCase property names. Add `#[serde(rename_all = "lowercase")]` on `LogLevel` and `SchemaType` so their string representations match what clap accepts.

`PathBuf` fields: schemars renders these as `{"type": "string"}`. No special handling needed.

clap and serde/schemars derive macros coexist — they read different attribute namespaces.

### Task 3: CLI schema generation binary

**File:** `crates/barnum_cli/src/bin/build_cli_schema.rs` (new)

A binary that calls `schemars::schema_for!(Cli)` to get a `RootSchema`, then walks it with a new emitter function (`emit_cli_ts`, in `barnum_config` alongside `emit_zod`) that generates plain TypeScript interfaces and spawn functions.

`emit_zod` stays untouched — it generates Zod validation schemas for config files, where runtime validation is useful. The CLI types don't need Zod validation (nobody parses untrusted CLI opts through Zod). A separate emitter generates plain TypeScript interfaces from the schemars `RootSchema` and emits the spawn functions that map typed objects to CLI args.

```rust
use barnum_cli::Cli;
use barnum_config::cli_ts::emit_cli_ts;

fn main() {
    let root = schemars::schema_for!(Cli);
    let ts = emit_cli_ts(&root);

    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let out = workspace_root.join("libs/barnum/barnum-cli.ts");

    std::fs::write(&out, &ts).unwrap();
    println!("Written: {}", out.display());
}
```

Add `[[bin]]` to `Cargo.toml` and add `schemars` + `serde` to `barnum_cli`'s dependencies.

### Task 4: `emit_cli_ts` emitter

**File:** `crates/barnum_config/src/cli_ts.rs` (new)

Walks a schemars `RootSchema` and emits:

1. TypeScript interfaces for each definition (structs become interfaces, enums become string literal unions)
2. A discriminated union type for the `Command` enum
3. Spawn functions that map each interface's fields to `--kebab-case` CLI args

The emitter reuses the same schemars tree-walking patterns as `emit_zod` (topological sort, discriminated union detection, property iteration) but outputs plain TypeScript instead of Zod schemas.

Field name conversion: schemars property names (camelCase, from `#[serde(rename_all = "camelCase")]`) are used as TypeScript field names. The spawn function converts camelCase back to kebab-case for CLI flags (`initialState` → `--initial-state`).

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

### Task 5: Update barrel and package.json

**File:** `libs/barnum/index.ts`

```typescript
export * from "./barnum-config-schema.zod.js";
export * from "./barnum-cli.js";
```

Add `barnum-cli.ts` to `files` in `package.json`.

### Task 6: CI verification + pre-commit hook

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
