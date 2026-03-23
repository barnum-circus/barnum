# JS Action Resolution

## Motivation

The Engine's `dispatch_task` matches on `ActionKind` variants to construct the appropriate `Action` impl. Every new action kind (Claude, Git, custom user kinds) requires adding a Rust enum variant, a match arm, and a new `Action` impl. This coupling is unnecessary — both `Pool` and `Command` ultimately just "run a subprocess, pipe stdin, read stdout."

The JS layer (`BarnumConfig`) already validates config, constructs CLI args, and spawns the Rust binary. This refactor extends JS to also resolve each step's action kind into a shell command string. Rust receives a config where every step has a `script` field. Rust runs `sh -c <script>`, manages the state machine, handles timeouts. It never needs to know what "Pool" or "Command" means.

This supersedes ACTION_REGISTRY.md.

## Architecture

```
User config (kind: "Pool", kind: "Command", kind: "Claude")
  → JS resolution (each kind → shell command string)
    → Resolved config (every step has a script string)
      → Rust (runs sh -c <script>, manages state tree, handles timeouts)
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

### 1. Resolved action type: just a script string

**File:** `crates/barnum_config/src/resolved.rs`

Replace `ActionKind` with a simple script string. Every action kind resolves to a shell command that Rust runs via `sh -c`.

```rust
pub struct Step {
    pub name: StepName,
    pub value_schema: Option<serde_json::Value>,
    pub script: String,  // was: action: ActionKind
    pub next: Vec<StepName>,
    pub finally_hook: Option<HookScript>,
    pub options: Options,
}
```

No new struct needed. The `CommandAction { script }` type already had the right shape — we just flatten it. JS resolvers produce shell strings; Rust consumes them uniformly.

The resolved config needs to serialize into the state log for resume. A `script: String` field serializes/deserializes trivially.

### 2. dispatch_task becomes trivial

**File:** `crates/barnum_config/src/runner/mod.rs`

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    info!(step = %task.step, "dispatching task");
    let action = Box::new(ShellAction {
        script: step.script.clone(),
        step_name: task.step.clone(),
        working_dir: self.working_dir.clone(),
    });
    spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
}
```

### 3. ShellAction doesn't change

**File:** `crates/barnum_config/src/runner/action.rs`

`ShellAction` already does `Command::new("sh").arg("-c").arg(&self.script)`. It stays exactly as-is. The only change is that `PoolAction` (the runtime struct) is deleted — every action is now a `ShellAction`.

### 4. Engine simplification

**File:** `crates/barnum_config/src/runner/mod.rs`

Engine drops `invoker` and `config` (no longer needed for `generate_step_docs`):

```rust
struct Engine<'a> {
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

Actually, `config` is still needed for `generate_full_docs` in the `docs` subcommand and for serializing into the state log. But `Engine` may not need `config` if docs generation moves entirely to JS. TBD during implementation.

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

**Path A: JS-driven (primary).** JS resolves actions into script strings before passing config to Rust. The config Rust receives already has a `script` per step.

**Path B: CLI-only (fallback).** When using `barnum run --config` directly (without the JS layer), the Rust CLI resolves `Pool` and `Command` actions itself. For `Command`, it's a passthrough (`script` field). For `Pool`, the CLI constructs the pool executor command string inline.

`ConfigFile::resolve()` in `barnum_config` continues to handle both: it can accept user-facing config with `ActionFile` variants and resolve them to script strings, or accept pre-resolved config that already has scripts.

### 6. JS action resolvers

Each action kind is a function that takes action params + context and returns a shell command string.

```typescript
type ActionResolver = (params: unknown, context: ActionContext) => string;

interface ActionContext {
  workingDir: string;
  config: ConfigFile;
  step: StepFile;
}
```

**Pool resolver (`libs/barnum/actions/pool.ts`):**

```typescript
const executorPath = resolve(import.meta.dirname, "pool-executor.js");

