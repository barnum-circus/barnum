# Add Handler Validation

**Parent:** TS_CONFIG.md
**Depends on:** ADD_RUN_HANDLER

## Motivation

TypeScript handlers declare Zod schemas for their inputs. These schemas serve two purposes:
1. Type safety for handler authors (TypeScript inference from Zod)
2. Runtime validation of step outputs by Rust (via JSON Schema)

Validation does **not** happen in `run-handler.ts`. The handler subprocess just calls the function — it assumes all inputs are valid. Instead:
- **JS validates step config** at `run()` time (before the Rust binary starts)
- **JS converts Zod → JSON Schema** and embeds it in the serialized config
- **Rust validates step outputs** at runtime using the embedded JSON Schema

## Handler interface

A handler exports an object with three parts:

```typescript
import { z } from "zod";
import type { HandlerDefinition } from "@barnum/barnum";

const stepConfigValidator = z.object({
  instructions: z.string(),
});

type StepConfig = z.infer<typeof stepConfigValidator>;

const stepValueValidator = z.object({ file: z.string() });

type StepValue = z.infer<typeof stepValueValidator>;

export default {
  stepConfigValidator,

  getStepValueValidator(_stepConfig) {
    return stepValueValidator;
  },

  async handle({ stepConfig, value, config, stepName }) {
    return [{ kind: "Implement", value: { plan: "..." } }];
  },
} satisfies HandlerDefinition<StepConfig, StepValue>;
```

- **`stepConfigValidator`**: Zod schema for `stepConfig`. Validated in JS at `run()` time.
- **`getStepValueValidator(stepConfig)`**: Returns a Zod schema for the step's input value. Called at `run()` time after step config is validated. The returned schema is converted to JSON Schema and sent to Rust.
- **`handle`**: Called at runtime. Receives validated inputs, returns follow-up tasks.

## JS-side: config construction (`run.ts`)

When `BarnumConfig.run()` is called, before serializing the config to Rust:

For each TypeScript step in the config:

1. **Import the handler module** (`await import(path)`)
2. **Validate step config** against `handler.stepConfigValidator.parse(step.stepConfig)`
3. **Get the value validator** via `handler.getStepValueValidator(stepConfig)`
4. **Reject non-serializable Zod features** — verify the schema only uses types representable as JSON Schema (no `.transform()`, `.preprocess()`, `.refine()`, `.pipe()`, `.brand()`, etc.)
5. **Convert Zod → JSON Schema** (using `zod-to-json-schema` or similar)
6. **Embed the JSON Schema** in the serialized config as `valueSchema` on the step's action

The serialized config sent to Rust looks like:

```json
{
  "steps": [{
    "name": "Analyze",
    "action": {
      "kind": "TypeScript",
      "path": "/abs/path/to/handler.ts",
      "exportedAs": "default",
      "stepConfig": { "instructions": "analyze the file" },
      "valueSchema": {
        "type": "object",
        "properties": { "file": { "type": "string" } },
        "required": ["file"]
      }
    },
    "next": ["Implement"]
  }]
}
```

If step config validation fails or the Zod schema uses non-serializable features, `run()` throws before the Rust binary starts.

## Rust-side: output validation

**File:** `crates/barnum_config/src/config.rs`

Add `value_schema` to `TypeScriptAction`:

```rust
pub struct TypeScriptAction {
    pub path: String,
    #[serde(default = "default_exported_as")]
    pub exported_as: String,
    #[serde(default)]
    pub step_config: serde_json::Value,
    /// JSON Schema for this step's input value. Produced by JS from Zod.
    /// Used to validate transition values targeting this step.
    #[serde(default)]
    pub value_schema: Option<serde_json::Value>,
}
```

**File:** `crates/barnum_config/src/runner/mod.rs`

When Rust processes a handler's response (`convert_task_result`), it validates each follow-up task:
- Look up the target step by `kind`
- If the target step has a `value_schema`, validate the transition's `value` against it
- Invalid values → treat as invalid response (retry per config)

Use a JSON Schema validation crate (e.g., `jsonschema`) on the Rust side.

## Zod subset restriction

Not all Zod features can be represented as JSON Schema. The JS side must reject schemas that use:
- `.transform()` — arbitrary code, not representable
- `.preprocess()` — same
- `.refine()` / `.superRefine()` — arbitrary predicates
- `.pipe()` — chained transforms
- `.brand()` — TypeScript-only, no runtime meaning

The check runs at `run()` time. If a handler uses unsupported features, throw with a clear error message listing the step name and the unsupported Zod method.

Supported Zod types (representable as JSON Schema):
- `z.string()`, `z.number()`, `z.boolean()`, `z.null()`
- `z.object()`, `z.array()`, `z.tuple()`
- `z.union()`, `z.discriminatedUnion()`, `z.intersection()`
- `z.literal()`, `z.enum()`, `z.nativeEnum()`
- `z.optional()`, `z.nullable()`, `z.default()`
- `z.record()`, `z.map()`
- `z.unknown()`, `z.any()`
- String/number refinements (`.min()`, `.max()`, `.regex()`, `.int()`, etc.) — these map to JSON Schema keywords

## TypeScript types

```typescript
interface HandlerDefinition<C = unknown, V = unknown> {
  stepConfigValidator: z.ZodType<C>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;
  handle: (context: HandlerContext<C, V>) => Promise<FollowUpTask[]>;
}

interface HandlerContext<C, V> {
  stepConfig: C;
  value: V;
  config: unknown;
  stepName: string;
}

interface FollowUpTask {
  kind: string;
  value: unknown;
}
```

## What this does NOT do

- Does not change run-handler.ts (stays a dumb pipe)
- Does not validate Bash action outputs (no schema to validate against)
- Does not implement `.validate()` on `BarnumConfig` (that's a separate convenience method)
