# JS Action Handlers

**Parent:** JS_ACTION_RESOLUTION.md
**Depends on:** Nothing (purely additive JS)

## Motivation

The JS executor needs handler implementations for each action kind, a type system for handler definitions, and a dispatch registry. This sub-refactor creates the `libs/barnum/actions/` directory and the `libs/troupe-task/` package.

Troupe task submission and step docs generation are extracted into `@barnum/troupe-task` — a separate published package. This keeps `@barnum/barnum` focused on workflow orchestration and lets other tools submit troupe tasks independently.

## Packages

### `@barnum/troupe-task` (new package: `libs/troupe-task/`)

Troupe task submission, step docs generation, and the Pool action handler.

```
libs/troupe-task/
├── package.json
├── tsconfig.json
├── submit.ts       # submitTask(): find binary, invoke CLI, parse response
├── docs.ts         # generateStepDocs(): markdown instruction generation
├── pool.ts         # Pool action handler (validate schema + handle function)
├── types.ts        # SubmitOptions, ResolvedStep, ResolvedConfig, FollowUpTask
└── index.ts        # Public API re-exports
```

### `@barnum/barnum` actions (existing package: `libs/barnum/actions/`)

Executor, registry, command handler, and action definition types. The pool handler is imported from `@barnum/troupe-task`.

```
libs/barnum/actions/
├── types.ts       # RawEnvelope, ActionContext, ActionDefinition, defineAction
├── command.ts     # Command handler
├── index.ts       # Handler registry (imports pool from @barnum/troupe-task)
└── executor.ts    # Stdin reader, dispatcher, stdout writer
```

## Stdin/Stdout Contract

### executor.ts (Boundary 2: Rust → JS)

**stdin:** A single JSON object — the enriched envelope from Rust. Read all of stdin, parse as JSON.

```json
{
  "action": { "kind": "Pool", "params": { "instructions": "...", "pool": "demo", "timeout": 300 } },
  "task": { "kind": "Analyze", "value": { "file": "src/main.rs" } },
  "step": { "name": "Analyze", "action": {...}, "next": ["Implement", "Done"], "options": {...} },
  "config": { "steps": [...], "max_concurrency": 10 }
}
```

**stdout:** A JSON array of follow-up tasks. Each element has `kind` (string) and `value` (any JSON).

```json
[{"kind": "Implement", "value": {"file": "src/main.rs", "plan": "..."}}]
```

**stderr:** Diagnostic output. Rust captures stderr and includes it in error messages on non-zero exit.

**exit code:** 0 on success. Non-zero on failure — Rust treats this as an action error and may retry per step options.

### command handler (Boundary 3: JS → user script)

**stdin to user script:** `{ "kind": "<step_name>", "value": <payload> }` — backward compatible with today's Command action.

**stdout from user script:** JSON array of follow-up tasks (same format as executor stdout).

## Resolved Types

The envelope contains Rust's **resolved** types, NOT the config file types from the generated Zod schema. Key differences:

| Field | Config file type (`*File`) | Resolved type (envelope) |
|-------|---------------------------|-------------------------|
| `PoolAction.instructions` | `MaybeLinked` (Inline/Link union) | `string` (already resolved) |
| `Step.options` | `StepOptions` (all nullable) | `Options` (concrete values, merged with global) |
| `Config` | Has `$schema`, `entrypoint`, `options` | Only `max_concurrency` + `steps` |
| `Step.finally` | `FinallyHook \| null` | `string \| undefined` (just the script) |
| `Options.max_retries` | `number \| null` (default 0) | `number` (always present) |
| `Options.retry_on_timeout` | `boolean \| null` (default true) | `boolean` (always present) |

types.ts defines these resolved shapes directly instead of importing from the generated schema.

## `@barnum/troupe-task`

### types.ts

```typescript
/** A follow-up task returned by an action handler or troupe agent. */
export interface FollowUpTask {
  kind: string;
  value: unknown;
}

/** Options for submitting a task to a troupe pool. */
export interface SubmitOptions {
  /** Pool name. If omitted, troupe uses its default. */
  pool?: string;
  /** Pool root directory. If omitted, troupe uses its default. */
  root?: string;
  /** Agent timeout in seconds. Forwarded to troupe as timeout_seconds. */
  timeout?: number;
}

/** A resolved step definition (from the Rust envelope). */
export interface ResolvedStep {
  name: string;
  value_schema?: unknown;
  action: { kind: string; params: Record<string, unknown> };
  next: string[];
  finally?: string;
  options: ResolvedOptions;
}

/** Resolved runtime options (all fields concrete, no nulls). */
export interface ResolvedOptions {
  timeout?: number;
  max_retries: number;
  retry_on_timeout: boolean;
  retry_on_invalid_response: boolean;
}

/** A resolved config (from the Rust envelope). */
export interface ResolvedConfig {
  max_concurrency?: number;
  steps: ResolvedStep[];
}
```

