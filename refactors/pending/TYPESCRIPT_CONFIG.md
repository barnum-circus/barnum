# TypeScript Config API

**Status:** Phase 1 — Architecture document

## Motivation

Barnum configs are JSON/JSONC files. The JSON Schema provides editor validation, but users writing configs programmatically (generating steps in a loop, computing values, sharing step definitions across workflows) have no type safety. The `r1-3/config.jsonc` workflow already has 7 steps with inline bash scripts that would benefit from TypeScript's editor completion and compile-time checking.

This refactor adds TypeScript types for the config format and teaches `barnum run` to accept a `.ts` file as the config source.

## Current State

### Config types

`crates/barnum_config/src/config.rs` defines `ConfigFile` as the top-level serde struct. Each field derives `schemars::JsonSchema` for automatic JSON Schema generation.

### JSON Schema generation

`crates/barnum_config/src/bin/build_barnum_schema.rs` calls `config_schema()` (which calls `schemars::schema_for!(ConfigFile)`), serializes the result to JSON, and writes it to `libs/barnum/barnum-config-schema.json`. This file is checked in and verified by CI.

### npm package

`libs/barnum/package.json` publishes `@barnum/barnum` (v0.2.4) with:
- `index.js` — resolves the platform-specific binary path
- `cli.js` — shebang wrapper that spawns the binary
- `barnum-config-schema.json` — JSON Schema
- `artifacts/` — pre-built binaries

The package has no TypeScript type definitions and no `.d.ts` files.

### CLI

`crates/barnum_cli/src/main.rs` uses clap. The `Run` command accepts `--config <json-or-file>`. Config is parsed with `json5::from_str` (supports JSONC). The `--config` flag is `required_unless_present = "resume_from"`.

### Invoker discovery

`crates/cli_invoker/src/lib.rs` implements a multi-level detection chain for CLI tools: env var, cargo workspace binary, `node_modules/.bin`, `package.json` `packageManager` field, global package manager in PATH. This pattern is reusable for discovering a TypeScript runtime.

## Proposed Changes

### 1. Generate TypeScript types from the JSON Schema

The build pipeline becomes:

```
Rust types (schemars) --> barnum-config-schema.json --> types.d.ts
```

A Node script in `libs/barnum/` converts the JSON Schema to TypeScript definitions using `json-schema-to-typescript` (established npm package, 3k+ GitHub stars, handles JSON Schema draft-07). The script runs after `build_barnum_schema` regenerates the JSON Schema.

```bash
# In libs/barnum/
node build-types.js
```

The script reads `barnum-config-schema.json`, produces `types.d.ts`, and appends a `defineConfig` helper signature:

```typescript
// Generated from barnum-config-schema.json — do not edit manually.

export interface ConfigFile {
  entrypoint?: string;
  options?: Options;
  steps: StepFile[];
}

export interface Options {
  timeout?: number;
  max_retries?: number;
  max_concurrency?: number;
  retry_on_timeout?: boolean;
  retry_on_invalid_response?: boolean;
}

export interface StepFile {
  name: string;
  value_schema?: unknown;
  pre?: PreHook;
  action: ActionFile;
  post?: PostHook;
  next?: string[];
  finally?: FinallyHook;
  options?: StepOptions;
}

// ... (ActionFile, PreHook, PostHook, FinallyHook, StepOptions, etc.)

/**
 * Identity function that provides type inference for config objects.
 * Returns its argument unchanged.
 */
export declare function defineConfig(config: ConfigFile): ConfigFile;
```

The exact output depends on `json-schema-to-typescript`'s mapping of the schema. The above is illustrative. The generated file is checked in alongside the schema and verified by CI (same diff-check pattern).

**Build command sequence:**
```bash
cargo run -p barnum_config --bin build_barnum_schema   # Rust -> JSON Schema
cd libs/barnum && node build-types.js                   # JSON Schema -> types.d.ts
```

### 2. Export types from the npm package

New files in `libs/barnum/`:

**`types.d.ts`** — generated TypeScript definitions (all config types).

**`index.d.ts`** — re-exports from `types.d.ts`:
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

**`defineConfig.js`** — runtime identity function:
```javascript
'use strict';
module.exports.defineConfig = function defineConfig(config) {
  return config;
};
```

`defineConfig` exists for ergonomics. It provides type inference at the call site without requiring a separate type annotation:

```typescript
import { defineConfig } from "@barnum/barnum";

export default defineConfig({
  entrypoint: "Analyze",
  steps: [
    { name: "Analyze", action: { kind: "Pool", instructions: { inline: "..." } }, next: ["Done"] },
    { name: "Done", action: { kind: "Command", script: "echo '[]'" }, next: [] },
  ],
});
```

Users who prefer not to install the package (using `pnpm dlx` to run barnum) can skip `defineConfig` and use a type-only import instead:

```typescript
import type { ConfigFile } from "@barnum/barnum";

const config: ConfigFile = {
  steps: [/* ... */],
};
export default config;
```

Type-only imports are erased by every TS runtime (tsx, bun, tsc), so the file executes without `@barnum/barnum` in `node_modules`. The types are only needed for editor completion and `tsc` type-checking.

**package.json changes:**
```json
{
  "types": "index.d.ts",
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

### 3. `barnum run --ts`

Add a `--ts` flag to the `Run` clap struct, mutually exclusive with `--config`:

```rust
// crates/barnum_cli/src/main.rs

