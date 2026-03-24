# Add Handler Validation Types

**Parent:** TS_CONFIG.md
**Depends on:** ADD_TYPESCRIPT_DISPATCH

## Motivation

TypeScript handlers need a well-defined interface: what they receive, how inputs are validated, and what they return. This refactor defines the handler type contract and validation flow. Both validators (`stepConfigValidator` and `getStepValueValidator`) are required — every handler must declare the shape of its inputs.

## Handler interface

A TypeScript handler module exports a `HandlerDefinition` — an object with three parts:

1. **`stepConfigValidator`**: A Zod schema that validates `stepConfig` from the envelope. This is the step-specific configuration from the config file, which Rust passes through as opaque JSON.

2. **`getStepValueValidator`**: A function that receives the validated step config and returns a Zod schema for the task value. The value schema can depend on the step configuration (e.g., different fields based on config options). Called per-task.

3. **`handle`**: Takes a single `HandlerContext` object. Returns follow-up tasks.

```typescript
// handlers/analyze.ts
import { z } from "zod";
import type { HandlerDefinition } from "@barnum/barnum";

export default {
  stepConfigValidator: z.object({
    instructions: z.string(),
    pool: z.string(),
  }),

  getStepValueValidator(stepConfig) {
    return z.object({ file: z.string() });
  },

  async handle({ stepConfig, value, config, stepName }) {
    stepConfig.instructions; // string — typed by stepConfigValidator
    value.file;              // string — typed by getStepValueValidator
    config;                  // full resolved Barnum config
    stepName;                // "Analyze"
    return [{ kind: "Implement", value: { plan: "..." } }];
  },
} satisfies HandlerDefinition;
```

### Types

**File:** `libs/barnum/types.ts` (or appropriate location in the `@barnum/barnum` package)

```typescript
interface HandlerDefinition<C, V> {
  /** Validates stepConfig from the envelope. */
  stepConfigValidator: z.ZodType<C>;

  /** Returns a validator for the task value, given the validated step config. */
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;

  /** Process the task. Returns follow-up tasks. */
  handle: (context: HandlerContext<C, V>) => Promise<FollowUpTask[]>;
}

interface HandlerContext<C, V> {
  /** The validated step configuration. */
  stepConfig: C;
  /** The validated task value. */
  value: V;
  /** The full resolved Barnum config. */
  config: Config;
  /** The name of the step this handler is processing. */
  stepName: string;
}

/** A follow-up task to spawn. */
interface FollowUpTask {
  /** Step name — must be one of this step's `next` entries. */
  kind: string;
  /** Task payload — opaque to the framework. */
  value: unknown;
}
```

Both `C` and `V` have no default type parameters — they're determined by the validators. The `satisfies HandlerDefinition` on the export object infers `C` from `stepConfigValidator` and `V` from the return type of `getStepValueValidator`, giving full type inference in `handle`.

### Validation flow

The validation flow runs in `run-handler.ts` (defined in ADD_RUN_HANDLER) before calling `handle`:

1. Parse `envelope.stepConfig` through `definition.stepConfigValidator` to get validated step config `C`.
2. Call `definition.getStepValueValidator(stepConfig)` to get the value schema, then parse `envelope.value` to get validated value `V`.
3. Call `definition.handle({ stepConfig, value, config: envelope.config, stepName: envelope.stepName })`.

If either validator rejects its input, the process exits with a non-zero code and the Zod error on stderr. Rust treats this as a failed action (same as any other subprocess failure).

### Type safety boundary

Inputs (`stepConfig` and `value`) are fully typed via the required Zod validators. The output (`FollowUpTask[]`) has typed `kind` and untyped `value` — which steps a handler can transition to is determined by the config's `next` array, and the handler has no compile-time knowledge of that. Invalid transitions are caught at runtime by Rust's response validator.

### Handlers with trivial inputs

A handler that doesn't need step config or has no meaningful value schema still declares both validators:

```typescript
export default {
  stepConfigValidator: z.object({}),
  getStepValueValidator() {
    return z.unknown();
  },
  async handle({ value }) {
    return [];
  },
} satisfies HandlerDefinition;
```

## What this does NOT do

- Does not implement run-handler.ts (ADD_RUN_HANDLER)
- Does not implement step constructors (`createTroupeStep`, `createBashStep`)
- Does not implement `.validate()` on `BarnumConfig`
