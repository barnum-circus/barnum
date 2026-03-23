# JS Action Resolution

## Motivation

The Engine's `dispatch_task` matches on `ActionKind` variants to construct the appropriate `Action` impl. Every new action kind (Claude, Git, custom user kinds) requires adding a Rust enum variant, a match arm, and a new `Action` impl. This coupling is unnecessary — both `Pool` and `Command` ultimately just "run a subprocess, pipe stdin, read stdout."

The JS layer (`BarnumConfig`) already validates config, constructs CLI args, and spawns the Rust binary. This refactor extends JS to also resolve each step's action kind into a concrete execution command. Rust receives a config where every step has a `{ command, args }` descriptor. Rust spawns the command, manages the state machine, handles timeouts. It never needs to know what "Pool" or "Command" means.

This supersedes ACTION_REGISTRY.md. Instead of building a Rust-side factory registry (which would be deleted when JS takes over), we go directly to JS resolution.

## Architecture

```
User config (kind: "Pool", kind: "Command", kind: "Claude")
  → JS resolution (each kind resolved to a { command, args })
    → Resolved config (every step has a command + args)
      → Rust (spawns commands, manages state tree, handles timeouts)
```

## Current State

### dispatch_task (`crates/barnum_config/src/runner/mod.rs:696-730`)

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    let tx = self.tx.clone();

    match &step.action {
        ActionKind::Pool(PoolActionConfig {
            pool,
            root,
            timeout: pool_timeout,
            ..
        }) => {
            let docs = generate_step_docs(step, self.config);
            info!(step = %task.step, "submitting task to pool");
            let action = Box::new(PoolAction {
                root: root.clone(),
                pool: pool.clone(),
                invoker: self.invoker.clone(),
                docs,
                step_name: task.step.clone(),
                pool_timeout: *pool_timeout,
            });
            spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
        }
        ActionKind::Command(CommandAction { script }) => {
            info!(step = %task.step, script = %script, "executing command");
            let action = Box::new(ShellAction {
                script: script.clone(),
                step_name: task.step.clone(),
                working_dir: self.working_dir.clone(),
            });
            spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
        }
    }
}
```

### Engine fields (`mod.rs:454-465`)

```rust
struct Engine<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    invoker: Invoker<TroupeCli>,
    working_dir: PathBuf,
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
}
```

### RunnerConfig (`mod.rs:39-48`)

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub invoker: &'a Invoker<TroupeCli>,
    pub state_log_path: &'a Path,
}
```

### Resolved types (`resolved.rs`)

```rust
pub struct PoolAction {
    pub instructions: String,
    pub pool: Option<String>,
    pub root: Option<PathBuf>,
    pub timeout: Option<u64>,
}

pub struct CommandAction {
    pub script: String,
}

#[serde(tag = "kind")]
pub enum ActionKind {
    Pool(PoolAction),
    Command(CommandAction),
}
```

### JS layer (`libs/barnum/run.ts`)

`BarnumConfig.run()` serializes the user config as JSON and passes it to `barnum run --config`. No action resolution happens in JS today.

### What gets deleted

- `PoolAction` struct in `runner/action.rs` (the runtime Action impl)
- `submit.rs` (troupe submission logic — `build_agent_payload`, `submit_via_cli`)
- `ActionKind` enum in `resolved.rs`
- `PoolAction` and `CommandAction` structs in `resolved.rs`
- `Invoker<TroupeCli>` from `Engine` and `RunnerConfig`
- `cli_invoker` and `troupe_cli` dependencies from `barnum_config`
- `generate_step_docs` in `docs.rs` (moves to JS; `generate_full_docs` stays for `barnum config docs`)
- `Config::has_pool_actions()` in `resolved.rs`

## Proposed Changes

### 1. New resolved type: `ActionExecution`

**File:** `crates/barnum_config/src/resolved.rs`

Replace `ActionKind` with a flat execution descriptor:

```rust
/// Pre-resolved action: command + args to spawn at dispatch time.
/// JS resolves action kinds into this at config time.
#[derive(Debug, Serialize, Deserialize)]
pub struct ActionExecution {
    /// Command to spawn (e.g., "node", "sh").
    pub command: String,
    /// Arguments to the command.
    pub args: Vec<String>,
}
```

The `Step` struct changes:

```rust
pub struct Step {
    pub name: StepName,
    pub value_schema: Option<serde_json::Value>,
    pub action: ActionExecution,  // was: ActionKind
    pub next: Vec<StepName>,
    pub finally_hook: Option<HookScript>,
    pub options: Options,
}
```

