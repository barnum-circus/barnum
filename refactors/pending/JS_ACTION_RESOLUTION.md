# JS Action Resolution

## Motivation

The Engine's `dispatch_task` matches on `ActionKind` variants to construct the appropriate `Action` impl. Every new action kind (Claude, Git, custom user kinds) requires adding a Rust enum variant, a match arm, and a new `Action` impl. This coupling is unnecessary — both `Pool` and `Command` ultimately just "run a subprocess, pipe stdin, read stdout."

The JS layer (`BarnumConfig`) already validates config, constructs CLI args, and spawns the Rust binary. This refactor makes JS the execution layer for all actions. Rust dispatches every task by spawning a single JS executor, passing the action config and task value via stdin. The JS executor looks up the handler by kind and runs it. Rust manages the state machine, timeouts, retries, concurrency. JS handles action-specific execution.

This supersedes ACTION_REGISTRY.md.

## Architecture

```
Rust dispatch_task (every task, regardless of kind)
  → spawns: node /path/to/action-executor.js
    → stdin: { action: { kind, ...params }, task: { kind, value } }
    → JS looks up handler by action.kind
    → handler executes (calls troupe, runs shell script, calls Claude, etc.)
    → stdout: [{ kind: "NextStep", value: {...} }, ...]
  ← Rust reads stdout, validates, queues follow-up tasks
```

Rust doesn't know what "Pool" or "Command" means. It pipes the action config + task value to the JS executor and reads the result.

## File Layout

```
libs/barnum/
├── actions/
│   ├── executor.ts         # Entry point: reads stdin, dispatches to handler, writes stdout
│   ├── types.ts            # ActionHandler type, ActionEnvelope type
│   ├── pool.ts             # Pool handler: submits to troupe, returns agent response
│   ├── command.ts          # Command handler: spawns sh -c, pipes task, returns output
│   ├── docs.ts             # JS port of generate_step_docs
│   └── index.ts            # Hardcoded handler registry (kind → handler)
├── run.ts                  # BarnumConfig (passes executor path to barnum CLI)
├── index.ts
└── package.json
```

Each handler file exports a `handle` function with the same signature. The `index.ts` hardcodes the mapping from kind name to handler. Adding a new kind means adding an import and a map entry.

**Future:** `BarnumConfig` gets a builder pattern where action kinds are registered explicitly:

```typescript
const barnum = BarnumConfig.builder()
  .action("Pool", poolHandler)       // built-in, pre-registered
  .action("Command", commandHandler) // built-in, pre-registered
  .action("Claude", claudeHandler)   // user-registered
  .fromConfig(config);

barnum.run();
```

The built-in kinds (Pool, Command) are pre-registered by the builder. Users add their own via `.action(kind, handler)`. The hardcoded map in `index.ts` is the degenerate case of this — it becomes the default set of registrations in the builder.

## Current State

