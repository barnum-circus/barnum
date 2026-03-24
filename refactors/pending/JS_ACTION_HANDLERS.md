# JS Action Handlers

**Parent:** JS_ACTION_RESOLUTION.md
**Depends on:** Nothing (purely additive JS), but includes a Rust prerequisite (add `value_schema` back)

## Prerequisite Rust Changes

1. ~~**Always serialize `Step.next`:**~~ Done.

2. ~~**Generate resolved type schemas:**~~ Done (SCHEMA_GENERATION_CLEANUP).

3. **Add `value_schema` back to step types.** Add `value_schema: Option<serde_json::Value>` to both `StepFile` (config) and `Step` (resolved). This is the field removed in REMOVE_VALUE_SCHEMA, brought back with a clearer purpose: it holds a JSON Schema that Rust validates `task.value` against at dispatch time. The field is optional — steps without it skip validation. In the JSON config format, users write JSON Schema directly. In the JS config API, the step constructors convert Zod validators to JSON Schema via `zod-to-json-schema`. Both paths produce the same wire format.

## Motivation

The JS executor needs handler implementations for each action kind. There are two: Pool and Command. This sub-refactor creates the handler files and the `@barnum/troupe-task` package.

Troupe task submission and step docs generation are extracted into `@barnum/troupe-task` — a separate published package. This keeps `@barnum/barnum` focused on workflow orchestration and lets other tools submit troupe tasks independently.

The public API is **step constructors** — `createTroupeStep()` and `createBashStep()`. These produce step config objects with the action pre-filled. The raw handler functions (`handlePool`, `handleCommand`) are internal to the executor, not user-facing.

## Step constructors

Each constructor is generic over `V`, the task value type. The optional `validator` parameter is a Zod schema that:

1. **Compile-time**: TypeScript infers `V` from the validator, giving typed `task.value` in downstream code.
2. **Config-time**: The constructor converts the Zod schema to JSON Schema via `zod-to-json-schema` and stores it as `value_schema` on the step config.
3. **Runtime**: Rust validates `task.value` against the JSON Schema before dispatching.

Without a validator, `V` defaults to `never` and `value_schema` is omitted (no runtime validation).

### `createTroupeStep<V>`

Exported from `@barnum/troupe-task`. Creates a Pool step that sends tasks to the agent pool.

```typescript
import { z } from "zod";
import { zodToJsonSchema } from "zod-to-json-schema";
import type { StepFile } from "@barnum/barnum/barnum-config-schema.zod.js";

export function createTroupeStep<V = never>(config: {
  name: string;
  instructions: string | { kind: "Link"; path: string };
  next?: string[];
  pool?: string;
  root?: string;
  timeout?: number;
  validator?: z.ZodType<V>;
  options?: {
    timeout?: number;
    max_retries?: number;
    retry_on_timeout?: boolean;
    retry_on_invalid_response?: boolean;
  };
  finally?: { kind: "Command"; params: { script: string } };
}): StepFile {
  const instructions =
    typeof config.instructions === "string"
      ? { kind: "Inline" as const, value: config.instructions }
      : config.instructions;

  return {
    name: config.name,
    action: {
      kind: "Pool",
      params: {
        instructions,
        pool: config.pool ?? null,
        root: config.root ?? null,
        timeout: config.timeout ?? null,
      },
    },
    next: config.next ?? [],
    value_schema: config.validator
      ? zodToJsonSchema(config.validator)
      : undefined,
    options: config.options ?? {},
    finally: config.finally ?? null,
  };
}
```

Usage:

```typescript
// With validator — task.value is typed as { file: string }
createTroupeStep({
  name: "Analyze",
  instructions: "Analyze the file.",
  next: ["Implement"],
  validator: z.object({ file: z.string() }),
});

// Without validator — task.value is never (can't access properties)
createTroupeStep({
  name: "Summarize",
  instructions: { kind: "Link", path: "./summarize.md" },
  next: [],
});
```

### `createBashStep<V>`

Exported from `@barnum/barnum`. Creates a Command step that runs a local shell script.

```typescript
import { z } from "zod";
import { zodToJsonSchema } from "zod-to-json-schema";
import type { StepFile } from "./barnum-config-schema.zod.js";

export function createBashStep<V = never>(config: {
  name: string;
  script: string;
  next?: string[];
  validator?: z.ZodType<V>;
  options?: {
    timeout?: number;
    max_retries?: number;
    retry_on_timeout?: boolean;
    retry_on_invalid_response?: boolean;
  };
  finally?: { kind: "Command"; params: { script: string } };
}): StepFile {
  return {
    name: config.name,
    action: {
      kind: "Command",
      params: { script: config.script },
    },
    next: config.next ?? [],
    value_schema: config.validator
      ? zodToJsonSchema(config.validator)
      : undefined,
    options: config.options ?? {},
    finally: config.finally ?? null,
  };
}
```

### Validator semantics

The `validator` Zod schema serves two purposes from a single source of truth:

1. **TypeScript type inference** (compile-time): `V` is inferred from the validator, so downstream code that references this step's task value gets type-checked. e.g., `config.run({ entrypointValue: { file: "foo.rs" } })` type-checks against the entrypoint step's validator.

2. **Runtime validation** (Rust dispatch-time): The constructor converts the Zod schema to JSON Schema via `zod-to-json-schema` and stores it as `value_schema` on the step config. Rust validates `task.value` against this JSON Schema before dispatching. Validation failures are treated like invalid responses (retryable or fatal depending on step options).

