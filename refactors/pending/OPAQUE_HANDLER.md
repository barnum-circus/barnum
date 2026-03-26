# Opaque Handler Type

**Status:** Pending

**Depends on:** None

## Motivation

TypeScript handlers are referenced by path string in the config:

```ts
// barnum.config.ts
import { resolve } from "node:path";

BarnumConfig.fromConfig({
  steps: [{
    name: "Greet",
    action: {
      kind: "TypeScript",
      path: resolve(import.meta.dirname, "handler.ts"),
      //     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
      //     manual, error-prone, no type connection to the handler
    },
    next: ["Done"],
  }],
});
```

The handler file separately declares its types via `satisfies HandlerDefinition<C, V>`, but nothing connects those types back to the config. The path is just a string. You can point at a file that doesn't export a handler, or export the wrong shape, and it only fails at runtime when `resolveConfig` imports the module.

## Current State

### Handler definition (`libs/barnum/types.ts`)

```ts
export interface HandlerDefinition<C = unknown, V = unknown> {
  stepConfigValidator: z.ZodType<C>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;
  handle: (context: HandlerContext<C, V>) => Promise<FollowUpTask[]>;
}
```

### Handler file (e.g. `demos/typescript-handler/handler.ts`)

```ts
export default {
  stepConfigValidator,
  getStepValueValidator(_stepConfig) { return stepValueValidator; },
  async handle({ value }) {
    return [{ kind: "Done", value: { greeting: `Hello, ${value.name}!` } }];
  },
} satisfies HandlerDefinition<StepConfig, StepValue>;
```

The `satisfies` annotation is opt-in and provides no guarantees at the config level.

### Config schema (`libs/barnum/barnum-config-schema.zod.ts`)

TypeScript actions carry a path string:

```ts
z.object({
  exportedAs: z.string().optional().default("default"),
  kind: z.literal("TypeScript"),
  path: z.string(),
  stepConfig: z.any().optional().default(null),
  valueSchema: z.any().optional().default(null),
})
```

### Resolution (`libs/barnum/run.ts:116`)

`resolveConfig()` is async because it dynamically imports each handler module by path to validate schemas and generate JSON Schemas:

```ts
private async resolveConfig(): Promise<z.output<typeof configSchema>> {
  const config = structuredClone(this.config);
  for (const step of config.steps) {
    if (step.action.kind !== "TypeScript") continue;
    const mod = await import(step.action.path);
    const handler = mod[step.action.exportedAs ?? "default"];
    // validate stepConfig, generate valueSchema from Zod
  }
  return config;
}
```

### Runtime invocation

`run-handler.ts` receives `[handlerPath, exportName]` as argv, imports the module, calls `handler.handle(envelope)`, and writes the result to stdout. Rust spawns this script.

## Target Architecture

### `createHandler`

A function that wraps a `HandlerDefinition` in an opaque `Handler<C, V>` type and captures the handler file's path:

```ts
const HANDLER_BRAND = Symbol.for("barnum:handler");

class Handler<C = unknown, V = unknown> {
  readonly [HANDLER_BRAND] = true as const;
  /** @internal */ readonly __filePath: string;
  /** @internal */ readonly __definition: HandlerDefinition<C, V>;

  /** @internal */
  constructor(definition: HandlerDefinition<C, V>, filePath: string) {
    this.__definition = definition;
    this.__filePath = filePath;
  }
}

function isHandler(x: unknown): x is Handler {
  return typeof x === "object" && x !== null && HANDLER_BRAND in x;
}

export function createHandler<C, V>(
  definition: HandlerDefinition<C, V>,
): Handler<C, V> {
  const filePath = getCallerFilePath();
  return new Handler(definition, filePath);
}
```

`createHandler` returns a `Handler<C, V>` that carries the handler definition and the file path where it was created.

### File path deduction

`createHandler` captures the caller's file path from the stack trace. When called at module scope in `handler.ts`, the second stack frame is the handler file:

```ts
function getCallerFilePath(): string {
  const original = Error.prepareStackTrace;
  let callerFile: string | undefined;

  Error.prepareStackTrace = (_err, stack) => {
    // Frame 0: getCallerFilePath
    // Frame 1: createHandler
    // Frame 2: the file that called createHandler
    const frame = stack[2];
    callerFile = frame?.getFileName() ?? undefined;
    return "";
  };

  const err = new Error();
  // Trigger stack preparation
  void err.stack;
  Error.prepareStackTrace = original;

  if (!callerFile) {
    throw new Error(
      "createHandler: could not determine caller file path from stack trace. " +
      "Pass the path explicitly: createHandler(definition, { path: import.meta.filename })"
    );
  }

  // Convert file:// URL to path if needed
  if (callerFile.startsWith("file://")) {
    return fileURLToPath(callerFile);
  }
  return callerFile;
}
```

`Error.prepareStackTrace` is a V8 API (Node.js, Bun) that gives structured `CallSite` objects rather than string parsing. `CallSite.getFileName()` returns the file path or file URL.

If stack trace deduction fails, the error message tells the user to pass the path explicitly:

```ts
export function createHandler<C, V>(
  definition: HandlerDefinition<C, V>,
  opts?: { path?: string },
): Handler<C, V> {
  const filePath = opts?.path ?? getCallerFilePath();
  return new Handler(definition, filePath);
}
```

### Handler file (target)

```ts
// handler.ts
import { createHandler } from "@barnum/barnum";
import { z } from "zod";

const stepConfigValidator = z.object({});
const stepValueValidator = z.object({ name: z.string() });

export default createHandler({
  stepConfigValidator,
  getStepValueValidator: () => stepValueValidator,
  handle: async ({ value }) => {
    return [{ kind: "Done", value: { greeting: `Hello, ${value.name}!` } }];
  },
});
```

No `satisfies` needed — `createHandler` enforces the `HandlerDefinition` type at call time. The return type is `Handler<{}, { name: string }>`, inferred from the validators.

### Config (target)

```ts
// barnum.config.ts
import handler from "./handler.ts";

BarnumConfig.fromConfig({
  steps: [{
    name: "Greet",
    action: handler,  // Handler<{}, { name: string }> — type-safe, no path string
    next: ["Done"],
  }],
});
```

The `action` field accepts either a `BashAction` (inline) or a `Handler`. Raw path-based TypeScript actions are not part of the user-facing API — they exist only in the serialized config that goes to Rust. The TypeScript input type:

```ts
type ActionInput =
  | { kind: "Bash"; script: string }
  | Handler;

type StepInput = {
  name: string;
  action: ActionInput;
  stepConfig?: unknown;
  next?: string[];
  options?: StepOptionsInput;
  finally?: FinallyHookInput | null;
};

type ConfigInput = {
  entrypoint?: string | null;
  options?: OptionsInput;
  steps: StepInput[];
};
```

### `fromConfig` changes

`fromConfig` pre-processes the config before Zod validation. It walks the steps, finds Handler objects, resolves them into standard TypeScript actions, and generates JSON Schemas. Since the handler definition is already in memory (not imported by path), this is synchronous:

```ts
static fromConfig(config: ConfigInput): BarnumConfig {
  const processed = resolveHandlers(config);
  return new BarnumConfig(configSchema.parse(processed));
}

function resolveHandlers(config: ConfigInput): z.input<typeof configSchema> {
  return {
    ...config,
    steps: config.steps.map((step) => {
      if (!isHandler(step.action)) return step;

      const handler = step.action;
      const def = handler.__definition;

      // Validate step config (from step-level field)
      const parsedStepConfig = def.stepConfigValidator.parse(
        step.stepConfig ?? null,
      );

      // Generate JSON Schema from Zod value validator
      const valueValidator = def.getStepValueValidator(parsedStepConfig);
      assertSerializableZod(valueValidator, step.name);
      const valueSchema = zodToJsonSchema(valueValidator, {
        target: "jsonSchema7",
      });

      return {
        ...step,
        action: {
          kind: "TypeScript" as const,
          path: handler.__filePath,
          exportedAs: "default",
          stepConfig: parsedStepConfig,
          valueSchema,
        },
      };
    }),
  };
}
```

