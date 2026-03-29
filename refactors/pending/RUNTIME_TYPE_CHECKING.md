# Runtime Type Checking: Handler Returns and Step Inputs

## Problem

Barnum's type safety is currently compile-time only (TypeScript generics). At runtime, handlers are opaque — the scheduler accepts whatever JSON they return. If a handler returns the wrong shape, the error surfaces far downstream (in a pick, branch, or serialization boundary), making debugging painful.

The same problem exists for step inputs in `registerSteps`: each step declares an input type, but nothing validates the actual runtime value matches when the step is entered.

## Goal

Add optional runtime validation at the two boundaries where data crosses between the Control Plane (AST/scheduler) and the Data Plane (handlers/external):

1. **Handler return values**: After an Invoke completes, validate the returned JSON against a schema before delivering it to the next node.
2. **Step inputs**: When a step is entered (via Step/goTo), validate the incoming value against the step's declared input schema.

## Why these two boundaries

These are the points where untrusted data enters the execution graph:

- **Handler returns**: The handler ran in a separate process. Its return value is JSON deserialized from IPC. Anything could be in there — wrong fields, missing fields, wrong types, extra fields the pipeline doesn't expect.
- **Step inputs**: Steps are entry points from other steps. The `goTo` site and the step's input type must agree. Currently this is only checked statically. A refactor that changes one side but not the other silently breaks at runtime.

Interior nodes (Chain, Parallel, Branch, etc.) don't need runtime validation — their data flow is structurally guaranteed by the AST. The scheduler connects outputs to inputs mechanically.

## Schema format: JSON Schema via Zod

TypeScript types erase at runtime. We need a runtime representation. Zod schemas are the natural choice:

1. Barnum users already write TypeScript. Zod is the standard TS schema library.
2. Zod schemas produce JSON Schema via `zod-to-json-schema`. JSON Schema is language-agnostic and can be validated in Rust (via `jsonschema` crate) or in the TypeScript worker.
3. Zod schemas serve double duty: they define the TypeScript type AND the runtime validator.

```ts
import { z } from "zod";

const ReviewResult = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("Approved") }),
  z.object({ kind: z.literal("RequiresHuman"), diffUrl: z.string() }),
]);

// The handler declaration includes the schema:
const automatedReview = defineHandler({
  input: z.object({ diff: z.string() }),
  output: ReviewResult,
  handler: async ({ diff }) => { ... },
});
```

## Where validation runs

### Option A: Rust-side validation (recommended)

The JSON Schema is embedded in the flat action table (attached to Invoke and Step entries). The Rust scheduler validates after deserialization, before delivering to the next node.

```rust
pub struct FlatInvoke {
    handler_id: HandlerId,
    output_schema: Option<JsonSchema>,  // None = no validation
}

// After handler returns:
if let Some(schema) = &invoke.output_schema {
    validate(schema, &returned_value)?;  // WorkflowError::SchemaViolation on failure
}
```

Pros: validation happens at the earliest possible point. Errors include the handler name, expected schema, and actual value. The TypeScript handler never sees invalid downstream behavior.

Cons: requires a JSON Schema validation library in the Rust crate.

### Option B: TypeScript-side validation

The worker SDK wraps each handler with Zod validation on output. The Rust engine sees already-validated data.

Pros: simpler Rust side. Zod validation is native.

Cons: validation runs in the worker process, which is less trusted. If the worker is buggy (returning wrong types), the validation itself might be buggy or skipped. Also doesn't cover the IPC boundary — malformed IPC messages bypass validation.

### Recommendation

Option A for handler returns (validate in Rust, at the trust boundary). Option B can be added as a development-time aid (fast Zod feedback in the worker), but the Rust validation is the authoritative check.

## Schema propagation

### Compile-time: TypeScript → JSON Schema → flat config

```ts
// In the builder, when creating an Invoke node:
const invokeNode: InvokeAction = {
  kind: "Invoke",
  handler: automatedReview,
  outputSchema: zodToJsonSchema(ReviewResult),  // embedded in the AST
};
```

The flattener carries the schema through to the flat action table. The Rust scheduler reads it at validation time.

### Step input schemas

```ts
registerSteps(({ typeCheck, deploy }) => ({
  typeCheck: defineStep({
    inputSchema: z.object({ branch: z.string(), repo: z.string() }),
    action: pipe(...),
  }),
  deploy: defineStep({
    inputSchema: z.object({ artifact: z.string() }),
    action: pipe(...),
  }),
}));
```

When a Step action fires (`goTo(typeCheck)`), the scheduler validates the incoming value against `typeCheck.inputSchema` before advancing the step's body.

## Error reporting

Schema validation errors should be maximally informative:

```
SchemaViolation {
  location: "handler 'automatedReview' return value",
  expected: { /* JSON Schema */ },
  actual: { "kind": "Approvd", "typo": true },
  path: ".kind",  // JSON pointer to the first failing field
  message: "Expected 'Approved' | 'RequiresHuman', got 'Approvd'",
}
```

The error includes:
- Where in the workflow the violation occurred (handler name, step name)
- The full expected schema
- The actual value
- The specific path and message from the JSON Schema validator

## Opt-in vs opt-out

Runtime validation has a cost (schema evaluation on every handler return). Two strategies:

**Opt-in (recommended)**: Validation only runs if the handler/step declares a schema. Handlers without schemas pass through unchecked. This is zero-cost for existing code and lets users add validation incrementally.

**Development mode**: A global flag enables validation for ALL handlers/steps, even those without explicit schemas (using schemas inferred from the TypeScript types, if available). This catches errors during development without production overhead.

## Interaction with effects

Handler DAGs (the AST subgraphs that run when an effect is caught) are internal — their inputs and outputs are structurally determined by the Handle/Perform mechanism. They don't need runtime validation.

The tagged output from handler DAGs (`{ kind: "Resume"|"Discard"|"RestartBody", value }`) could optionally be validated by the Handle frame. This would catch bugs in handler DAG construction. But since handler DAGs are framework-generated (not user-written), this is lower priority.

## Deliverables

1. `defineHandler()` function that accepts Zod input/output schemas
2. `defineStep()` function that accepts Zod input schema
3. `zodToJsonSchema` integration in the builder (compile-time schema embedding)
4. `output_schema` field on `FlatInvoke` and `FlatStep`
5. JSON Schema validation in the Rust scheduler (post-handler-return, pre-step-entry)
6. `SchemaViolation` error type with informative diagnostics
7. Tests: valid handler returns pass, invalid returns produce clear errors
8. Tests: step input validation catches mismatches