Two entry points, same wire format:
- **JS config** (`createTroupeStep`/`createBashStep`): write Zod, auto-converted to JSON Schema.
- **JSON config** (`.json`/`.jsonc`): write JSON Schema directly in the `value_schema` field.

The Zod → JSON Schema conversion is lossless for data shapes (objects, strings, numbers, arrays, unions, optional, nullable). Zod features involving runtime logic (`transform`, `refine`, `preprocess`) are not representable in JSON Schema but would never appear in task value validators — those are plain data shapes.

## Packages

### `@barnum/troupe-task` (new package: `libs/troupe-task/`)

Troupe task submission, step docs generation, Pool handler (internal), and `createTroupeStep` constructor (public).

```
libs/troupe-task/
├── package.json
├── tsconfig.json
├── submit.ts       # submitTask(): find binary, invoke CLI, parse response
├── docs.ts         # generateStepDocs(): markdown instruction generation
├── pool.ts         # Pool handler (internal) + createTroupeStep (public)
├── types.ts        # FollowUpTask
└── index.ts        # Public API re-exports
```

### `@barnum/barnum` actions (existing package: `libs/barnum/actions/`)

Executor, Command handler (internal), and `createBashStep` constructor (public).

```
libs/barnum/actions/
├── command.ts     # Command handler (internal) + createBashStep (public)
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

The Pool action handler (internal) and `createTroupeStep` constructor (public).

The handler generates step docs from the instructions and next steps, then submits the task to the troupe agent pool.

```typescript
import { z } from "zod";
import type { FollowUpTask } from "./types.js";
import { submitTask } from "./submit.js";
import { generateStepDocs } from "./docs.js";

// --- Internal: handler used by executor ---

export function handlePool(ctx: {
  params: { instructions: string; pool?: string | null; root?: string | null; timeout?: number | null };
  task: { kind: string; value: unknown };
  step: { next: string[] };
  config: unknown;
}): FollowUpTask[] {
  const { params, task, step } = ctx;
  const docs = generateStepDocs(task.kind, params.instructions, step.next);

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

`createTroupeStep` is defined here (shown in the "Step constructors" section above) and re-exported from index.ts.

### index.ts

```typescript
// Public API
export { createTroupeStep } from "./pool.js";
export { submitTask } from "./submit.js";
export { generateStepDocs } from "./docs.js";
export type { FollowUpTask } from "./types.js";

// Internal — used by executor, not user-facing
export { handlePool } from "./pool.js";
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
    "zod": "^3.0.0",
    "zod-to-json-schema": "^3.0.0"
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

The Command handler (internal) and `createBashStep` constructor (public).

The handler spawns the user's shell script, piping `{ kind, value }` to stdin. Backward compatible with today's Command action.

```typescript
import { execSync } from "node:child_process";
import type { FollowUpTask } from "@barnum/troupe-task";

// --- Internal: handler used by executor ---

export function handleCommand(
  params: { script: string },
  task: { kind: string; value: unknown },
): FollowUpTask[] {
  const stdin = JSON.stringify(task);
  const stdout = execSync(params.script, {
    input: stdin,
    encoding: "utf-8",
    shell: "/bin/sh",
  });

  return JSON.parse(stdout);
}
```

`createBashStep` is defined here (shown in the "Step constructors" section above) and exported from `@barnum/barnum`'s main entry point.

### executor.ts

The entry point that Rust spawns for every task. Reads the Rust envelope from stdin, dispatches to the correct handler based on action kind. The envelope's `step` and `config` are passed through to handlers as-is.

The envelope type is composed from individual TypeScript types exported by `barnum-resolved-schema.zod.ts` (generated from Rust resolved types by `build_schemas`).

```typescript
import { handlePool } from "@barnum/troupe-task";
import { handleCommand } from "./command.js";
import type { ActionKind, Task, Step, Config } from "../barnum-resolved-schema.zod.js";

interface Envelope {
  action: ActionKind;
  task: Task;
  step: Step;
  config: Config;
}

const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope: Envelope = JSON.parse(Buffer.concat(chunks).toString());

const { action, task, step, config } = envelope;

let results;
switch (action.kind) {
  case "Pool": {
    results = handlePool({ params: action.params, task, step, config });
    break;
  }
  case "Command": {
    results = handleCommand(action.params, task);
    break;
  }
  default:
    throw new Error(`Unknown action kind: "${action.kind}". Built-in kinds: Pool, Command`);
}

process.stdout.write(JSON.stringify(results));
```

Note: the executor no longer does Zod `.parse()` on action params — the params come from Rust (trusted), and the handler types match the generated `ActionKind` discriminated union. The `action.params` type narrows correctly in each `case` branch via TypeScript's discriminated union narrowing.

## package.json changes (`@barnum/barnum`)

Add `tsx` and `@barnum/troupe-task` dependencies, include the actions directory in the published package.

```diff
  "dependencies": {
    "zod": "^3.0.0",
+   "zod-to-json-schema": "^3.0.0",
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
+   "barnum-resolved-schema.zod.ts",
    "run.ts"
  ],
```

## Testing

- `pnpm typecheck` validates all TypeScript types compile.
- Manual test: write a small script that imports executor.ts, feeds it a mock envelope on stdin, and verifies the output. This exercises the full handler pipeline without Rust.
- Integration test: `barnum run --config ... --executor "npx tsx actions/executor.ts"` for a full round-trip.
