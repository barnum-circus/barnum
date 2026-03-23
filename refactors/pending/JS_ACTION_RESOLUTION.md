# JS Action Resolution

## Motivation

The Engine's `dispatch_task` matches on `ActionKind` variants to construct the appropriate `Action` impl. Every new action kind (Claude, Git, custom user kinds) requires adding a Rust enum variant, a match arm, and a new `Action` impl. This coupling is unnecessary — both `Pool` and `Command` ultimately just "run a subprocess, pipe stdin, read stdout."

The JS layer (`BarnumConfig`) already validates config, constructs CLI args, and spawns the Rust binary. This refactor makes JS the execution layer for all actions. Rust dispatches every task by spawning a single JS executor, passing the action config and task value via stdin. The JS executor looks up the handler by kind and runs it. Rust manages the state machine, timeouts, retries, concurrency. JS handles action-specific execution.

This supersedes ACTION_REGISTRY.md.

## Architecture

```
Rust dispatch_task (every task, regardless of kind)
  → spawns: node /path/to/action-executor.js
    → stdin: { action: { kind, params: {...} }, task: { kind, value } }
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

Each handler file exports a default **action definition object** — not just a function. The object has an optional `validate` key (a Zod schema for the action's params) and a `handle` function. The Zod schema does double duty: runtime validation of params and TypeScript type inference for the handler's `params` argument.

```typescript
// libs/barnum/actions/command.ts
import { z } from "zod";
import { defineAction } from "./types.js";

export default defineAction({
  validate: z.object({ script: z.string() }),
  handle: async ({ params, task }) => {
    // params is typed as { script: string } — inferred from validate
    const stdout = execSync(params.script, { input: JSON.stringify(task), ... });
    return JSON.parse(stdout);
  },
});
```

The `index.ts` hardcodes the mapping from kind name to action definition. Adding a new kind means adding an import and a map entry.

**Future:** `BarnumConfig` gets a builder pattern where action kinds are registered explicitly:

```typescript
const barnum = BarnumConfig.builder()
  .action("Pool", poolAction)         // built-in, pre-registered
  .action("Command", commandAction)   // built-in, pre-registered
  .action("Claude", claudeAction)     // user-registered
  .fromConfig(config);

barnum.run();
```

The built-in kinds (Pool, Command) are pre-registered by the builder. Users add their own via `.action(kind, actionDef)`. Because each action definition carries its Zod schema, the builder can validate config params at construction time and the config construction API is fully typed — when you call `.action("Claude", claudeAction)`, the config type knows what params "Claude" accepts.

The hardcoded map in `index.ts` is the degenerate case of this — it becomes the default set of registrations in the builder.

## Current State

**Prerequisite landed:** ACTION_PARAMS_NESTING is complete. All action enums (`ActionFile`, `ActionKind`, `FinallyHook`) now use `#[serde(tag = "kind", content = "params")]`. This means `serde_json::to_value(action_kind)` produces `{ "kind": "Pool", "params": { "instructions": ..., "pool": ... } }` — exactly the shape the JS executor expects. Making the resolved action opaque (Step 5 below) is now a straightforward `serde_json::to_value()` call during resolution.

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

With ACTION_PARAMS_NESTING landed, `ConfigFile::resolve()` can convert `ActionFile` → `serde_json::Value` via `serde_json::to_value()`. The result is `{ "kind": "Pool", "params": { ... } }` — the exact shape JS handlers expect. No manual restructuring needed.

The action config serializes into the state log for resume. JS doesn't need to re-resolve anything — the action config is the same data the handler needs.

**Note:** This is a temporary loss of type safety in the resolved layer. The user-facing config types (`ActionFile` with `Pool`/`Command` variants in `config.rs`) and the generated schemas (Zod, JSON Schema) remain fully typed — schema generation operates on config types, not resolved types. Once the builder pattern lands, the resolved type can regain type safety: each registered handler declares its params type, and the builder validates at registration time.

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
  "action": { "kind": "Pool", "params": { "instructions": "...", "pool": "agents" } },
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
import { type ZodType, type z } from "zod";
import type { configFileSchema } from "../barnum-config-schema.zod.js";

export type ConfigFile = z.output<typeof configFileSchema>;
export type StepFile = ConfigFile["steps"][number];

/** A follow-up task to queue after this action completes. */
export interface FollowUpTask {
  kind: string;
  value: unknown;
}

/** The raw envelope piped to the JS executor's stdin by Rust. */
export interface RawEnvelope {
  /** The step's action config (kind + params). */
  action: { kind: string; params: Record<string, unknown> };
  /** The task being dispatched. */
  task: { kind: string; value: unknown };
}