### docs.ts

JS port of `generate_step_docs` (`crates/barnum_config/src/docs.rs:23-90`). Takes the step name, instructions text, step definition, and config. Produces the markdown that the Pool handler sends to the agent.

```typescript
import type { ResolvedStep, ResolvedConfig } from "./types.js";

export function generateStepDocs(
  stepName: string,
  instructions: string,
  step: ResolvedStep,
  config: ResolvedConfig,
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

### submit.ts

Finds the troupe binary, invokes `troupe submit_task`, and parses the response.

```typescript
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import type { SubmitOptions, FollowUpTask } from "./types.js";

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
 * Submit a task to a troupe pool and return the follow-up tasks.
 *
 * Calls `troupe submit_task` with --notify file, waits for the response,
 * and parses the agent's stdout as a JSON array of follow-up tasks.
 */
export function submitTask(
  data: unknown,
  options?: SubmitOptions,
): FollowUpTask[] {
  const troupe = troupeBinary();
  const args = ["submit_task"];

  if (options?.root) args.push("--root", options.root);
  if (options?.pool) args.push("--pool", options.pool);
  if (options?.timeout != null) {
    // Inject timeout_seconds into the data payload
    const payload = typeof data === "object" && data !== null
      ? { ...data, timeout_seconds: options.timeout }
      : data;
    args.push("--notify", "file", "--data", JSON.stringify(payload));
  } else {
    args.push("--notify", "file", "--data", JSON.stringify(data));
  }

  const result = execFileSync(troupe, args, { encoding: "utf-8" });
  const response = JSON.parse(result);

  if (response.kind === "Processed") {
    return JSON.parse(response.stdout);
  }

  throw new Error(`Pool submission failed: ${JSON.stringify(response)}`);
}
```

### pool.ts

The Pool action handler. Generates agent instructions, constructs the troupe payload, and submits. Exported as a plain object with `validate` (Zod schema) and `handle` (async function) — structurally compatible with `ActionDefinition` in `@barnum/barnum` without importing it (no circular dependency).

```typescript
import { z } from "zod";
import type { ResolvedStep, ResolvedConfig, FollowUpTask } from "./types.js";
import { submitTask } from "./submit.js";
import { generateStepDocs } from "./docs.js";

const validate = z.object({
  instructions: z.string(),
  pool: z.string().nullable().optional(),
  root: z.string().nullable().optional(),
  timeout: z.number().nullable().optional(),
});

type Params = z.output<typeof validate>;

async function handle(ctx: {
  params: Params;
  task: { kind: string; value: unknown };
  step: ResolvedStep;
  config: ResolvedConfig;
}): Promise<FollowUpTask[]> {
  const { params, task, step, config } = ctx;
  const docs = generateStepDocs(task.kind, params.instructions, step, config);
  const payload = { task, instructions: docs };

  return submitTask(payload, {
    pool: params.pool ?? undefined,
    root: params.root ?? undefined,
    timeout: params.timeout ?? undefined,
  });
}

export const poolAction = { validate, handle };
```

Note: `params.instructions` is a plain `string` — Rust resolves `MaybeLinked` (Inline/Link) to a string during `ConfigFile::resolve()`.

### index.ts

```typescript
export { submitTask } from "./submit.js";
export { generateStepDocs } from "./docs.js";
export { poolAction } from "./pool.js";
export type {
  FollowUpTask,
  SubmitOptions,
  ResolvedStep,
  ResolvedOptions,
  ResolvedConfig,
} from "./types.js";
```

### package.json

```json
{
  "name": "@barnum/troupe-task",
  "version": "0.2.4",
  "type": "module",
  "description": "Submit tasks to troupe agent pools and generate step documentation.",
  "main": "index.ts",
  "exports": {
    ".": "./index.ts"
  },
  "dependencies": {
    "@barnum/troupe": "workspace:*",
    "zod": "^3.0.0"
  },
  "files": [
    "*.ts"
  ],
  "author": "Robert Balicki",
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "git+https://github.com/barnum-circus/barnum.git"
  },
  "publishConfig": {
    "access": "public"
  }
}
```

## `@barnum/barnum` actions

### types.ts

Defines the envelope shape, handler context, and action definition interface. Uses resolved types from `@barnum/troupe-task` for step/config, defines its own envelope and handler types.

```typescript
import { type ZodType, type z } from "zod";
import type { ResolvedStep, ResolvedConfig, FollowUpTask } from "@barnum/troupe-task";
export type { FollowUpTask, ResolvedStep, ResolvedConfig };