### dispatch_task (`crates/barnum_config/src/runner/mod.rs:696-730`)

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    let tx = self.tx.clone();

    match &step.action {
        ActionKind::Pool(PoolActionConfig {
            pool, root, timeout: pool_timeout, ..
        }) => {
            let docs = generate_step_docs(step, self.config);
            let action = Box::new(PoolAction {
                root: root.clone(), pool: pool.clone(),
                invoker: self.invoker.clone(), docs,
                step_name: task.step.clone(), pool_timeout: *pool_timeout,
            });
            spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
        }
        ActionKind::Command(CommandAction { script }) => {
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

### What gets deleted from Rust

- `PoolAction` struct in `runner/action.rs` (the runtime Action impl)
- `submit.rs` (troupe submission logic — `build_agent_payload`, `submit_via_cli`)
- `ActionKind` enum in `resolved.rs`
- `PoolAction` and `CommandAction` structs in `resolved.rs`
- `Invoker<TroupeCli>` from `Engine` and `RunnerConfig`
- `cli_invoker` and `troupe_cli` dependencies from `barnum_config`
- `generate_step_docs` in `docs.rs` (moves to JS; `generate_full_docs` stays for `barnum config docs`)
- `Config::has_pool_actions()` in `resolved.rs`

## Proposed Changes

### 1. Resolved action type: opaque JSON

**File:** `crates/barnum_config/src/resolved.rs`

Replace `ActionKind` with opaque JSON. Rust doesn't interpret action configs — it passes them through to the JS executor.

```rust
pub struct Step {
    pub name: StepName,
    pub value_schema: Option<serde_json::Value>,
    pub action: serde_json::Value,  // was: action: ActionKind — opaque, passed to JS
    pub next: Vec<StepName>,
    pub finally_hook: Option<HookScript>,
    pub options: Options,
}
```

The action config serializes into the state log for resume. JS doesn't need to re-resolve anything — the action config is the same data the handler needs.

### 2. dispatch_task becomes kind-agnostic

**File:** `crates/barnum_config/src/runner/mod.rs`

Rust builds an enriched envelope containing the action config and task value, then pipes it to the JS executor.

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    info!(step = %task.step, "dispatching task");
    let action = Box::new(ShellAction {
        script: self.executor_script.clone(),
        step_name: task.step.clone(),
        working_dir: self.working_dir.clone(),
    });
    spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
}
```

But `ShellAction` currently pipes `{"kind": "StepName", "value": {...}}` to stdin. The JS executor needs more: the action config and docs. Two options for getting the action config to the executor:

**Option A: Enriched stdin.** Change what `ShellAction` pipes. Instead of just `{ kind, value }`, pipe `{ action: {...}, task: { kind, value } }`. This breaks the stdin contract for Command actions (users expect `{ kind, value }`), but the JS Command handler restores it by piping just `{ kind, value }` to the user's shell script.

**Option B: Action config as CLI args.** The executor script path includes the action config as a base64 arg: `node executor.js --action <base64>`. Stdin stays as `{ kind, value }`. Simpler for the Rust side but means serializing the action config into a CLI arg.

**Recommendation: Option A.** The stdin contract is between Rust and the JS executor, not between Rust and the user's scripts. The JS handlers control what their subprocesses receive.

The enriched stdin envelope:

```json
{
  "action": { "kind": "Pool", "instructions": "...", "pool": "agents" },
  "task": { "kind": "Analyze", "value": { "file": "main.rs" } }
}
```

### 3. ShellAction pipes the enriched envelope

**File:** `crates/barnum_config/src/runner/action.rs`

`ShellAction` currently constructs `{"kind": step_name, "value": value}` and pipes it. It changes to pipe a richer envelope that includes the step's action config.

```rust
pub struct ShellAction {
    pub script: String,
    pub action_config: serde_json::Value,  // NEW: the step's action config
    pub step_name: StepName,
    pub working_dir: PathBuf,
}
```

The piped stdin becomes:
```rust
let envelope = serde_json::json!({
    "action": self.action_config,
    "task": { "kind": &self.step_name, "value": value },
});
```

### 4. Engine holds executor script path

**File:** `crates/barnum_config/src/runner/mod.rs`

Engine drops `invoker`, adds `executor_script`:

```rust
struct Engine<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    executor_script: String,
    working_dir: PathBuf,
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
}
```

`RunnerConfig` drops `invoker`, adds `executor_script`:

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub executor_script: &'a str,
    pub state_log_path: &'a Path,
}
```

The JS layer passes the executor path via a new CLI flag: `barnum run --executor /path/to/executor.js --config ...`. For direct CLI use (no JS), the CLI resolves a default executor path.

### 5. JS action handler files

#### `libs/barnum/actions/types.ts`