export function resolvePool(
  params: { instructions: string; pool?: string; root?: string; timeout?: number },
  context: ActionContext
): string {
  const docs = generateStepDocs(context.step, context.config);
  const docsBase64 = Buffer.from(docs).toString("base64");

  const args = [
    `node ${quote(executorPath)}`,
    `--docs ${quote(docsBase64)}`,
    ...(params.pool ? [`--pool ${quote(params.pool)}`] : []),
    ...(params.root ? [`--root ${quote(params.root)}`] : []),
    ...(params.timeout ? [`--timeout ${params.timeout}`] : []),
  ];

  return args.join(" ");
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

**Command resolver (`libs/barnum/actions/command.ts`):**

```typescript
export function resolveCommand(
  params: { script: string },
  _context: ActionContext
): string {
  return params.script;
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
  const resolvers = new Map<string, ActionResolver>([
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
      return {
        ...step,
        script: resolver(step.action, context),
      };
    }),
  };
}
```

### 8. Step docs generation moves to JS

`generate_step_docs` currently lives in Rust (`crates/barnum_config/src/docs.rs`). It generates markdown instructions for pool agents. With JS resolution:

- The pool resolver calls `generateStepDocs(step, config)` at resolution time
- The result is base64-encoded and baked into the pool executor command string
- The executor decodes it and includes it in the troupe payload

`generate_full_docs` stays in Rust for the `barnum config docs` subcommand. `generate_step_docs` gets a JS port. The Rust version can be kept for `generate_full_docs` (which calls it internally) or `generate_full_docs` can be rewritten to not depend on it.

### 9. Troupe binary discovery

The pool executor script needs to find `troupe`. Pattern:
- `TROUPE` env var (set by the JS layer, pointing to the bundled binary)
- Fall back to `troupe` on PATH
- The `@barnum/troupe` npm package already bundles the binary — the JS layer resolves its path via `require("@barnum/troupe")` and passes it as the env var

The JS layer already does binary resolution for `barnum` itself (`process.env.BARNUM ?? require("./index.cjs")`). Same pattern for troupe.

## What doesn't change

- State machine logic (`RunState`, task tracking, retry logic)
- `ActionHandle`, `run_action`, `spawn_worker` (already generic)
- `dispatch_finally` (already ShellAction with `self.working_dir`)
- `CompiledSchemas` and validation
- State log format (entries are the same; config entry stores resolved config)
- `barnum config validate`, `barnum config docs`, `barnum config graph` (operate on user-facing config)
- `ShellAction` implementation (still `sh -c <script>`)

## Resume behavior

On resume, Rust reads the state log which contains the resolved config (with `script` strings per step). The JS layer doesn't run again — Rust uses the serialized scripts directly. The command strings must still be valid at resume time (e.g., `node /path/to/pool-executor.js` requires the executor to still exist at that path). This is fine in practice since resume is expected to happen on the same machine.

## Phasing

1. **Add `script` field to resolved `Step`** alongside existing `action: ActionKind`. Dual-format acceptance.
2. **Add JS resolution** in `BarnumConfig.run()` with pool and command resolvers.
3. **Add pool executor script** bundled with the npm package.
4. **Port `generateStepDocs` to JS.**
5. **Update `dispatch_task`** to use `step.script` when present, falling back to `ActionKind` match.
6. **Remove `ActionKind`**, `PoolAction` (runtime), `submit.rs`, `Invoker` from Engine/RunnerConfig.
7. **Move CLI fallback resolution** into the barnum CLI binary for direct-use case.

## Relationship to other docs

- **ACTION_REGISTRY.md** — Superseded and deleted.
- **PLUGGABLE_ACTION_KINDS.md** — The end-state vision. JS resolution is the implementation path. User-defined kinds register JS resolver functions that return shell command strings.
- **CLAUDE_CLI_ACTION_KIND.md** — Claude becomes a JS resolver that returns something like `"claude -p --model sonnet --output-format json"`. No Rust code needed.
