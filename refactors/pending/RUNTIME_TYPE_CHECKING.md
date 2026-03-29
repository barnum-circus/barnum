# Runtime Type Checking: Two Failure Modes

## Problem

Barnum's type safety is currently compile-time only (TypeScript generics). At runtime, handlers are opaque — the scheduler accepts whatever JSON they return. If a handler returns the wrong shape, the error surfaces far downstream (in a pick, branch, or serialization boundary), making debugging painful.

The same problem exists for step inputs in `registerSteps`: each step declares an input type, but nothing validates the actual runtime value matches when the step is entered.

## Architectural constraint: Rust stays domain-ignorant

The Rust engine routes opaque `serde_json::Value` payloads. It only validates the structural integrity of its own protocol (e.g., verifying an IPC message is a valid `Yield` or `Resume`). It never inspects domain types.

This is intentional. The engine is a state machine that advances a DAG. Embedding JSON Schema validation in Rust would:

- Couple the engine to domain-specific concerns
- Require shipping schema data through the flat config and into the Rust crate
- Add a JSON Schema validation dependency to the engine

Runtime type checking belongs entirely in the Data Plane — specifically in the worker SDK (TypeScript/Python) at the boundary where the worker communicates back to the orchestrator.

## The two failure modes

Not all handler failures are equal. The correct response depends on whether the handler is deterministic or non-deterministic.

### 1. Byzantine faults: deterministic handler contract violations

A deterministic handler (e.g., `deriveBranch`, `classifyErrors`, `preparePRInput`) is pure code. If it violates its compile-time contract at runtime — returning a string instead of an object, missing a required field — something is deeply wrong. The AST's type guarantees are broken; the DAG is dead.

**Response**: The worker SDK catches the Zod parsing error on the handler's output and sends a `ContractViolation` message over IPC. The Rust engine receives this, immediately halts the workflow execution, marks it as `Failed(ByzantineFault)`, and drops the slab. This is not routed to a `tryCatch` block. There is no recovery.

```
Worker                              Rust Engine
  │                                      │
  │  handler returns { wrong: "shape" }  │
  │  outputValidator.parse() throws      │
  │                                      │
  │  ──── ContractViolation ──────────>  │
  │                                      │  marks workflow Failed(ByzantineFault)
  │                                      │  drops slab
```

### 2. Expected domain failures: non-deterministic handler schema errors

Non-deterministic handlers — those that call LLMs, external APIs, or other unpredictable sources — are expected to fail. An LLM hallucinating a JSON key is not a Byzantine fault; it is an expected domain failure.

If you treat an LLM hallucination as a `ContractViolation`, the workflow dies, and you cannot retry the prompt. To build self-healing agentic workflows, you must lift the schema validation failure into the data plane so the AST can loop and retry.

**Response**: The handler itself catches the Zod error and returns an explicit `Result` variant:

```typescript
const ExpectedSchema = z.object({
  prUrl: z.string().url(),
  confidence: z.number(),
});

async function askAgent(prompt: string): Promise<Result<ParsedOutput, string>> {
  const rawText = await llm.generate(prompt);

  try {
    const parsed = JSON.parse(rawText);
    const validData = ExpectedSchema.parse(parsed);
    return { kind: "Ok", value: validData };
  } catch (err) {
    // Expected failure — return as data, not a crash.
    return {
      kind: "Err",
      value: `Agent produced invalid schema: ${err.message}. Raw: ${rawText}`,
    };
  }
}
```

The AST handles retry via `loop`/`branch` or algebraic effects (`tryCatch`/`Perform(Throw)`):

```typescript
loop(
  pipe(
    askAgent,
    branch({
      Ok: done(),
      Err: pipe(augmentPromptWithFeedback, recur()),
    }),
  ),
)
```

## The rule

| Handler type | Schema failure means | Worker response | Engine response |
|---|---|---|---|
| **Deterministic** (pure code) | Bug — contract violation | `ContractViolation` IPC message | `Failed(ByzantineFault)`, kill workflow |
| **Non-deterministic** (LLM, API) | Expected — domain failure | Return `Result` with `Err` variant | Route normally; AST handles retry |

## Where validation runs: worker-side only

Every handler is wrapped in a strict validation schema (Zod in TypeScript, Pydantic in Python) in the worker SDK. The worker validates *before* sending the IPC payload to the Rust engine.

