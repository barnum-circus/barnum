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

## File Layout

```
libs/barnum/
├── actions/
│   ├── index.ts           # Resolver registry (discovers + exports all resolvers)
│   ├── types.ts           # Shared types (ActionResolver, ActionContext)
│   ├── pool.ts            # Pool resolver: kind "Pool" → shell command
│   ├── pool-executor.ts   # Standalone script invoked by pool's shell command
│   ├── command.ts         # Command resolver: kind "Command" → shell command (passthrough)
│   └── docs.ts            # JS port of generate_step_docs
├── run.ts                 # BarnumConfig (calls resolvers before spawning barnum)
├── index.ts               # Package exports
└── package.json           # Includes actions/ in "files"
```

Each resolver file exports a `resolve` function with the same signature. The `index.ts` hardcodes the mapping from kind name to resolver — no convention-based discovery, no magic. Adding a new kind means adding an import and a map entry.

**Future:** `BarnumConfig` gets a builder pattern where action kinds are registered explicitly:

```typescript
const barnum = BarnumConfig.builder()
  .action("Pool", resolvePool)       // built-in, pre-registered
  .action("Command", resolveCommand) // built-in, pre-registered
  .action("Claude", resolveClaude)   // user-registered
  .fromConfig(config);

barnum.run();
```

The built-in kinds (Pool, Command) are pre-registered by the builder. Users add their own via `.action(kind, resolver)`. The hardcoded map in `index.ts` is the degenerate case of this — it becomes the default set of registrations in the builder.

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

### 1. Resolved action type: just a script string

**File:** `crates/barnum_config/src/resolved.rs`

Replace `ActionKind` with a plain script string. Every action kind resolves to a shell command that Rust runs via `sh -c`.

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

No new struct needed. `ShellAction` already does `sh -c <script>`. The resolved config serializes into the state log trivially.

### 2. dispatch_task becomes trivial

```rust
fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
    let step = self.step_map.get(&task.step).expect("[P015] unknown step");
    let timeout = step.options.timeout.map(Duration::from_secs);
    let action = Box::new(ShellAction {
        script: step.script.clone(),
        step_name: task.step.clone(),
        working_dir: self.working_dir.clone(),
    });
    spawn_worker(self.tx.clone(), action, task_id, task, WorkerKind::Task, timeout);
}
```

### 3. ShellAction doesn't change

`ShellAction` already does `Command::new("sh").arg("-c").arg(&self.script)`. The only change is deleting `PoolAction` (the runtime struct) — every action is now a `ShellAction`.

### 4. Engine and RunnerConfig drop invoker

```rust
pub struct RunnerConfig<'a> {
    pub working_dir: &'a Path,
    pub wake_script: Option<&'a str>,
    pub state_log_path: &'a Path,
}
```

### 5. JS resolver files

#### `libs/barnum/actions/types.ts`

Shared types for all resolvers.

```typescript
import type { z } from "zod";
import type { configFileSchema } from "../barnum-config-schema.zod.js";

export type ConfigFile = z.output<typeof configFileSchema>;
export type StepFile = ConfigFile["steps"][number];

export interface ActionContext {
  /** Absolute path to the config file's directory (for resolving relative paths). */
  configDir: string;
  /** The full config. */
  config: ConfigFile;
  /** The step this action belongs to. */
  step: StepFile;
}

/**
 * An action resolver takes the action params from a step's `action` field
 * and returns a shell command string. Rust will run this via `sh -c <script>`.
 *
 * **Contract:**
 * - stdin: JSON `{"kind": "<step name>", "value": <payload>}`
 * - stdout: JSON array of follow-up tasks `[{"kind": "NextStep", "value": {...}}, ...]`
 * - exit 0 on success, non-zero on failure
 */
export type ActionResolver = (action: Record<string, unknown>, context: ActionContext) => string;
```

#### `libs/barnum/actions/command.ts`

The simplest resolver — `Command` actions already have a `script` field. Passthrough.

