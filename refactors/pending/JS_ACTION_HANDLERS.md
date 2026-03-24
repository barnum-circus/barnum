# JS Action Handlers

**Parent:** JS_ACTION_RESOLUTION.md
**Depends on:** Nothing (purely additive JS)

## Prerequisite Rust Change

Remove `skip_serializing_if = "Vec::is_empty"` from `Step.next` in `crates/barnum_config/src/resolved.rs:53`. The `next` field should always be serialized, even when empty. This keeps the JS types simple — `next` is always `string[]`, never optional.

## Motivation

The JS executor needs handler implementations for each action kind. There are two: Pool and Command. This sub-refactor creates the handler files and the `@barnum/troupe-task` package.

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
├── pool.ts         # Pool action handler
├── types.ts        # SubmitOptions, ResolvedStep, ResolvedConfig, FollowUpTask
└── index.ts        # Public API re-exports
```

### `@barnum/barnum` actions (existing package: `libs/barnum/actions/`)

Executor and Command handler. The executor reads the envelope from stdin, dispatches to the correct handler based on action kind, and writes follow-up tasks to stdout.

```
libs/barnum/actions/
├── types.ts       # RawEnvelope
├── command.ts     # Command handler
└── executor.ts    # Stdin reader, dispatcher, stdout writer
```

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

Finds the troupe binary, invokes `troupe submit_task`, and parses the response. The `data` argument is sent as-is — the caller builds the complete payload (including `timeout_seconds` if needed).

```typescript
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import type { FollowUpTask } from "./types.js";

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
 *
 * The `data` payload is sent as-is to the troupe CLI. The caller is
 * responsible for building the complete payload shape (task, instructions,
 * timeout_seconds, etc.).
 */
export function submitTask(
  data: unknown,
  options?: { pool?: string; root?: string },
): FollowUpTask[] {
  const troupe = troupeBinary();
  const args = ["submit_task"];

  if (options?.root) args.push("--root", options.root);
  if (options?.pool) args.push("--pool", options.pool);
  args.push("--notify", "file", "--data", JSON.stringify(data));

  const result = execFileSync(troupe, args, { encoding: "utf-8" });
  const response = JSON.parse(result);

  if (response.kind === "Processed") {
    return JSON.parse(response.stdout);
  }

  throw new Error(`Pool submission failed: ${JSON.stringify(response)}`);
}
```

### pool.ts

The Pool action handler. Generates agent instructions, constructs the troupe payload, and submits.

```typescript
import { z } from "zod";
import type { ResolvedStep, ResolvedConfig, FollowUpTask } from "./types.js";
import { submitTask } from "./submit.js";
import { generateStepDocs } from "./docs.js";

export const poolParamsSchema = z.object({
  instructions: z.string(),
  pool: z.string().nullable().optional(),
  root: z.string().nullable().optional(),
  timeout: z.number().nullable().optional(),
});

export type PoolParams = z.output<typeof poolParamsSchema>;

export async function handlePool(ctx: {
  params: PoolParams;
  task: { kind: string; value: unknown };
  step: ResolvedStep;
  config: ResolvedConfig;
}): Promise<FollowUpTask[]> {
  const { params, task, step, config } = ctx;
  const docs = generateStepDocs(task.kind, params.instructions, step, config);

  const payload: Record<string, unknown> = { task, instructions: docs };
  if (params.timeout != null) {
    payload.timeout_seconds = params.timeout;
  }

  return submitTask(payload, {
    pool: params.pool ?? undefined,
    root: params.root ?? undefined,
  });
}
```

Note: `params.instructions` is a plain `string` — Rust resolves `MaybeLinked` (Inline/Link) to a string during `ConfigFile::resolve()`.

### index.ts

```typescript
export { submitTask } from "./submit.js";
export { generateStepDocs } from "./docs.js";
export { handlePool, poolParamsSchema } from "./pool.js";
export type { PoolParams } from "./pool.js";
export type {
  FollowUpTask,
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

The envelope type and re-exports of resolved types from `@barnum/troupe-task`.

```typescript
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
```

### command.ts

The Command handler spawns the user's shell script, piping `{ kind, value }` to stdin. Backward compatible with today's Command action.

```typescript
import { execSync } from "node:child_process";
import type { FollowUpTask } from "@barnum/troupe-task";

export function handleCommand(
  script: string,
  task: { kind: string; value: unknown },
): FollowUpTask[] {
  const stdin = JSON.stringify(task);
  const stdout = execSync(script, {
    input: stdin,
    encoding: "utf-8",
    shell: "/bin/sh",
  });

  return JSON.parse(stdout);
}
```

### executor.ts

The entry point that Rust spawns for every task. Reads the envelope from stdin, dispatches to the correct handler based on action kind, and writes follow-up tasks to stdout.

```typescript
import { handlePool, poolParamsSchema } from "@barnum/troupe-task";
import { handleCommand } from "./command.js";
import type { RawEnvelope } from "./types.js";

const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope: RawEnvelope = JSON.parse(Buffer.concat(chunks).toString());

const { action, task, step, config } = envelope;

let results;
switch (action.kind) {
  case "Pool": {
    const params = poolParamsSchema.parse(action.params);
    results = await handlePool({ params, task, step, config });
    break;
  }
  case "Command": {
    const script = action.params.script;
    if (typeof script !== "string") {
      throw new Error(`Command action requires a "script" string param, got: ${typeof script}`);
    }
    results = handleCommand(script, task);
    break;
  }
  default:
    throw new Error(`Unknown action kind: "${action.kind}". Built-in kinds: Pool, Command`);
}

process.stdout.write(JSON.stringify(results));
```

## package.json changes (`@barnum/barnum`)

Add `tsx` and `@barnum/troupe-task` dependencies, include the actions directory in the published package.

```diff
  "dependencies": {
    "zod": "^3.0.0",
+   "tsx": "^4.0.0",
+   "@barnum/troupe-task": "workspace:*"
  },
```

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

## Testing

- `pnpm typecheck` validates all TypeScript types compile.
- Manual test: write a small script that imports executor.ts, feeds it a mock envelope on stdin, and verifies the output. This exercises the full handler pipeline without Rust.
- Integration test: `barnum run --config ... --executor "npx tsx actions/executor.ts"` for a full round-trip.