### 2. dispatch_task becomes trivial

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    info!(step = %task.step, command = %step.action.command, "dispatching task");
    let action = Box::new(ShellAction {
        command: step.action.command.clone(),
        args: step.action.args.clone(),
        step_name: task.step.clone(),
        working_dir: self.working_dir.clone(),
    });
    spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
}
```

### 3. ShellAction changes

**File:** `crates/barnum_config/src/runner/action.rs`

Currently ShellAction wraps everything in `sh -c`. It changes to spawn commands directly:

```rust
pub struct ShellAction {
    pub command: String,
    pub args: Vec<String>,
    pub step_name: StepName,
    pub working_dir: PathBuf,
}

impl Action for ShellAction {
    fn start(self: Box<Self>, value: serde_json::Value) -> ActionHandle {
        // ...
        let child = Command::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(&self.working_dir)
            .spawn();
        // rest unchanged: pipe stdin, read stdout/stderr, ProcessGuard
    }
}
```

### 4. Engine simplification

**File:** `crates/barnum_config/src/runner/mod.rs`

Engine drops `invoker`:

```rust
struct Engine<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    working_dir: PathBuf,
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
}
```

`RunnerConfig` drops `invoker`:

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub state_log_path: &'a Path,
}
```

### 5. Config resolution path splits

Currently `ConfigFile::resolve()` resolves `ActionFile` → `ActionKind`. With JS resolution, there are two paths:

**Path A: JS-driven (primary).** JS resolves actions into `ActionExecution` before passing config to Rust. The config Rust receives already has `{ command, args }` per step. `ConfigFile::resolve()` no longer needs to resolve actions — it just passes through the execution spec.

**Path B: CLI-only (fallback).** When using `barnum run --config` directly (without the JS layer), the Rust CLI needs to resolve actions itself. This is the escape hatch for users who don't want to use the npm package. The CLI would have hardcoded resolution for `Pool` and `Command` — similar to today but happening in the CLI binary, not in `barnum_config`.

This means `ActionFile` stays as the user-facing config type (with `Pool`, `Command` variants), and the resolved type is `ActionExecution`. The resolution logic lives in either JS (primary path) or the CLI binary (fallback path), not in `barnum_config`.

### 6. JS action modules

**File:** `libs/barnum/actions/pool.ts`

Each action kind is a function that takes action params + context and returns `{ command, args }`.

```typescript
interface ActionContext {
  workingDir: string;
  config: ResolvedConfig;
  step: ResolvedStep;
}

interface ExecutionSpec {
  command: string;
  args: string[];
}
```

**Pool resolver:**

```typescript
// libs/barnum/actions/pool.ts
const executorPath = resolve(import.meta.dirname, "pool-executor.js");

export function resolvePool(
  params: { instructions: string; pool?: string; root?: string; timeout?: number },
  context: ActionContext
): ExecutionSpec {
  const docs = generateStepDocs(context.step, context.config);
  const docsBase64 = Buffer.from(docs).toString("base64");

  return {
    command: "node",
    args: [
      executorPath,
      "--docs", docsBase64,
      ...(params.pool ? ["--pool", params.pool] : []),
      ...(params.root ? ["--root", params.root] : []),
      ...(params.timeout ? ["--timeout", String(params.timeout)] : []),
    ],
  };
}
```

The pool executor script (`pool-executor.ts`, compiled to `pool-executor.js`) reads stdin, builds the troupe payload, invokes `troupe submit_task`, and writes the result to stdout:

```typescript
// libs/barnum/actions/pool-executor.ts
import { parseArgs } from "node:util";
import { execFileSync } from "node:child_process";

const { values } = parseArgs({
  options: {
    pool: { type: "string" },
    root: { type: "string" },
    docs: { type: "string" },
    timeout: { type: "string" },
  },
});

// Read task from stdin
const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const input = JSON.parse(Buffer.concat(chunks).toString());

// Build troupe payload
const docs = Buffer.from(values.docs!, "base64").toString();
const payload = JSON.stringify({
  kind: "Task",
  task: { instructions: docs, data: input },
  ...(values.timeout && { timeout_seconds: parseInt(values.timeout) }),
});

// Submit to troupe
const troupeBin = process.env.TROUPE ?? "troupe";
const args = ["submit_task"];
if (values.root) args.push("--root", values.root);
if (values.pool) args.push("--pool", values.pool);
args.push("--notify", "file", "--data", payload);

const result = execFileSync(troupeBin, args, { encoding: "utf-8" });
const response = JSON.parse(result);
if (response.kind === "Processed") {
  process.stdout.write(response.stdout);
} else {
  process.stderr.write("not processed by pool\n");
  process.exit(1);
}
```

**Command resolver:**

```typescript
// libs/barnum/actions/command.ts
export function resolveCommand(
  params: { script: string },
  _context: ActionContext
): ExecutionSpec {
  return {
    command: "sh",
    args: ["-c", params.script],
  };
}
```

### 7. BarnumConfig.run() resolves actions

**File:** `libs/barnum/run.ts`