Handler objects are always resolved as `exportedAs: "default"`. Handlers created with `createHandler` must be default exports from their files.

### `resolveConfig` deletion

`resolveConfig` existed to dynamically import handler modules by path for validation and JSON Schema generation. With `createHandler`, the handler definition is already in memory at `fromConfig` time — there's nothing to import. `resolveConfig` is deleted. The `run()` method calls `fromConfig` (synchronous) and passes the resolved config directly to the Rust binary.

### stepConfig for Handler-based actions

Currently, `stepConfig` is passed in the config alongside `path`: `{ kind: "TypeScript", path: "...", stepConfig: { ... } }`. With Handler objects replacing the action field, stepConfig moves to a step-level field:

```ts
{
  name: "Greet",
  action: handler,
  stepConfig: { model: "gpt-4" },
  next: ["Done"],
}
```

`stepConfig` is parallel to `action`, not nested inside it. `resolveHandlers` reads `step.stepConfig`, validates it against `handler.__definition.stepConfigValidator`, and embeds the parsed value in the resolved TypeScript action.

The `StepInput` type adds the optional field:

```ts
type StepInput = Omit<z.input<typeof stepSchema>, "action"> & {
  action: z.input<typeof ActionKind> | Handler;
  stepConfig?: unknown;
};
```

For Bash actions or raw path-based TypeScript actions, `stepConfig` at the step level is ignored (those actions carry their own stepConfig internally).

### Runtime invocation (unchanged)

`run-handler.ts` still receives `[handlerPath, exportName]` as argv and imports the module. The Rust side still sees `{ kind: "TypeScript", path: "/abs/path", exportedAs: "default", ... }` in the serialized config. The Handler abstraction is JS-only.

## Before/After

### Handler file

```ts
// Before
import type { HandlerDefinition } from "@barnum/barnum";
export default {
  stepConfigValidator,
  getStepValueValidator: () => stepValueValidator,
  handle: async ({ value }) => [...],
} satisfies HandlerDefinition<StepConfig, StepValue>;

// After
import { createHandler } from "@barnum/barnum";
export default createHandler({
  stepConfigValidator,
  getStepValueValidator: () => stepValueValidator,
  handle: async ({ value }) => [...],
});
```

### Config

```ts
// Before
import { BarnumConfig } from "@barnum/barnum";
import { resolve } from "node:path";
BarnumConfig.fromConfig({
  steps: [{
    name: "Greet",
    action: {
      kind: "TypeScript",
      path: resolve(import.meta.dirname, "handler.ts"),
    },
    next: ["Done"],
  }],
});

// After
import { BarnumConfig } from "@barnum/barnum";
import handler from "./handler.ts";
BarnumConfig.fromConfig({
  steps: [{
    name: "Greet",
    action: handler,
    next: ["Done"],
  }],
});
```

## Changes Summary

| Component | Change |
|-----------|--------|
| `libs/barnum/types.ts` | Add `Handler` class, `createHandler` function, `isHandler` helper |
| `libs/barnum/run.ts` | `fromConfig` pre-processes Handlers before Zod validation; `resolveConfig` deleted |
| `libs/barnum/index.ts` | Export `createHandler`, `Handler` |
| `libs/barnum/barnum-config-schema.zod.ts` | No change (Rust schema unchanged) |
| `libs/barnum/actions/run-handler.ts` | No change |
| Rust side | No change |
| Demo configs | Update to use `createHandler` + direct import |

## Open Questions

None.