```typescript
import type { z } from "zod";
import type { configFileSchema } from "../barnum-config-schema.zod.js";

export type ConfigFile = z.output<typeof configFileSchema>;
export type StepFile = ConfigFile["steps"][number];

/** The envelope piped to the JS executor's stdin by Rust. */
export interface ActionEnvelope {
  /** The step's action config (kind + params). */
  action: { kind: string } & Record<string, unknown>;
  /** The task being dispatched. */
  task: { kind: string; value: unknown };
}

/**
 * An action handler executes a single task dispatch.
 *
 * It receives the full action config and the task value, and returns
 * the follow-up tasks to queue.
 */
export type ActionHandler = (
  envelope: ActionEnvelope,
) => Promise<Array<{ kind: string; value: unknown }>>;
```

#### `libs/barnum/actions/command.ts`

The Command handler spawns the user's shell script, piping the task envelope (without the action config — users expect `{ kind, value }`).

```typescript
import { execSync } from "node:child_process";
import type { ActionHandler } from "./types.js";

/**
 * Command handler: spawns sh -c <script>, pipes the task, returns parsed stdout.
 *
 * The user's script receives `{"kind": "StepName", "value": {...}}` on stdin
 * and must write `[{"kind": "NextStep", "value": {...}}, ...]` to stdout.
 */
export const handle: ActionHandler = async (envelope) => {
  const script = envelope.action.script;
  if (typeof script !== "string") {
    throw new Error(`Command action requires a "script" string, got: ${typeof script}`);
  }

  // Pipe just { kind, value } to the user's script (not the action config)
  const stdin = JSON.stringify(envelope.task);
  const stdout = execSync(script, {
    input: stdin,
    encoding: "utf-8",
    shell: "/bin/sh",
  });

  return JSON.parse(stdout);
};
```

#### `libs/barnum/actions/pool.ts`

The Pool handler generates docs, builds the troupe payload, submits via troupe CLI, and returns the agent's response.

```typescript
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import type { ActionHandler } from "./types.js";
import { generateStepDocs } from "./docs.js";

const require = createRequire(import.meta.url);

function troupeBinary(): string {
  if (process.env.TROUPE) return process.env.TROUPE;
  try {
    return require("@barnum/troupe");
  } catch {
    return "troupe";
  }
}

/**
 * Pool handler: submits the task to the troupe agent pool and returns
 * the agent's response (follow-up tasks).
 *
 * The handler:
 * 1. Generates markdown docs for the agent (instructions + valid responses)
 * 2. Builds the troupe payload with task, docs, and optional timeout
 * 3. Calls `troupe submit_task` and returns the agent's parsed response
 */
export const handle: ActionHandler = async (envelope) => {
  const { action, task } = envelope;
  const troupe = troupeBinary();

  // TODO: generateStepDocs needs the full step + config context.
  // For now, use the instructions directly. Full docs generation
  // requires passing step metadata through the envelope.
  const instructions = typeof action.instructions === "string"
    ? action.instructions
    : "";

  // Build troupe payload
  const payload: Record<string, unknown> = {
    task,
    instructions,
  };
  if (typeof action.timeout === "number") {
    payload.timeout_seconds = action.timeout;
  }

  // Submit to troupe
  const args = ["submit_task"];
  if (typeof action.root === "string") args.push("--root", action.root);
  if (typeof action.pool === "string") args.push("--pool", action.pool);
  args.push("--notify", "file", "--data", JSON.stringify(payload));

  const result = execFileSync(troupe, args, { encoding: "utf-8" });
  const response = JSON.parse(result);

  if (response.kind === "Processed") {
    return JSON.parse(response.stdout);
  }

  throw new Error(`Pool submission failed: ${JSON.stringify(response)}`);
};
```

#### `libs/barnum/actions/docs.ts`

JS port of `generate_step_docs`. Used by the pool handler to generate agent instructions. (Same as before — generates markdown with step name, instructions, valid responses, schemas.)