/** Context passed to an action's handle function, with typed params. */
export interface ActionContext<TParams = Record<string, unknown>> {
  /** The validated and typed action params. */
  params: TParams;
  /** The task being dispatched. */
  task: { kind: string; value: unknown };
}

/**
 * An action definition: an object with a handle function and optional
 * Zod schema for params validation + type inference.
 *
 * The Zod schema serves two purposes:
 * 1. Runtime validation of the action's params before handle() is called
 * 2. TypeScript type inference — params in handle() is typed as z.output<validate>
 */
export interface ActionDefinition<TParams = Record<string, unknown>> {
  /** Optional Zod schema. Validates params at runtime, infers TParams at compile time. */
  validate?: ZodType<TParams>;
  /** Execute the action. Receives typed params + task, returns follow-up tasks. */
  handle: (ctx: ActionContext<TParams>) => Promise<FollowUpTask[]>;
}

/**
 * Helper to define an action with full type inference from the Zod schema.
 *
 * Usage:
 *   export default defineAction({
 *     validate: z.object({ script: z.string() }),
 *     handle: async ({ params, task }) => {
 *       // params is { script: string }
 *     },
 *   });
 */
export function defineAction<T extends ZodType>(
  def: { validate: T; handle: (ctx: ActionContext<z.output<T>>) => Promise<FollowUpTask[]> },
): ActionDefinition<z.output<T>>;
export function defineAction(
  def: { handle: (ctx: ActionContext<Record<string, unknown>>) => Promise<FollowUpTask[]> },
): ActionDefinition;
export function defineAction(def: ActionDefinition<any>): ActionDefinition<any> {
  return def;
}
```

#### `libs/barnum/actions/command.ts`

The Command handler spawns the user's shell script, piping the task (without the action config — users expect `{ kind, value }`).

```typescript
import { execSync } from "node:child_process";
import { z } from "zod";
import { defineAction } from "./types.js";

/**
 * Command action: spawns sh -c <script>, pipes the task, returns parsed stdout.
 *
 * The user's script receives `{"kind": "StepName", "value": {...}}` on stdin
 * and must write `[{"kind": "NextStep", "value": {...}}, ...]` to stdout.
 */
export default defineAction({
  validate: z.object({ script: z.string() }),

  handle: async ({ params, task }) => {
    // params.script is typed as string — validated by Zod before we get here
    const stdin = JSON.stringify(task);
    const stdout = execSync(params.script, {
      input: stdin,
      encoding: "utf-8",
      shell: "/bin/sh",
    });

    return JSON.parse(stdout);
  },
});
```

#### `libs/barnum/actions/pool.ts`

The Pool handler generates docs, builds the troupe payload, submits via troupe CLI, and returns the agent's response.

```typescript
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { z } from "zod";
import { defineAction } from "./types.js";
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

const instructionsSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("Inline"), value: z.string() }),
  z.object({ kind: z.literal("Link"), path: z.string() }),
]);

/**
 * Pool action: submits the task to the troupe agent pool and returns
 * the agent's response (follow-up tasks).
 */
export default defineAction({
  validate: z.object({
    instructions: instructionsSchema,
    pool: z.string().optional(),
    root: z.string().optional(),
    timeout: z.number().optional(),
  }),

  handle: async ({ params, task }) => {
    // params is fully typed: { instructions: ..., pool?: string, root?: string, timeout?: number }
    const troupe = troupeBinary();

    // TODO: generateStepDocs needs the full step + config context.
    // For now, use the instructions directly. Full docs generation
    // requires passing step metadata through the envelope.
    const instructions = params.instructions.kind === "Inline"
      ? params.instructions.value
      : "";

    // Build troupe payload
    const payload: Record<string, unknown> = { task, instructions };
    if (params.timeout != null) {
      payload.timeout_seconds = params.timeout;
    }

    // Submit to troupe
    const args = ["submit_task"];
    if (params.root) args.push("--root", params.root);
    if (params.pool) args.push("--pool", params.pool);
    args.push("--notify", "file", "--data", JSON.stringify(payload));

    const result = execFileSync(troupe, args, { encoding: "utf-8" });
    const response = JSON.parse(result);

    if (response.kind === "Processed") {
      return JSON.parse(response.stdout);
    }

    throw new Error(`Pool submission failed: ${JSON.stringify(response)}`);
  },
});
```

#### `libs/barnum/actions/docs.ts`

JS port of `generate_step_docs`. Used by the pool handler to generate agent instructions. Takes the instructions string directly (the pool handler already has it typed from its Zod schema), plus the step/config context for response docs.

```typescript
import type { ConfigFile, StepFile } from "./types.js";