#[derive(Subcommand)]
enum Command {
    Run {
        /// Config (JSON string or path to file). Mutually exclusive with --ts.
        #[arg(long, group = "config_source", required_unless_present_any = ["ts", "resume_from"])]
        config: Option<String>,

        /// TypeScript config file. The file's default export must be a ConfigFile object.
        /// Mutually exclusive with --config.
        #[arg(long, group = "config_source")]
        ts: Option<PathBuf>,

        /// Override the TypeScript runtime (e.g., "bun", "tsx", "node").
        /// Only valid with --ts. Default: auto-detected.
        #[arg(long, requires = "ts")]
        ts_runner: Option<String>,

        // ... existing fields unchanged
    },
}
```

#### Execution flow when `--ts` is provided

1. Detect a TS runner (see discovery below), or use `--ts-runner` if provided.
2. Resolve the TS file to an absolute path.
3. Build an eval script that dynamic-imports the file and serializes its default export:
   ```javascript
   import('/absolute/path/to/workflow.ts')
     .then(m => process.stdout.write(JSON.stringify(m.default)))
     .catch(e => { process.stderr.write(String(e)); process.exit(1); })
   ```
4. Execute: `<runner> -e "<eval script>"` and capture stdout.
5. Parse stdout as `ConfigFile` via `json5::from_str` (same as the existing config path).
6. Continue with config validation, resolution, schema compilation, and `run()`.

The eval script itself is plain JavaScript (no TS syntax) since it only calls `import()` and `JSON.stringify`. The user's `.ts` file is what requires the TS runtime, and the dynamic import handles that.

The `config_dir` for resolving relative paths (linked instruction files, schemas) is the parent directory of the TS file, same as for JSON config files.

#### TS runner discovery

When `--ts-runner` is not provided, barnum searches for a TS-capable runtime in this order:

1. **`BARNUM_TS_RUNNER` env var** — explicit override (e.g., `BARNUM_TS_RUNNER=bun`).
2. **`tsx` in `node_modules/.bin/`** — walk up from CWD looking for a project-local install.
3. **`tsx` in PATH** — global tsx installation.
4. **`bun` in PATH** — Bun executes TypeScript natively.
5. **`node`** with `--experimental-strip-types` — Node 22.6+ has built-in TS support (limited: no enums, no namespaces). Barnum checks `node --version` to verify 22.6+.

If nothing is found, barnum exits with an error listing the supported runtimes and how to install one.

The eval invocation varies by runner:

| Runner | Invocation |
|--------|-----------|
| `tsx` | `tsx -e "<script>"` |
| `bun` | `bun -e "<script>"` |
| `node` (22.6+) | `node --experimental-strip-types -e "<script>"` |

Since the eval script is plain JS, all three invocations use the same `-e` flag. The `--experimental-strip-types` flag on node is needed for the user's TS file (imported dynamically), not for the eval script itself.

#### Implementation location

The TS runner detection logic lives in a new module `crates/barnum_cli/src/ts_runner.rs` (not in `cli_invoker`, since this is CLI-specific, not a reusable invoker pattern). The module exports a single function:

```rust
/// Detect a TypeScript runtime, or return an error with installation instructions.
pub fn detect_ts_runner(override_runner: Option<&str>) -> io::Result<TsRunner> { ... }

pub struct TsRunner {
    command: String,
    args: Vec<String>,  // e.g., ["--experimental-strip-types"] for node 22.6+
}

impl TsRunner {
    /// Execute a TS file's default export and return it as a JSON string.
    pub fn eval_default_export(&self, ts_file: &Path) -> io::Result<String> { ... }
}
```

The `main.rs` changes are minimal. In the `(None, Some(ts_file))` match arm of the `Run` command:

```rust
(None, None, Some(ts_file)) => {
    let runner = ts_runner::detect_ts_runner(ts_runner_override.as_deref())?;
    let json = runner.eval_default_export(&ts_file)?;
    // Parse, validate, resolve, run — same as the --config path
    let config_dir = ts_file.canonicalize()?.parent().unwrap().to_path_buf();
    let cfg_file: ConfigFile = json5::from_str(&json).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData,
            format!("[E074] TS config produced invalid JSON: {e}"))
    })?;
    // ... validate, resolve, compile schemas, run
}
```

### 4. User workflow

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
      action: { kind: "Pool", instructions: { link: "analyze.md" } },
      next: ["Implement"],
    },
    {
      name: "Implement",
      action: { kind: "Pool", instructions: { link: "implement.md" } },
      next: [],
    },
  ],
});
```

Run it:
```bash
barnum run --ts workflow.ts --pool default
```

Or with `pnpm dlx` (no local install of barnum needed, but tsx must be available):
```bash
pnpm dlx @barnum/barnum run --ts workflow.ts
```

## Open Questions

1. **Should `build-types.js` be a dev dependency or a checked-in script?** It only runs during the build. If it's a script, we vendor `json-schema-to-typescript` or shell out to `npx`. If it's a dev dependency, `libs/barnum/` needs a `package-lock.json` or similar.

2. **Should the generated types use `interface` or `type`?** `json-schema-to-typescript` defaults to interfaces. This is fine for most cases, but union types (like `ActionFile` which is `Pool | Command`) may need adjustment. We should verify the output and hand-edit if the generated types are awkward.

3. **What happens when the TS file's default export is not a valid ConfigFile?** The current plan parses the JSON and lets `json5::from_str` fail with a deserialization error. The error message should indicate that the TS file's export didn't match the expected config shape, not just "invalid JSON field X."
