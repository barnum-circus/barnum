# TypeScript CLI API

**Status:** Pending (Task 1 — barrel file — landed on `refactor/barrel-file`, pending CI)

## Motivation

The npm package should export typed functions for invoking the barnum binary. A TypeScript caller should be able to `import { barnumRun } from "@barnum/barnum"` and get type-checked parameters matching the Rust CLI. The types and arg-building code should be generated from the clap struct so they stay in sync automatically.

## Current State

`crates/barnum_cli/src/main.rs:40-141` defines the CLI via clap derives:

- `Cli` — global fields: `root` (`Option<PathBuf>`), `log_level` (`LogLevel`)
- `Command::Run` — `config`, `initial_state`, `entrypoint_value`, `pool`, `wake`, `log_file`, `state_log`, `resume_from`
- `Command::Config` — subcommands: `Docs`, `Validate`, `Graph`, `Schema`
- `Command::Version` — `json` flag
- `LogLevel` — `Off`, `Error`, `Warn`, `Info`, `Debug`, `Trace`
- `SchemaType` — `Zod`, `Json`

`libs/barnum/index.js` resolves the platform-specific binary path and exports it as a string.

## Completed

### Task 1: Barrel file

`index.ts` re-exports from `barnum-config-schema.zod.ts`. `package.json` root export points at the barrel. Landed on `refactor/barrel-file`, pending CI.

## Remaining Work

### Task 2: Generator binary

**File:** `crates/barnum_cli/src/bin/build_barnum_cli_ts.rs` (new)

A binary that calls `Cli::command()` (via clap's `CommandFactory`) and walks the command tree to emit TypeScript. For each `clap::Arg`, the generator reads:

- `get_long()` — the flag name, converted to camelCase for TS
- `get_help()` — JSDoc comment
- `is_required_set()` — optional vs required field
- `get_action()` — `SetTrue` means boolean, `Set` means string
- `get_possible_values()` — string literal union for enums
- `get_default_values()` — noted in JSDoc

The generator emits `libs/barnum/barnum-cli.ts` with:

1. A `GlobalOptions` interface for `root` and `logLevel`
2. An interface per terminal command (`RunOptions`, `ConfigDocsOptions`, `ConfigValidateOptions`, `ConfigGraphOptions`, `ConfigSchemaOptions`, `VersionOptions`)
3. A discriminated union `BarnumCommand` covering all subcommands
4. A spawn helper per terminal command (`barnumRun`, `barnumConfigDocs`, etc.)
5. A top-level `barnum()` function accepting the discriminated union

Special case: `RunOptions.config` accepts `string | ConfigFile`. The generator applies this override based on a hardcoded mapping (the `config` arg on the `run` subcommand gets widened to also accept the config object type). When `config` is an object, the spawn function calls `JSON.stringify` before passing it as a CLI arg.

Generated output (sketch):

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
  /** Root directory. Pools live in `<root>/pools/<id>/`. Defaults to `/tmp/troupe` on Unix. */
  root?: string;
  /** Log level (debug shows task return values). */
  logLevel?: "off" | "error" | "warn" | "info" | "debug" | "trace";
}

export interface RunOptions extends GlobalOptions {
  /** Config (JSON string or path to file). Required unless resumeFrom is set. */
  config?: string | ConfigFile;
  /** Initial tasks (JSON string or path to file). */
  initialState?: string;
  /** Initial value for the entrypoint step (JSON string or path to file). */
  entrypointValue?: string;
  /** Agent pool ID. Defaults to "default". */
  pool?: string;
  /** Wake script to call before starting. */
  wake?: string;
  /** Log file path. */
  logFile?: string;
  /** State log file path (NDJSON for persistence/resume). */
  stateLog?: string;
  /** Resume from a previous state log file. */
  resumeFrom?: string;
}

export interface ConfigDocsOptions extends GlobalOptions {
  /** Config (JSON string or path to file). */
  config: string | ConfigFile;
}

export interface ConfigValidateOptions extends GlobalOptions {
  /** Config (JSON string or path to file). */
  config: string | ConfigFile;
}

export interface ConfigGraphOptions extends GlobalOptions {
  /** Config (JSON string or path to file). */
  config: string | ConfigFile;
}

export interface ConfigSchemaOptions extends GlobalOptions {
  /** Output format. */
  type?: "zod" | "json";
}

export interface VersionOptions extends GlobalOptions {
  /** Output as JSON. */
  json?: boolean;
}

export type BarnumCommand =
  | { command: "run" } & RunOptions
  | { command: "config docs" } & ConfigDocsOptions
  | { command: "config validate" } & ConfigValidateOptions
  | { command: "config graph" } & ConfigGraphOptions
  | { command: "config schema" } & ConfigSchemaOptions
  | { command: "version" } & VersionOptions;

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
  if (options.entrypointValue) { args.push("--entrypoint-value", options.entrypointValue); }
  if (options.pool) { args.push("--pool", options.pool); }
  if (options.wake) { args.push("--wake", options.wake); }
  if (options.logFile) { args.push("--log-file", options.logFile); }
  if (options.stateLog) { args.push("--state-log", options.stateLog); }
  if (options.resumeFrom) { args.push("--resume-from", options.resumeFrom); }
  return spawnBarnum(args);
}

export function barnumConfigDocs(options: ConfigDocsOptions): ChildProcess { /* ... */ }
export function barnumConfigValidate(options: ConfigValidateOptions): ChildProcess { /* ... */ }
export function barnumConfigGraph(options: ConfigGraphOptions): ChildProcess { /* ... */ }
export function barnumConfigSchema(options: ConfigSchemaOptions): ChildProcess { /* ... */ }
export function barnumVersion(options: VersionOptions): ChildProcess { /* ... */ }

export function barnum(options: BarnumCommand): ChildProcess {
  switch (options.command) {
    case "run": return barnumRun(options);
    case "config docs": return barnumConfigDocs(options);
    // ...
  }
}
```

All functions return `ChildProcess` for full caller control (pipe stdio, signals, detach).

### Task 3: Regeneration infrastructure

Add `libs/barnum/barnum-cli.ts` to:

- `CLAUDE.md` generated artifacts list, with the regeneration command
- Pre-commit hook (same pattern as the existing schema generation)
- CI verification (same pattern: regenerate and diff)
- `package.json` `files` array

### Task 4: Update barrel

**File:** `libs/barnum/index.ts`

Add re-exports from `barnum-cli.ts`:

```typescript
export * from "./barnum-config-schema.zod.js";
export * from "./barnum-cli.js";
```