```typescript
import type { ActionResolver } from "./types.js";

/**
 * Command resolver: passes through the script field unchanged.
 *
 * Config input:
 *   { "kind": "Command", "script": "jq '.value | {kind: \"Done\", value: .}' | jq -s" }
 *
 * Output script:
 *   jq '.value | {kind: "Done", value: .}' | jq -s
 */
export const resolve: ActionResolver = (action) => {
  const script = action.script;
  if (typeof script !== "string") {
    throw new Error(`Command action requires a "script" string, got: ${typeof script}`);
  }
  return script;
};
```

#### `libs/barnum/actions/pool.ts`

Pool resolver: constructs a shell command that invokes `pool-executor.js` with the right args. The executor handles troupe submission.

```typescript
import { resolve as resolvePath } from "node:path";
import { createRequire } from "node:module";
import type { ActionResolver } from "./types.js";
import { generateStepDocs } from "./docs.js";

const require = createRequire(import.meta.url);

/** Resolve the troupe binary path from the @barnum/troupe package. */
function troupeBinary(): string {
  if (process.env.TROUPE) return process.env.TROUPE;
  try {
    return require("@barnum/troupe");
  } catch {
    return "troupe"; // fall back to PATH
  }
}

/** Shell-escape a string for embedding in a shell command. */
function shellQuote(s: string): string {
  return "'" + s.replace(/'/g, "'\\''") + "'";
}

/**
 * Pool resolver: resolves a Pool action to a shell command that submits
 * the task to the troupe agent pool.
 *
 * Config input:
 *   {
 *     "kind": "Pool",
 *     "instructions": {"kind": "Inline", "value": "Analyze the input."},
 *     "pool": "agents",
 *     "root": "/tmp/troupe",
 *     "timeout": 120
 *   }
 *
 * Output script (conceptual):
 *   node /path/to/pool-executor.js \
 *     --troupe /path/to/troupe \
 *     --docs <base64-encoded docs> \
 *     --pool agents \
 *     --root /tmp/troupe \
 *     --timeout 120
 */
export const resolve: ActionResolver = (action, context) => {
  const executorPath = resolvePath(import.meta.dirname, "pool-executor.js");
  const troupe = troupeBinary();

  // Generate agent docs (instructions + valid responses + schemas)
  const docs = generateStepDocs(context.step, context.config);
  const docsBase64 = Buffer.from(docs).toString("base64");

  const parts = [
    `node ${shellQuote(executorPath)}`,
    `--troupe ${shellQuote(troupe)}`,
    `--docs ${shellQuote(docsBase64)}`,
  ];

  const pool = action.pool;
  if (typeof pool === "string") parts.push(`--pool ${shellQuote(pool)}`);

  const root = action.root;
  if (typeof root === "string") parts.push(`--root ${shellQuote(root)}`);

  const timeout = action.timeout;
  if (typeof timeout === "number") parts.push(`--timeout ${timeout}`);

  return parts.join(" ");
};
```

#### `libs/barnum/actions/pool-executor.ts`

Standalone script invoked by the pool resolver's shell command. Reads task from stdin, submits to troupe, writes response to stdout. This is what Rust actually spawns via `sh -c`.

```typescript
#!/usr/bin/env node
import { parseArgs } from "node:util";
import { execFileSync } from "node:child_process";

const { values } = parseArgs({
  options: {
    troupe:  { type: "string" },
    pool:    { type: "string" },
    root:    { type: "string" },
    docs:    { type: "string" },
    timeout: { type: "string" },
  },
});

// 1. Read task JSON from stdin
const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const taskEnvelope = JSON.parse(Buffer.concat(chunks).toString());

// 2. Build troupe payload
//    taskEnvelope is {"kind": "StepName", "value": {...}}
const docs = Buffer.from(values.docs!, "base64").toString();
const payload: Record<string, unknown> = {
  task: taskEnvelope,
  instructions: docs,
};
if (values.timeout) {
  payload.timeout_seconds = parseInt(values.timeout, 10);
}

// 3. Submit to troupe
const troupe = values.troupe ?? "troupe";
const args = ["submit_task"];
if (values.root) args.push("--root", values.root);
if (values.pool) args.push("--pool", values.pool);
args.push("--notify", "file", "--data", JSON.stringify(payload));

const result = execFileSync(troupe, args, { encoding: "utf-8" });
const response = JSON.parse(result);

// 4. Write agent's response (the follow-up tasks array) to stdout
if (response.kind === "Processed") {
  process.stdout.write(response.stdout);
} else {
  process.stderr.write(`Pool submission failed: ${JSON.stringify(response)}\n`);
  process.exit(1);
}
```

