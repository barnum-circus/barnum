# TypeScript Run API

**Status:** Pending

## Motivation

Users who write TypeScript configs with `defineConfig` should also be able to invoke barnum programmatically from TypeScript. The current package exports `defineConfig` directly from the generated Zod file (`barnum-config-schema.zod.ts`), with no barrel file aggregating exports. There's also no way to call `barnum run` from TypeScript without manually resolving the binary path and constructing CLI args.

Two changes:

1. Add a barrel file (`index.ts`) that re-exports schema/types from the Zod file. `defineConfig` should be importable from the package root via a proper barrel, not directly from a generated file with an unwieldy name.

2. Export a typed `run()` function that spawns the barnum binary with the correct arguments. The parameter type mirrors the Rust `Command::Run` variant and the global `Cli` fields (`crates/barnum_cli/src/main.rs:40-96`), so callers get type safety over the CLI interface.

## Current State

### Package exports (`libs/barnum/package.json:7-11`)

```json
"exports": {
  ".": "./barnum-config-schema.zod.ts",
  "./schema": "./barnum-config-schema.zod.ts",
  "./binary": "./index.js"
}
```

The root export points directly at the generated Zod file. There is no `index.ts`.

### Binary resolution (`libs/barnum/index.js`)

`index.js` is a CJS file that resolves the platform-specific binary path and exports it as a string. `cli.js` uses the same platform-detection logic independently (duplicated) and spawns the binary with `process.argv.slice(2)`.

### CLI opts (`crates/barnum_cli/src/main.rs:40-96`)

The `Cli` struct has two global fields (`root`, `log_level`). The `Command::Run` variant has: `config`, `initial_state`, `entrypoint_value`, `pool`, `wake`, `log_file`, `state_log`, `resume_from`.

## Proposed Changes

### Task 1: Create `index.ts` barrel file

**File:** `libs/barnum/index.ts` (new)

```typescript
// Re-export everything from the generated schema
export {
  configFileSchema,
  defineConfig,
  type ConfigFile,
  type MaybeLinked_for_String,
  type ActionFile,
  type FinallyHook,
  type Options,
  type PostHook,
  type PreHook,
  type SchemaRef,
  type StepOptions,
  type StepFile,
} from "./barnum-config-schema.zod.js";

export { run, type RunOptions } from "./run.js";
```

### Task 2: Create `run.ts` with typed run function

**File:** `libs/barnum/run.ts` (new)

The `RunOptions` type mirrors the Rust `Cli` + `Command::Run` fields from `crates/barnum_cli/src/main.rs:40-96`:

```typescript
import { spawn, type ChildProcess } from "node:child_process";
import { chmodSync } from "node:fs";
import { join } from "node:path";

export interface RunOptions {
  /** Config: JSON string or path to a file. Required unless resumeFrom is set. */
  config?: string;
  /** Initial tasks: JSON string or path to file. Required if config has no entrypoint. */
  initialState?: string;
  /** Initial value for the entrypoint step: JSON string or path to file. */
  entrypointValue?: string;
  /** Agent pool ID (defaults to "default"). */
  pool?: string;
  /** Wake script to call before starting. */
  wake?: string;
  /** Log file path. */
  logFile?: string;
  /** State log file path (NDJSON for persistence/resume). */
  stateLog?: string;
  /** Resume from a previous state log file. Incompatible with config/initialState/entrypointValue. */
  resumeFrom?: string;
  /** Root directory (defaults to /tmp/troupe on Unix). */
  root?: string;
  /** Log level. */
  logLevel?: "debug" | "info" | "warn" | "error";
}

function resolveBinary(): string {
  const platform = process.platform;
  const arch = process.arch;

  const key =
    platform === "darwin" && arch === "x64"    ? "macos-x64" :
    platform === "darwin" && arch === "arm64"   ? "macos-arm64" :
    platform === "linux"  && arch === "x64"     ? "linux-x64" :
    platform === "linux"  && arch === "arm64"   ? "linux-arm64" :
    platform === "win32"  && arch === "x64"     ? "win-x64" :
    null;

  if (!key) {
    throw new Error(`Platform "${platform} (${arch})" not supported.`);
  }

  const ext = platform === "win32" ? ".exe" : "";
  return join(__dirname, "artifacts", key, `barnum${ext}`);
}

export function run(options: RunOptions): ChildProcess {
  const bin = resolveBinary();
  try { chmodSync(bin, 0o755); } catch {}

  const args: string[] = ["run"];

  if (options.root)            { args.push("--root", options.root); }
  if (options.logLevel)        { args.push("--log-level", options.logLevel); }
  if (options.config)          { args.push("--config", options.config); }
  if (options.initialState)    { args.push("--initial-state", options.initialState); }
  if (options.entrypointValue) { args.push("--entrypoint-value", options.entrypointValue); }
  if (options.pool)            { args.push("--pool", options.pool); }
  if (options.wake)            { args.push("--wake", options.wake); }
  if (options.logFile)         { args.push("--log-file", options.logFile); }
  if (options.stateLog)        { args.push("--state-log", options.stateLog); }
  if (options.resumeFrom)      { args.push("--resume-from", options.resumeFrom); }

  return spawn(bin, args, { stdio: "inherit" });
}
```

