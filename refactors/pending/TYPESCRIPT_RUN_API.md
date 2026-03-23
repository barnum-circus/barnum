# TypeScript CLI API

**Status:** Pending (Task 1 done)

## Motivation

The npm package should export typed functions for invoking the barnum binary. Types are generated from Rust CLI structs via schemars, using the same `emit_zod` function that generates the config schema. When CLI args change, regenerate and CI catches drift.

## Current State

`emit_zod` (`crates/barnum_config/src/zod.rs:18`) walks a schemars `RootSchema` and emits Zod schemas + TypeScript types. It hardcodes three config-specific things: the root export name (`configFileSchema`), the root type name (`ConfigFile`), and the `defineConfig` helper function.

CLI types (`Cli`, `Command`, `ConfigCommand`, `LogLevel`, `SchemaType`) live in `crates/barnum_cli/src/main.rs:20-149`. Private, no `JsonSchema` or `Serialize` derives.

## Proposed Changes

### Task 1: Barrel file

Done.

### Task 2: Extract `emit_zod` into its own crate

**New crate:** `crates/schemars_emit`

Move `zod.rs` from `barnum_config` into this crate. Parameterize the root export name:

```rust
pub fn emit_zod(root: &RootSchema, root_name: &str) -> String
```

Move the `defineConfig` helper out of `emit_zod` — it's config-specific. The config binary appends it after calling `emit_zod`.

Update call sites:
- `build_barnum_schema.rs`: `schemars_emit::emit_zod(&root, "configFile")` + append `defineConfig`
- `build_cli_schema.rs`: `schemars_emit::emit_zod(&root, "cli")`

Both `barnum_config` and `barnum_cli` depend on `schemars_emit`.

### Task 3: Extract CLI types into `barnum_cli`'s library target

Add `src/lib.rs` to `barnum_cli`. Move type definitions there. `main.rs` imports with `use barnum_cli::*`.

Add `Serialize` and `JsonSchema` derives. Add `#[serde(tag = "kind")]` to `Command` and `ConfigCommand`. Add `#[serde(rename_all = "camelCase")]` on struct variants with multi-word fields. Add `#[serde(rename_all = "lowercase")]` on `LogLevel` and `SchemaType`.

### Task 4: CLI schema generation binary

**File:** `crates/barnum_cli/src/bin/build_cli_schema.rs` (new)

```rust
use barnum_cli::Cli;
use schemars_emit::emit_zod;

fn main() {
    let root = schemars::schema_for!(Cli);
    let zod = emit_zod(&root, "cli");
    // write to libs/barnum/barnum-cli-schema.zod.ts
}
```

Output: `libs/barnum/barnum-cli-schema.zod.ts` — same format as the config schema, with `export const cliSchema`, `export type Cli`, etc.

### Task 5: Hand-written spawn functions

**File:** `libs/barnum/run.ts` (new)

Imports the generated types from `barnum-cli-schema.zod.ts` and provides spawn functions.

```typescript
import { spawn, type ChildProcess } from "node:child_process";
import { chmodSync } from "node:fs";
import { createRequire } from "node:module";
import type { Cli, Command, ConfigCommand } from "./barnum-cli-schema.zod.js";

const require = createRequire(import.meta.url);
const binaryPath: string = require("./index.js");

function spawnBarnum(args: string[]): ChildProcess {
  try { chmodSync(binaryPath, 0o755); } catch {}
  return spawn(binaryPath, args, { stdio: "inherit" });
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
  opts: Omit<Extract<Command, { kind: "Run" }>, "kind">,
  global?: { root?: string; logLevel?: string },
): ChildProcess {
  return barnum({ ...global, command: { kind: "Run", ...opts } });
}
```

### Task 6: Update barrel and package.json

**File:** `libs/barnum/index.ts`

```typescript
export * from "./barnum-config-schema.zod.js";
export * from "./barnum-cli-schema.zod.js";
export { barnum, barnumRun } from "./run.js";
```

Add `barnum-cli-schema.zod.ts` and `run.ts` to `files` in `package.json`.

### Task 7: CI verification + pre-commit hook

Add to CI:

```bash
cargo run -p barnum_cli --bin build_cli_schema
git diff --exit-code libs/barnum/barnum-cli-schema.zod.ts
```

Extend pre-commit hook to regenerate both files.

Add to `CLAUDE.md` generated artifacts:

```
- `libs/barnum/barnum-cli-schema.zod.ts` — regenerate with `cargo run -p barnum_cli --bin build_cli_schema`
```