#### `libs/barnum/actions/docs.ts`

JS port of `generate_step_docs` from Rust (`crates/barnum_config/src/docs.rs`). Generates the markdown instructions that pool agents receive.

```typescript
import type { ConfigFile, StepFile } from "./types.js";

/** Generate markdown documentation for a specific step. */
export function generateStepDocs(step: StepFile, config: ConfigFile): string {
  const lines: string[] = [];

  // Task isolation preamble
  lines.push(
    "**IMPORTANT: This task is completely isolated. You have no memory of previous tasks. " +
    "Even if this task seems related to prior work, you must complete it from scratch using " +
    "only the information provided here.**",
    "",
  );

  // Header
  lines.push(`# Current Step: ${step.name}`, "");

  // Step instructions (Pool actions only)
  if (step.action.kind === "Pool") {
    const instructions = (step.action as Record<string, unknown>).instructions;
    if (typeof instructions === "string" && instructions.length > 0) {
      lines.push(instructions, "");
    }
  }

  // Valid responses
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
        lines.push(
          "Accepts any JSON value.", "",
          "```json",
          `{"kind": "${nextName}", "value": <any>}`,
          "```",
        );
      } else {
        lines.push(
          "Value must match schema:", "",
          "```json",
          JSON.stringify(nextStep.value_schema, null, 2),
          "```", "",
          "Example:",
          "```json",
          `{"kind": "${nextName}", "value": {...}}`,
          "```",
        );
      }
      lines.push("");
    }
  }

  return lines.join("\n");
}
```

#### `libs/barnum/actions/index.ts`

The resolver registry. Hardcoded map from kind name to resolver function. Adding a new kind means adding an import and a map entry.

```typescript
import type { ActionResolver } from "./types.js";
import { resolve as resolvePool } from "./pool.js";
import { resolve as resolveCommand } from "./command.js";

/** Built-in action resolvers, keyed by kind name. */
const builtinResolvers = new Map<string, ActionResolver>([
  ["Pool", resolvePool],
  ["Command", resolveCommand],
]);

/**
 * Get the resolver for an action kind.
 * Throws if the kind is unknown.
 */
export function getResolver(kind: string): ActionResolver {
  const resolver = builtinResolvers.get(kind);
  if (!resolver) {
    throw new Error(
      `Unknown action kind: "${kind}". ` +
      `Built-in kinds: ${[...builtinResolvers.keys()].join(", ")}`,
    );
  }
  return resolver;
}

export type { ActionResolver, ActionContext } from "./types.js";
```

### 6. BarnumConfig.run() calls resolvers

**File:** `libs/barnum/run.ts`

```typescript
import { getResolver, type ActionContext } from "./actions/index.js";

export class BarnumConfig {
  // ...existing constructor, fromConfig...

  run(opts?: RunOptions): ChildProcess {
    const resolvedConfig = this.resolveActions();
    const args = opts?.resumeFrom
      ? ["run", "--resume-from", opts.resumeFrom]
      : ["run", "--config", JSON.stringify(resolvedConfig)];
    if (opts?.entrypointValue) args.push("--entrypoint-value", opts.entrypointValue);
    if (opts?.logLevel) args.push("--log-level", opts.logLevel);
    if (opts?.logFile) args.push("--log-file", opts.logFile);
    if (opts?.stateLog) args.push("--state-log", opts.stateLog);
    if (opts?.wake) args.push("--wake", opts.wake);
    return spawnBarnum(args);
  }