```typescript
// Worker SDK wraps every handler invocation:
const result = await handler.__definition.handle({ value: input.value });

if (handler.__definition.outputValidator) {
  try {
    handler.__definition.outputValidator.parse(result);
  } catch (err) {
    // Deterministic handler violated its contract.
    sendIPC({ kind: "ContractViolation", handler: handler.name, error: err.message });
    return;
  }
}

sendIPC({ kind: "Yield", value: result });
```

For non-deterministic handlers, the Zod validation happens *inside* the handler (before the `return`), not in the SDK wrapper. The handler catches its own Zod errors and returns `Result.err(...)`.

### Why not Rust-side validation?

The earlier version of this doc recommended Rust-side validation (embedding JSON Schema in the flat config, validating in the Rust scheduler after deserialization). This is rejected for three reasons:

1. **Violates domain ignorance**: The Rust engine should not know about handler output shapes.
2. **Wrong trust boundary**: The worker SDK is the code that calls handlers. It is the natural place to validate their output. Validation in Rust after IPC transit is too late — you've already crossed the serialization boundary.
3. **Unnecessary complexity**: Requires `jsonschema` crate in Rust, schema propagation through the flat config, and JSON Schema conversion from Zod. Worker-side Zod validation is native, zero-cost to set up, and catches errors at the source.

## Schema format: Zod (native)

TypeScript types erase at runtime. We need a runtime representation. Zod schemas are the natural choice:

1. Barnum users already write TypeScript. Zod is the standard TS schema library.
2. Zod schemas serve double duty: they define the TypeScript type AND the runtime validator.
3. No JSON Schema conversion needed — Zod validates directly in the worker.

```ts
import { z } from "zod";

export const myHandler = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.object({ content: z.string() }),
  handle: async ({ value }) => {
    // If this returns wrong shape, outputValidator catches it
    return { content: readFile(value.file) };
  },
});
```

## Handler declaration: `outputValidator`

Add an optional `outputValidator` field to `createHandler`:

```ts
export function createHandler<TValue, TOutput>(definition: {
  inputValidator?: z.ZodType<TValue>;
  outputValidator?: z.ZodType<TOutput>;  // Must match handle's return type
  handle: (context: { value: TValue }) => Promise<TOutput>;
}, name: string): TypedHandler<TValue, TOutput>;
```

TypeScript ensures the `outputValidator` schema matches the `handle` function's return type at compile time. At runtime, the worker SDK calls `outputValidator.parse()` on the handler's return value.

## Step input validation

Step inputs are another boundary where untrusted data enters the execution graph. When a step is entered (via `goTo`), the incoming value should be validated against the step's declared input schema.

```ts
registerSteps(({ stepRef }) => ({
  typeCheck: defineStep({
    inputValidator: z.object({ branch: z.string(), repo: z.string() }),
    action: pipe(...),
  }),
}));
```

Step input validation is also worker-side. When the Rust engine dispatches to a step, the worker validates the incoming value before executing the step's action.

## Error reporting

### ContractViolation (deterministic handlers)

The IPC message includes enough context for the developer to diagnose the bug:

```json
{
  "kind": "ContractViolation",
  "handler": "deriveBranch",
  "expected": "{ branch: string }",
  "actual": "\"some-string\"",
  "zodError": "Expected object, received string",
  "path": ""
}
```

The Rust engine logs this and marks the workflow as `Failed(ByzantineFault)`.

### Domain errors (non-deterministic handlers)

These are regular `Result` values flowing through the AST. The error payload is whatever the handler puts in the `Err` variant — typically a string describing the parse failure and the raw LLM output. The AST's `branch`/`loop`/`tryCatch` mechanisms handle retry.

## Opt-in behavior

- `outputValidator` is optional. Handlers without it pass through unchecked.
- Non-deterministic handlers that call LLMs should NOT use `outputValidator` — they should catch Zod errors internally and return `Result`.
- `outputValidator` is for deterministic handlers where a schema violation is a bug.

## Deliverables

1. Add optional `outputValidator` field to `createHandler`
2. Worker SDK validates handler output via `outputValidator.parse()` before sending IPC
3. `ContractViolation` IPC message type
4. Rust engine handles `ContractViolation`: marks workflow `Failed(ByzantineFault)`
5. Step input validation in worker SDK
6. Documentation: guidance on when to use `outputValidator` vs internal `Result` returns
7. Tests: deterministic handler violation produces `ContractViolation`
8. Tests: non-deterministic handler returns `Result.err()`, AST retries
