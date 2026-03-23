# JS Action Handlers

**Parent:** JS_ACTION_RESOLUTION.md
**Depends on:** Nothing (purely additive JS)

## Motivation

The JS executor needs handler implementations for each action kind, a type system for handler definitions, and a dispatch registry. This sub-refactor creates the entire `libs/barnum/actions/` directory. It lands independently of any Rust changes — nothing in Rust references these files yet.

## File Layout

```
libs/barnum/actions/
├── types.ts       # Type definitions: RawEnvelope, ActionContext, ActionDefinition, defineAction
├── command.ts     # Command handler
├── pool.ts        # Pool handler
├── docs.ts        # JS port of generate_step_docs
├── index.ts       # Handler registry
└── executor.ts    # Stdin reader, dispatcher, stdout writer
```

## types.ts

Defines the envelope shape (what Rust pipes to stdin), the context passed to handlers, and the action definition interface.

```typescript
import { type ZodType, type z } from "zod";
import type { ActionFile, StepFile, ConfigFile } from "../barnum-config-schema.zod.js";
export type { ActionFile, StepFile, ConfigFile };

/** A follow-up task to queue after this action completes. */
export interface FollowUpTask {
  kind: string;
  value: unknown;
}

/**
 * The raw envelope piped to the JS executor's stdin by Rust.
 *
 * Type parameters default to the concrete generated types. Handlers
 * that need tighter typing (e.g., a specific action variant) can
 * narrow them.
 */
export interface RawEnvelope<
  TAction = ActionFile,
  TTask = { kind: string; value: unknown },
  TStep = StepFile,
  TConfig = ConfigFile,
> {
  action: TAction;
  task: TTask;
  step: TStep;
  config: TConfig;
}

/**
 * Context passed to an action's handle function. The params field
 * is typed from the handler's Zod schema (if defined).
 */
export interface ActionContext<
  TParams = Record<string, unknown>,
  TTask = { kind: string; value: unknown },
  TStep = StepFile,
  TConfig = ConfigFile,
> {
  params: TParams;
  task: TTask;
  step: TStep;
  config: TConfig;
}

/**
 * An action definition: an object with a handle function and optional
 * Zod schema for params validation and type inference.
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

## command.ts

The Command handler spawns the user's shell script, piping `{ kind, value }` to stdin. The user's script receives the same format as today's Command action — backward compatible.

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

## pool.ts

The Pool handler generates agent instructions (via `docs.ts`), constructs the troupe payload, submits via the troupe CLI, and parses the response.

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

export default defineAction({
  validate: z.object({
    instructions: instructionsSchema,
    pool: z.string().nullable().optional(),
    root: z.string().nullable().optional(),
    timeout: z.number().nullable().optional(),
  }),

  handle: async ({ params, task, step, config }) => {
    const troupe = troupeBinary();

    const instructions = params.instructions.kind === "Inline"
      ? params.instructions.value
      : "";

    const docs = generateStepDocs(task.kind, instructions, step, config);

    // Build troupe payload. params.timeout is the agent timeout forwarded
    // to troupe (opaque to barnum). Barnum's worker kill deadline is
    // step.options.timeout, enforced by Rust at Boundary 2.
    const payload: Record<string, unknown> = { task, instructions: docs };
    if (params.timeout != null) {
      payload.timeout_seconds = params.timeout;
    }

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

## docs.ts

JS port of `generate_step_docs` (`crates/barnum_config/src/docs.rs:23-90`). Takes the step name, instructions text, step definition, and config. Produces the markdown that the Pool handler sends to the agent.

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

## index.ts

Hardcoded handler registry. Maps action kind names to action definitions.

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

## executor.ts

The entry point that Rust spawns for every task. Reads the envelope from stdin, validates params, dispatches to the handler, and writes follow-up tasks to stdout.

```typescript
import { getAction } from "./index.js";
import type { RawEnvelope } from "./types.js";

const chunks: Buffer[] = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const envelope: RawEnvelope = JSON.parse(Buffer.concat(chunks).toString());

const action = getAction(envelope.action.kind);

const rawParams = "params" in envelope.action ? envelope.action.params : {};
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

## package.json changes

Add `tsx` dependency and include the actions directory in the published package.

```diff
  "dependencies": {
    "zod": "^3.0.0",
+   "tsx": "^4.0.0"
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