The function returns a `ChildProcess` rather than a `Promise`. Callers who want a promise can listen on `"exit"`. Returning the raw process gives full control: pipe stdio, send signals, detach, etc.

**Note:** Global flags (`--root`, `--log-level`) go before the subcommand in clap's parsing, but clap also accepts them after subcommands when `global = true` is set (which both fields have at `main.rs:46,50`). Placing them before `run` in the args array is the canonical order.

Wait, actually: clap with `global = true` accepts global args in any position, but the conventional placement is before the subcommand. Let me re-check. Looking at `main.rs:46`: `#[arg(long, global = true)]` on `root`, and `main.rs:50`: `#[arg(short, long, global = true, default_value = "info")]` on `log_level`. With `global = true`, clap accepts these args at any position in the command line. I'll place them before `run` for convention, but after works too.

Actually, re-reading the code: the args are built as `["run", "--root", ..., "--config", ...]` which puts global args after the subcommand. That's fine with `global = true`.

### Task 3: Update package.json

**File:** `libs/barnum/package.json`

```json
{
  "main": "index.ts",
  "types": "index.ts",
  "exports": {
    ".": "./index.ts",
    "./schema": "./barnum-config-schema.zod.ts",
    "./binary": "./index.js"
  },
  "files": [
    "index.ts",
    "index.js",
    "run.ts",
    "cli.js",
    "artifacts/**/*",
    "barnum-config-schema.json",
    "barnum-config-schema.zod.ts"
  ]
}
```

Root export changes from the generated Zod file to the barrel. `./schema` stays as a direct path to the Zod file for consumers who only want types without the run dependency.

### Task 4: Deduplicate binary resolution

`cli.js` and `index.js` both contain the same platform-detection switch. The new `run.ts` adds a third copy. `cli.js` and `index.js` are CJS and must stay as plain `.js` (they're the bin entry and the `./binary` export). `run.ts` is TypeScript.

Two options:
- **A)** Have `run.ts` import from `./index.js` (the existing CJS binary resolver) instead of duplicating the logic. This works since `index.js` just exports a string path.
- **B)** Keep the duplication. Three files, same switch statement, low churn rate.

Option A is cleaner:

```typescript
// run.ts — use existing binary resolver
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const bin: string = require("./index.js");
```

This avoids a third copy of the platform switch. `cli.js` still has its own copy because it's a shebang entry point that can't share imports cleanly with ESM, but that's acceptable for a two-line npm bin wrapper.

## Open Questions

1. **Should `run()` accept a `ConfigFile` object directly?** The Rust CLI accepts a JSON string or file path for `--config`. The TypeScript `run()` could accept a `ConfigFile` object, serialize it to a temp file or inline JSON, and pass that. This would close the loop with `defineConfig`: `run({ config: JSON.stringify(defineConfig({...})) })`. But the user can do this themselves, and auto-temp-files add cleanup concerns. Leaning toward keeping `config` as `string` and letting callers stringify.

2. **Should `run.ts` use ESM or CJS?** The package currently ships `.ts` files directly (no build step). The barrel `index.ts` uses ESM import syntax. This assumes consumers have a TypeScript-aware bundler or runtime (tsx, vite, next, etc.). This matches the existing pattern where `barnum-config-schema.zod.ts` is shipped as raw `.ts`.

3. **Keeping `RunOptions` in sync with Rust.** The type is hand-written to match the Rust CLI opts. If the CLI adds or removes flags, `RunOptions` must be updated manually. An alternative: generate `RunOptions` from the clap struct (clap can emit JSON schema via `clap_complete`, but the tooling for TS generation from clap schemas is immature). Hand-written is fine for now given the small surface area.