  private resolveActions() {
    return {
      ...this.config,
      steps: this.config.steps.map((step) => {
        const resolver = getResolver(step.action.kind);
        const context: ActionContext = {
          configDir: process.cwd(),
          config: this.config,
          step,
        };
        return {
          name: step.name,
          next: step.next,
          ...(step.value_schema && { value_schema: step.value_schema }),
          ...(step.options && { options: step.options }),
          ...(step.finally && { finally: step.finally }),
          script: resolver(step.action as Record<string, unknown>, context),
        };
      }),
    };
  }
}
```

### 7. Config resolution path splits

Currently `ConfigFile::resolve()` resolves `ActionFile` → `ActionKind`. With JS resolution:

**Path A: JS-driven (primary).** JS resolves actions into script strings before passing config to Rust. The config Rust receives has `script` per step instead of `action`.

**Path B: CLI-only (fallback).** When using `barnum run --config` directly (without JS), the Rust CLI resolves `Pool` and `Command` itself. For `Command`, passthrough. For `Pool`, construct the pool executor command string inline.

`ConfigFile::resolve()` handles both: it detects whether the step already has a `script` (pre-resolved by JS) or an `action` (needs resolution).

### 8. Step docs generation moves to JS

`generate_step_docs` moves to `libs/barnum/actions/docs.ts` (shown above). The pool resolver calls it at resolution time and base64-encodes the result into the executor command.

`generate_full_docs` stays in Rust for `barnum config docs`.

### 9. Troupe binary discovery

The pool resolver finds troupe via:
1. `TROUPE` env var (explicit override)
2. `require("@barnum/troupe")` (bundled binary from npm package)
3. `"troupe"` on PATH (fallback)

The resolved path is passed to the executor via `--troupe`, so the executor doesn't need to do its own discovery.

## What doesn't change

- State machine logic (`RunState`, task tracking, retry logic)
- `ActionHandle`, `run_action`, `spawn_worker` (already generic)
- `dispatch_finally` (already ShellAction with `self.working_dir`)
- `CompiledSchemas` and validation
- State log format (entries are the same; config entry stores resolved config)
- `barnum config validate`, `barnum config docs`, `barnum config graph` (operate on user-facing config)
- `ShellAction` implementation (still `sh -c <script>`)

## Resume behavior

On resume, Rust reads the state log which contains the resolved config (with `script` strings per step). The JS layer doesn't run again — Rust uses the serialized scripts directly. The command strings must still be valid at resume time (e.g., `node /path/to/pool-executor.js` requires the executor to still exist at that path). This is fine in practice since resume happens on the same machine.

## Phasing

1. **Add `script` field to resolved `Step`** alongside existing `action: ActionKind`. Dual-format acceptance.
2. **Create `libs/barnum/actions/` directory** with types.ts, command.ts, pool.ts, pool-executor.ts, docs.ts, index.ts.
3. **Update `BarnumConfig.run()`** to resolve actions via the registry before spawning barnum.
4. **Update `dispatch_task`** to use `step.script` when present, falling back to `ActionKind` match.
5. **Remove `ActionKind`**, `PoolAction` (runtime), `submit.rs`, `Invoker` from Engine/RunnerConfig.
6. **Move CLI fallback resolution** into the barnum CLI binary for direct-use case.

## Relationship to other docs

- **ACTION_REGISTRY.md** — Superseded and deleted.
- **PLUGGABLE_ACTION_KINDS.md** — The end-state vision. User-defined kinds register resolvers via the `BarnumConfig` builder's `.action(kind, resolver)` method.
- **CLAUDE_CLI_ACTION_KIND.md** — Claude becomes `libs/barnum/actions/claude.ts` exporting a `resolve` function that returns something like `"claude -p --model sonnet --output-format json"`. No Rust code needed.
