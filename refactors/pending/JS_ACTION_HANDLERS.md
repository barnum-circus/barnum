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
├── types.ts        # FollowUpTask
└── index.ts        # Public API re-exports
```

### `@barnum/barnum` actions (existing package: `libs/barnum/actions/`)

Executor and Command handler. executor.ts reads the full Rust envelope and extracts only the data each handler needs — handlers never see barnum internals like resolved options, config, or step definitions.

```
libs/barnum/actions/
├── command.ts     # Command handler
└── executor.ts    # Stdin reader, dispatcher, stdout writer
```

## `@barnum/troupe-task`

### types.ts

```typescript
/** A follow-up task returned by an action handler or troupe agent. */
export interface FollowUpTask {
  kind: string;
  value: unknown;
}
```

### docs.ts

JS port of `generate_step_docs` (`crates/barnum_config/src/docs.rs:23-90`). Takes the step name, instructions text, and valid next step names. Produces the markdown that the Pool handler sends to the agent.

```typescript
export function generateStepDocs(
  stepName: string,
  instructions: string,
  nextSteps: string[],
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

  if (nextSteps.length === 0) {
    lines.push("## Terminal Step", "", "This is a terminal step. Return an empty array: `[]`");
  } else {
    lines.push(
      "## Valid Responses", "",
      "You must return a JSON array of tasks. Each task has `kind` and `value` fields.", "",
      "Valid next steps:", "",
    );

    for (const name of nextSteps) {
      lines.push(
        `### ${name}`, "",
        "Accepts any JSON value.", "",
        "```json",
        `{"kind": "${name}", "value": <any>}`,
        "```", "",
      );
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
import type { FollowUpTask } from "./types.js";
import { submitTask } from "./submit.js";
import { generateStepDocs } from "./docs.js";

export const poolParamsSchema = z.object({
  instructions: z.string(),
  pool: z.string().nullable().optional(),
  root: z.string().nullable().optional(),
  timeout: z.number().nullable().optional(),
});

export function handlePool(ctx: {
  params: z.output<typeof poolParamsSchema>;
  task: { kind: string; value: unknown };
  nextSteps: string[];
}): FollowUpTask[] {
  const { params, task } = ctx;
  const docs = generateStepDocs(task.kind, params.instructions, ctx.nextSteps);

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
export type { FollowUpTask } from "./types.js";
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

The entry point that Rust spawns for every task. Reads the full Rust envelope from stdin and extracts only the data each handler needs. Handlers never see barnum internals (resolved options, config shape, step definitions).

```typescript
import { handlePool, poolParamsSchema } from "@barnum/troupe-task";
import { handleCommand } from "./command.js";

interface Envelope {
  action: { kind: string; params: Record<string, unknown> };
  task: { kind: string; value: unknown };
  step: { next: string[] };
  config: unknown;
}

const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope: Envelope = JSON.parse(Buffer.concat(chunks).toString());

const { action, task, step } = envelope;

let results;
switch (action.kind) {
  case "Pool": {
    const params = poolParamsSchema.parse(action.params);
    results = handlePool({ params, task, nextSteps: step.next });
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
