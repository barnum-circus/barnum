# Executor CLI Flag

**Parent:** JS_ACTION_RESOLUTION.md
**Depends on:** JS_ACTION_HANDLERS.md (executor.ts must exist)

## Motivation

Rust needs to know what command to spawn for each task. The Rust binary can't reliably resolve tsx from the npm package layout (pnpm symlinks, yarn PnP, dlx temp directories). JS can — `require.resolve("tsx/cli")` works across all package managers and install modes.

This sub-refactor adds a hidden `--executor` flag as the internal communication channel from JS to Rust. `cli.cjs` detects the JS runtime, resolves the executor command, and injects `--executor` when spawning the Rust binary. Users never interact with this flag.

## Changes

### 1. cli.cjs: runtime detection and --executor injection

**File:** `libs/barnum/cli.cjs`

`cli.cjs` is the single entry point to the Rust binary. It detects the JS runtime, resolves the executor command, and injects `--executor` before forwarding the user's args to the Rust binary.

```javascript
const { resolve } = require("path");
const { createRequire } = require("module");

const executorPath = resolve(__dirname, "actions", "executor.ts");

function resolveExecutorCommand() {
  if (typeof Bun !== "undefined") {
    // Bun runs .ts natively
    return `${process.execPath} ${executorPath}`;
  }
  // Node: use tsx
  const tsxPath = require.resolve("tsx/cli");
  return `node ${tsxPath} ${executorPath}`;
}

// Find the Rust binary
const binaryPath = process.env.BARNUM || require("./index.cjs");

// Inject --executor, then forward all user args
const executor = resolveExecutorCommand();
const args = [...process.argv.slice(2), "--executor", executor];

const { execFileSync } = require("child_process");
// ... spawn binaryPath with args
```

tsx is a declared dependency of `@barnum/barnum`, so `require.resolve("tsx/cli")` works in all install modes: npm, pnpm (including dlx), yarn (including PnP), global installs.

### 2. BarnumConfig.run() calls cli.cjs

**File:** `libs/barnum/run.ts`

`BarnumConfig.run()` stops calling the Rust binary directly. Instead it spawns `cli.cjs`, which handles executor resolution.

```typescript
export class BarnumConfig {
  run(opts?: RunOptions): ChildProcess {
    const cliPath = resolve(import.meta.dirname, "cli.cjs");
    const args = opts?.resumeFrom
      ? ["run", "--resume-from", opts.resumeFrom]
      : ["run", "--config", JSON.stringify(this.config)];
    // ... rest of opts
    return spawn(process.execPath, [cliPath, ...args], { stdio: "inherit" });
  }
}
```

### 3. Add --executor to Rust CLI (hidden, required)

**File:** `crates/barnum_cli/src/lib.rs`

```rust
Run {
    #[arg(long, required_unless_present = "resume_from")]
    config: Option<String>,

    // ... existing fields ...

    /// Internal: executor command injected by cli.cjs.
    /// Not user-facing — hidden from --help.
    #[arg(long, hide = true)]
    executor: String,

    #[arg(long, conflicts_with = "config")]
    resume_from: Option<PathBuf>,
}
```

The flag is required (no `Option`). If the Rust binary is invoked without it (i.e., bypassing `cli.cjs`), clap errors with a missing argument. This enforces that all invocations go through the JS wrapper.

### 4. Add executor_script to RunnerConfig and Engine

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub invoker: &'a Invoker<TroupeCli>,
    pub executor_script: &'a str,
    pub state_log_path: &'a Path,
}
```

Engine stores `executor_script: String` and pre-serializes config JSON once at construction:

```rust
struct Engine<'a> {
    // ... existing fields ...
    executor_script: String,
    config_json: serde_json::Value,  // pre-serialized, reused per dispatch
}
```

### 5. dispatch_task uses executor_script

**File:** `crates/barnum_config/src/runner/mod.rs`

`dispatch_task` uses `self.executor_script` as the ShellAction script. The enriched envelope format (`{ action, task, step, config }`) is defined in the parent doc — this sub-refactor just wires `executor_script` into the dispatch path.

## Testing

- All existing tests need `--executor` injected. Test helpers (`common/mod.rs`) should provide a mock executor (e.g., a script that reads stdin and echoes `[]`).
- Integration test: full round-trip with the real JS executor, verifying envelope format and handler dispatch.
- cli.cjs test: verify runtime detection produces correct executor command for Node (with tsx) and Bun (without tsx).

## What This Removes

Once this lands, the following become dead code (deleted in a follow-up):
- `PoolAction` struct in `runner/action.rs`
- `submit.rs` (troupe submission)
- `Invoker<TroupeCli>` from `Engine` and `RunnerConfig`
- `ActionKind` enum and associated structs in `resolved.rs`
- `generate_step_docs` in `docs.rs`