```typescript
import type { ConfigFile, StepFile } from "./types.js";

export function generateStepDocs(step: StepFile, config: ConfigFile): string {
  const lines: string[] = [];

  lines.push(
    "**IMPORTANT: This task is completely isolated. You have no memory of previous tasks. " +
    "Even if this task seems related to prior work, you must complete it from scratch using " +
    "only the information provided here.**",
    "",
  );

  lines.push(`# Current Step: ${step.name}`, "");

  if (step.action.kind === "Pool") {
    const instructions = (step.action as Record<string, unknown>).instructions;
    if (typeof instructions === "string" && instructions.length > 0) {
      lines.push(instructions, "");
    }
  }

  if (step.next.length === 0) {
    lines.push("## Terminal Step", "", "This is a terminal step. Return an empty array: `[]`");
  } else {
    lines.push(
      "## Valid Responses", "",
      "You must return a JSON array of tasks. Each task has `kind` and `value` fields.", "",
      "Valid next steps:", "",
    );

    for (const nextName of step.next) {
      const nextStep = config.steps.find((s) => s.name === nextName);
      if (!nextStep) continue;

      lines.push(`### ${nextName}`, "");

      if (!nextStep.value_schema) {
        lines.push("Accepts any JSON value.", "", "```json", `{"kind": "${nextName}", "value": <any>}`, "```");
      } else {
        lines.push(
          "Value must match schema:", "", "```json",
          JSON.stringify(nextStep.value_schema, null, 2),
          "```", "", "Example:", "```json",
          `{"kind": "${nextName}", "value": {...}}`, "```",
        );
      }
      lines.push("");
    }
  }

  return lines.join("\n");
}
```

#### `libs/barnum/actions/index.ts`

Hardcoded handler registry.

```typescript
import type { ActionHandler } from "./types.js";
import { handle as handlePool } from "./pool.js";
import { handle as handleCommand } from "./command.js";

const handlers = new Map<string, ActionHandler>([
  ["Pool", handlePool],
  ["Command", handleCommand],
]);

export function getHandler(kind: string): ActionHandler {
  const handler = handlers.get(kind);
  if (!handler) {
    throw new Error(
      `Unknown action kind: "${kind}". ` +
      `Built-in kinds: ${[...handlers.keys()].join(", ")}`,
    );
  }
  return handler;
}

export type { ActionHandler, ActionEnvelope } from "./types.js";
```

#### `libs/barnum/actions/executor.ts`

The single entry point that Rust spawns for every task. Reads the envelope from stdin, dispatches to the right handler, writes the result to stdout.

```typescript
#!/usr/bin/env node
import { getHandler } from "./index.js";
import type { ActionEnvelope } from "./types.js";

// Read envelope from stdin
const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope: ActionEnvelope = JSON.parse(Buffer.concat(chunks).toString());

// Dispatch to handler
const handler = getHandler(envelope.action.kind);
const results = await handler(envelope);

// Write follow-up tasks to stdout
process.stdout.write(JSON.stringify(results));
```

### 6. BarnumConfig passes executor path

**File:** `libs/barnum/run.ts`

The JS layer resolves the executor script path and passes it to the barnum CLI.

```typescript
import { resolve } from "node:path";

const executorPath = resolve(import.meta.dirname, "actions", "executor.js");

export class BarnumConfig {
  // ...existing...

