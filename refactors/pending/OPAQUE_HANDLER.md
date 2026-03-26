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

The `action` field accepts either a standard `ActionKind` (for Bash or raw TypeScript path references) or a `Handler`. The TypeScript input type for the config widens the `action` field:

```ts
type StepInput = Omit<z.input<typeof stepSchema>, "action"> & {
  action: z.input<typeof ActionKind> | Handler;
};

type ConfigInput = Omit<z.input<typeof configSchema>, "steps"> & {
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

      // Validate step config
      const parsedStepConfig = def.stepConfigValidator.parse(
        null, // no stepConfig for Handler-based actions (or extend Handler to carry it)
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

Handler objects are always resolved as `exportedAs: "default"`. Handlers created with `createHandler` should be default exports from their files. Named exports use the raw `{ kind: "TypeScript", path: "...", exportedAs: "myExport" }` syntax.

### `resolveConfig` simplification

If all TypeScript actions use `createHandler`, `resolveConfig` has nothing to do — handlers were already resolved in `fromConfig`. For backwards compatibility with raw path-based TypeScript actions, `resolveConfig` still handles those:

```ts
private async resolveConfig(): Promise<z.output<typeof configSchema>> {
  const config = structuredClone(this.config);
  for (const step of config.steps) {
    if (step.action.kind !== "TypeScript") continue;
    // Handler-based actions already have valueSchema set by fromConfig.
    // Only process raw path-based actions that still need resolution.
    if (step.action.valueSchema != null) continue;

    const mod = await import(step.action.path);
    const handler = mod[step.action.exportedAs ?? "default"];
    // ... existing validation and schema generation ...
  }
  return config;
}
```

### stepConfig for Handler-based actions

Currently, `stepConfig` is passed in the config: `{ kind: "TypeScript", path: "...", stepConfig: { ... } }`. With Handler objects, stepConfig needs a different mechanism since the action field is just the Handler.

Two options:

**Option A: Step-level field.** Add `stepConfig` as a field on the step itself (parallel to `action`):

```ts
{
  name: "Greet",
  action: handler,
  stepConfig: { model: "gpt-4" },
  next: ["Done"],
}
```

**Option B: Handler carries stepConfig.** The Handler has a `.with(stepConfig)` method that returns a new Handler with the config baked in:

```ts
{
  name: "Greet",
  action: handler.with({ model: "gpt-4" }),
  next: ["Done"],
}
```

Option B is more ergonomic and keeps handler-specific data together. `with` returns a new Handler with the same definition and file path but a different stepConfig. The type parameter `C` constrains what `with` accepts:

```ts
class Handler<C = unknown, V = unknown> {
  // ...
  with(stepConfig: C): Handler<C, V> {
    return new Handler(this.__definition, this.__filePath, stepConfig);
  }
}
```

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
| `libs/barnum/run.ts` | `fromConfig` pre-processes Handlers before Zod validation; `resolveConfig` skips already-resolved actions |
| `libs/barnum/index.ts` | Export `createHandler`, `Handler` |
| `libs/barnum/barnum-config-schema.zod.ts` | No change (Rust schema unchanged) |
| `libs/barnum/actions/run-handler.ts` | No change |
| Rust side | No change |
| Demo configs | Update to use `createHandler` + direct import |

## Open Questions

1. **stepConfig mechanism**: Option A (step-level field) vs Option B (`handler.with(stepConfig)`). Option B is more ergonomic but adds a method to the opaque type. Recommendation: Option B.

2. **Bun/Deno compatibility**: `Error.prepareStackTrace` is a V8 API. Bun supports it (V8-compatible). Deno supports it. If a runtime doesn't support it, `createHandler` falls back to string-parsing `Error().stack` or requires explicit `{ path }`.

3. **Future type-safe step graph**: With Handler type parameters, we could eventually type-check that `entrypointValue` matches the entrypoint handler's value type, and that follow-up tasks' values match their target handlers' types. This requires a more sophisticated generic config type and is out of scope here but becomes possible with this foundation.

4. **Deprecating raw path strings**: Should raw `{ kind: "TypeScript", path: "..." }` be deprecated in the JS API? It's still needed for the serialized config (Rust side), but users could be steered toward `createHandler` exclusively. Recommendation: keep raw paths supported but undocumented.