```typescript
run(opts?: RunOptions): ChildProcess {
  const resolvedConfig = this.resolveActions();
  const args = opts?.resumeFrom
    ? ["run", "--resume-from", opts.resumeFrom]
    : ["run", "--config", JSON.stringify(resolvedConfig)];
  // ... rest unchanged
  return spawnBarnum(args);
}

private resolveActions(): ResolvedConfig {
  const resolvers = new Map([
    ["Pool", resolvePool],
    ["Command", resolveCommand],
  ]);

  return {
    ...this.config,
    steps: this.config.steps.map((step) => {
      const resolver = resolvers.get(step.action.kind);
      if (!resolver) throw new Error(`Unknown action kind: ${step.action.kind}`);
      const context: ActionContext = {
        workingDir: process.cwd(),
        config: this.config,
        step,
      };
      const exec = resolver(step.action, context);
      return {
        ...step,
        action: { command: exec.command, args: exec.args },
      };
    }),
  };
}
```

### 8. Step docs generation moves to JS

`generate_step_docs` currently lives in Rust (`crates/barnum_config/src/docs.rs`). It generates markdown instructions for pool agents. With JS resolution:

- The pool resolver calls `generateStepDocs(step, config)` at resolution time
- The result is base64-encoded and passed as a CLI arg to the pool executor
- The executor decodes it and includes it in the troupe payload

`generate_full_docs` stays in Rust for the `barnum config docs` subcommand. `write_instructions` and `generate_step_docs` can be deleted from Rust once JS resolution is the only path, or kept for `barnum config docs`.

### 9. Config types for Rust input

Rust needs a way to accept the resolved config (with `ActionExecution` instead of `ActionKind`). Two options:

**Option A: Two config formats.** `barnum_config` accepts both user-facing config (`ActionFile` with `Pool`/`Command`) and resolved config (`ActionExecution`). The user-facing config has a `resolve()` method that produces the resolved config. JS calls `resolve()` on its side and passes the result.

**Option B: Resolved-only.** Rust only accepts the resolved format. The CLI binary handles `ActionFile` → `ActionExecution` resolution for the direct-use case. `barnum_config` never sees `ActionFile`.

Option B is cleaner — `barnum_config` has one format, and the CLI is responsible for bridging from user-facing config to resolved config. But it means `barnum config validate` and `barnum config docs` need to work on `ActionFile` config (before resolution), which lives in the CLI binary, not in the library.

**Recommendation: Option A.** Keep both formats in `barnum_config`. The resolved config is what the runner consumes. `ConfigFile::resolve()` produces it by resolving `ActionFile` variants into `ActionExecution`. JS can also produce resolved configs directly. The `barnum_config` library remains self-contained.

### 10. Troupe binary discovery

The pool executor script needs to find `troupe`. Options:
- `TROUPE` env var (set by the JS layer, pointing to the bundled binary)
- Fall back to `troupe` on PATH
- The `@barnum/troupe` npm package already bundles the binary — the JS layer can resolve its path and pass it

The JS layer already does binary resolution for `barnum` itself (`process.env.BARNUM ?? require("./index.cjs")`). The same pattern works for troupe.

## What doesn't change

- State machine logic (`RunState`, task tracking, retry logic)
- `ActionHandle`, `run_action`, `spawn_worker` (already generic)
- `dispatch_finally` (always ShellAction with `self.working_dir`)
- `CompiledSchemas` and validation
- State log format (entries are the same; config entry stores resolved config)
- `barnum config validate`, `barnum config docs`, `barnum config graph` (operate on user-facing config)

## Resume behavior

On resume, Rust reads the state log which contains the resolved config (with `ActionExecution` fields). The JS layer doesn't run again — Rust uses the serialized execution specs directly. The command paths must still be valid at resume time. If the pool executor script moved between runs, resume would break. This is fine in practice since resume is expected to happen on the same machine.

## Phasing

1. **Add `ActionExecution` to resolved types** alongside existing `ActionKind`. Dual-format acceptance.
2. **Update `ShellAction`** to accept `command + args` instead of just `script`.
3. **Add JS resolution** in `BarnumConfig.run()` with pool and command resolvers.
4. **Add pool executor script** bundled with the npm package.
5. **Port `generateStepDocs` to JS.**
6. **Update `dispatch_task`** to use `ActionExecution` instead of matching on `ActionKind`.
7. **Remove `ActionKind`**, `PoolAction` (runtime), `submit.rs`, `Invoker` from Engine/RunnerConfig.
8. **Move CLI fallback resolution** into the barnum CLI binary for direct-use case.

## Relationship to other docs

- **ACTION_REGISTRY.md** — Superseded. Delete it.
- **PLUGGABLE_ACTION_KINDS.md** — The end-state vision. JS resolution is the implementation path. User-defined kinds register JS resolver functions. The `Custom` catch-all in config types allows arbitrary kind strings that JS resolves.
- **CLAUDE_CLI_ACTION_KIND.md** — Claude becomes a JS resolver function that returns `{ command: "claude", args: ["-p", "--model", model, ...] }`. No Rust code needed.