  run(opts?: RunOptions): ChildProcess {
    const args = opts?.resumeFrom
      ? ["run", "--resume-from", opts.resumeFrom]
      : ["run", "--config", JSON.stringify(this.config)];
    args.push("--executor", `node ${executorPath}`);
    if (opts?.entrypointValue) args.push("--entrypoint-value", opts.entrypointValue);
    // ...rest of opts...
    return spawnBarnum(args);
  }
}
```

### 7. Config resolution simplifies

`ConfigFile::resolve()` no longer needs to resolve `ActionFile` → `ActionKind`. The action config stays as-is (opaque JSON). Resolution only handles:
- Resolving `MaybeLinked` instructions (file links → inline content)
- Resolving schema links
- Computing effective options

The resolved `Step.action` is a `serde_json::Value` containing the original action config with links resolved.

### 8. Step docs generation

The pool handler needs step docs (instructions + valid responses + schemas). This requires the full step + config context, not just the action config.

**Option A: Include step metadata in the envelope.** Rust adds `step` and `config` fields to the stdin envelope:

```json
{
  "action": { "kind": "Pool", ... },
  "task": { "kind": "Analyze", "value": {...} },
  "step": { "name": "Analyze", "next": ["Implement", "Done"], ... },
  "config": { "steps": [...] }
}
```

This gives the JS handler everything it needs to generate docs. It's redundant (the task kind == the step name), but it's explicit.

**Option B: Pre-generate docs in Rust, include in envelope.** Rust generates the docs string and includes it:

```json
{
  "action": { "kind": "Pool", ... },
  "task": { "kind": "Analyze", "value": {...} },
  "docs": "# Current Step: Analyze\n\n..."
}
```

Simpler for JS but keeps the docs generation in Rust.

**Recommendation: Option A** for now. It's more data but gives JS handlers full context. Docs generation moves entirely to JS. Option B is fine as a shortcut during migration.

### 9. Troupe binary discovery

Same as before. The pool handler finds troupe via:
1. `TROUPE` env var (explicit override)
2. `require("@barnum/troupe")` (bundled binary from npm package)
3. `"troupe"` on PATH (fallback)

### 10. dispatch_finally stays in Rust

Finally hooks are always shell scripts (no action kind dispatch). They continue to use `ShellAction` directly with the hook's script. They don't go through the JS executor — they're a Rust-native concept.

```rust
fn dispatch_finally(&self, ...) {
    let action = Box::new(ShellAction {
        script: hook_script.to_string(),
        action_config: serde_json::Value::Null,  // not used for finally hooks
        step_name: task.step.clone(),
        working_dir: self.working_dir.clone(),
    });
    // ...
}
```

For finally hooks, `ShellAction` pipes just `{ kind, value }` (no enriched envelope) since the hook script expects the same format as today's Command actions.

## What doesn't change

- State machine logic (`RunState`, task tracking, retry logic)
- `ActionHandle`, `run_action`, `spawn_worker` (already generic)
- `CompiledSchemas` and validation
- State log format (entries are the same; config entry stores action config as-is)
- `barnum config validate`, `barnum config docs`, `barnum config graph` (operate on user-facing config)
- Finally hooks (stay as direct `ShellAction` with `sh -c`)

## Resume behavior

On resume, Rust reads the state log which contains the config with action configs per step (opaque JSON). The `--executor` flag must be provided again (it's not stored in the log). For the JS-driven path, `BarnumConfig` always provides it. For direct CLI use, the user provides it or the CLI uses a default.

## Phasing

1. **Add `executor_script` to Engine/RunnerConfig**, `--executor` CLI flag. Default to current behavior when not set.
2. **Change `ShellAction` to pipe enriched envelope** when `action_config` is present, plain `{ kind, value }` when null (backward compat for finally hooks and migration).
3. **Create `libs/barnum/actions/` directory** with types.ts, command.ts, pool.ts, docs.ts, index.ts, executor.ts.
4. **Update `BarnumConfig.run()`** to pass `--executor` flag.
5. **Make resolved `Step.action` opaque** (`serde_json::Value` instead of `ActionKind`).
6. **Update `dispatch_task`** to always use executor script.
7. **Remove `ActionKind`**, `PoolAction` (runtime), `submit.rs`, `Invoker` from Engine/RunnerConfig.

## Relationship to other docs

- **ACTION_REGISTRY.md** — Superseded and deleted.
- **PLUGGABLE_ACTION_KINDS.md** — The end-state vision. User-defined kinds register handler functions via the `BarnumConfig` builder's `.action(kind, handler)` method.
- **CLAUDE_CLI_ACTION_KIND.md** — Claude becomes `libs/barnum/actions/claude.ts` exporting a `handle` function that spawns the Claude CLI with the task value and returns the parsed response. No Rust code needed.