/**
 * The raw envelope piped to the JS executor's stdin by Rust.
 *
 * Fields are Rust's resolved types, not config file types.
 */
export interface RawEnvelope {
  action: { kind: string; params: Record<string, unknown> };
  task: { kind: string; value: unknown };
  step: ResolvedStep;
  config: ResolvedConfig;
}

/**
 * Context passed to an action's handle function.
 *
 * The params field is typed from the handler's Zod schema (if defined).
 */
export interface ActionContext<TParams = Record<string, unknown>> {
  params: TParams;
  task: { kind: string; value: unknown };
  step: ResolvedStep;
  config: ResolvedConfig;
}

/**
 * An action definition: a handle function with an optional Zod schema
 * for params validation and type inference.
 *
 * The Zod schema does double duty:
 * 1. Runtime validation of action params before handle() is called.
 * 2. TypeScript type inference — params in handle() is typed as z.output<validate>.
 */
export interface ActionDefinition<TParams = Record<string, unknown>> {
  validate?: ZodType<TParams>;
  handle: (ctx: ActionContext<TParams>) => Promise<FollowUpTask[]>;
}

/**
 * Define an action with full type inference from the Zod schema.
 *
 * With validate:
 *   defineAction({
 *     validate: z.object({ script: z.string() }),
 *     handle: async ({ params }) => { ... }  // params is { script: string }
 *   })
 *
 * Without validate:
 *   defineAction({
 *     handle: async ({ params }) => { ... }  // params is Record<string, unknown>
 *   })
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

### command.ts

The Command handler spawns the user's shell script, piping `{ kind, value }` to stdin. Backward compatible with today's Command action.

```typescript
import { execSync } from "node:child_process";
import { z } from "zod";
import { defineAction } from "./types.js";

export default defineAction({
  validate: z.object({ script: z.string() }),

  handle: async ({ params, task }) => {
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

### index.ts

Hardcoded handler registry. Maps action kind names to action definitions.

```typescript
import type { ActionDefinition } from "./types.js";
import { poolAction } from "@barnum/troupe-task";
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

### executor.ts

The entry point that Rust spawns for every task. Reads the envelope from stdin, validates params, dispatches to the handler, and writes follow-up tasks to stdout.

```typescript
import { getAction } from "./index.js";
import type { RawEnvelope } from "./types.js";

const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope: RawEnvelope = JSON.parse(Buffer.concat(chunks).toString());

const action = getAction(envelope.action.kind);

const rawParams = envelope.action.params ?? {};
const params = action.validate
  ? action.validate.parse(rawParams)
  : rawParams;

const results = await action.handle({
  params,
  task: envelope.task,
  step: envelope.step,
  config: envelope.config,
});

process.stdout.write(JSON.stringify(results));
```

## package.json changes (`@barnum/barnum`)

Add `tsx` and `@barnum/troupe-task` dependencies, include the actions directory in the published package.

```diff
  "dependencies": {
    "zod": "^3.0.0",
+   "tsx": "^4.0.0",
+   "@barnum/troupe-task": "workspace:*",
+   "@barnum/troupe": "workspace:*"
  },
```

`@barnum/troupe` is needed because `@barnum/troupe-task` uses `require("@barnum/troupe")` to find the troupe binary.

```diff
  "files": [
    "index.ts",
    "index.cjs",
    "cli.cjs",
    "artifacts/**/*",
+   "actions/**/*",
    "barnum-config-schema.json",
    "barnum-config-schema.zod.ts",
    "barnum-cli-schema.zod.ts",
    "run.ts"
  ],
```

Add an export for the actions module so users can import `defineAction` and the types:

```diff
  "exports": {
    ".": "./index.ts",
    "./schema": "./barnum-config-schema.zod.ts",
    "./cli-schema": "./barnum-cli-schema.zod.ts",
-   "./binary": "./index.cjs"
+   "./binary": "./index.cjs",
+   "./actions": "./actions/index.ts"
  },
```

## Testing

- `pnpm typecheck` validates all TypeScript types compile.
- Manual test: write a small script that imports executor.ts, feeds it a mock envelope on stdin, and verifies the output. This exercises the full handler pipeline without Rust.
- Once EXECUTOR_CLI_FLAG lands, integration test: `barnum run --config ... --executor "npx tsx actions/executor.ts"`.