export function generateStepDocs(
  stepName: string,
  instructions: string,
  step: StepFile,
  config: ConfigFile,
): string {
  const lines: string[] = [];

  lines.push(
    "**IMPORTANT: This task is completely isolated. You have no memory of previous tasks. " +
    "Even if this task seems related to prior work, you must complete it from scratch using " +
    "only the information provided here.**",
    "",
  );

  lines.push(`# Current Step: ${stepName}`, "");

  if (instructions.length > 0) {
    lines.push(instructions, "");
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

Hardcoded action definition registry.

```typescript
import type { ActionDefinition } from "./types.js";
import poolAction from "./pool.js";
import commandAction from "./command.js";

const actions = new Map<string, ActionDefinition<any>>([
  ["Pool", poolAction],
  ["Command", commandAction],
]);

export function getAction(kind: string): ActionDefinition<any> {
  const action = actions.get(kind);
  if (!action) {
    throw new Error(
      `Unknown action kind: "${kind}". ` +
      `Built-in kinds: ${[...actions.keys()].join(", ")}`,
    );
  }
  return action;
}

export type { ActionDefinition, ActionContext, FollowUpTask } from "./types.js";
export { defineAction } from "./types.js";
```

#### `libs/barnum/actions/executor.ts`

The single entry point that Rust spawns for every task. Reads the envelope from stdin, validates params against the action's Zod schema (if present), dispatches to the handler, writes the result to stdout. Invoked via `tsx` (see section 6).

```typescript
import { getAction } from "./index.js";
import type { RawEnvelope } from "./types.js";

// Read envelope from stdin
const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope: RawEnvelope = JSON.parse(Buffer.concat(chunks).toString());

// Look up action definition
const action = getAction(envelope.action.kind);

// Validate params if the action defines a schema
const params = action.validate
  ? action.validate.parse(envelope.action.params)
  : envelope.action.params;

// Dispatch to handler with validated, typed params
const results = await action.handle({ params, task: envelope.task });

// Write follow-up tasks to stdout
process.stdout.write(JSON.stringify(results));
```

### 6. BarnumConfig passes executor command

**File:** `libs/barnum/run.ts`

The package ships TypeScript source (no build step). The executor is `executor.ts`, which needs a TS-capable runtime. We use `tsx` as a dependency — it's lightweight (~2MB), works with any Node version, and avoids fragile runtime detection (`process.execPath` doesn't detect `babel-node`, `ts-node` binary, or other wrappers).

**`tsx` added as a dependency in `package.json`:**

```json
{
  "dependencies": {
    "zod": "^3.0.0",
    "tsx": "^4.0.0"
  }
}
```

The JS layer resolves the `tsx` binary and executor script path, passes them to the barnum CLI via `--executor`.

```typescript
import { resolve } from "node:path";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const tsxBinary = require.resolve("tsx/cli");
const executorPath = resolve(import.meta.dirname, "actions", "executor.ts");

export class BarnumConfig {
  // ...existing...

  run(opts?: RunOptions): ChildProcess {
    const args = opts?.resumeFrom
      ? ["run", "--resume-from", opts.resumeFrom]
      : ["run", "--config", JSON.stringify(this.config)];
    args.push("--executor", `node ${tsxBinary} ${executorPath}`);
    if (opts?.entrypointValue) args.push("--entrypoint-value", opts.entrypointValue);
    // ...rest of opts...
    return spawnBarnum(args);
  }
}
```

For direct CLI use without the JS wrapper, the user passes `--executor` explicitly (e.g., `barnum run --executor "npx tsx /path/to/executor.ts" ...`).

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
  "action": { "kind": "Pool", "params": { ... } },
  "task": { "kind": "Analyze", "value": {...} },
  "step": { "name": "Analyze", "next": ["Implement", "Done"], ... },
  "config": { "steps": [...] }
}
```

This gives the JS handler everything it needs to generate docs. It's redundant (the task kind == the step name), but it's explicit.

**Option B: Pre-generate docs in Rust, include in envelope.** Rust generates the docs string and includes it:

```json
{
  "action": { "kind": "Pool", "params": { ... } },
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

On resume, Rust reads the state log which contains the config with action configs per step (opaque JSON). The `--executor` flag must be provided again (it's not stored in the log). For the JS-driven path, `BarnumConfig` always provides it (using bundled `tsx`). For direct CLI use, the user passes `--executor` explicitly.

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
