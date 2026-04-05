# Handler Validation

## TL;DR

Handlers have Zod schemas (`inputValidator`, optional `outputValidator`) that are currently ignored at runtime. We need to:

1. Convert Zod schemas to JSON Schema at AST construction time
2. Embed the JSON Schema in the serialized AST (on the TypeScript handler definition)
3. Validate handler inputs and outputs in Rust at runtime using the `jsonschema` crate
4. Panic on validation failure (terminate the workflow)

Future: handler definitions will be deduplicated with handler IDs so the same handler used multiple times doesn't repeat its schema. That's out of scope here — noted for context only.

## Current state

`HandlerDefinition` in `libs/barnum/src/handler.ts:9-20`:

```ts
export interface HandlerDefinition<TValue, TOutput, TStepConfig> {
  inputValidator?: z.ZodType<TValue>;
  stepConfigValidator?: z.ZodType<TStepConfig>;
  handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
}
```

These validators drive TypeScript's compile-time types but are completely ignored at runtime:
- Not serialized to the AST (`TypeScriptHandler` has only `module` and `func`)
- Worker (`worker.ts:47`) calls `handle()` directly without validation
- Rust side has no knowledge of schemas

## What changes

### 1. Add `outputValidator` to `HandlerDefinition`

```ts
export interface HandlerDefinition<TValue, TOutput, TStepConfig> {
  inputValidator?: z.ZodType<TValue>;
  outputValidator?: z.ZodType<TOutput>;       // NEW
  stepConfigValidator?: z.ZodType<TStepConfig>;
  handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
}
```

### 2. Convert Zod to JSON Schema at construction time

In `createHandler` and `createHandlerWithConfig`, convert any provided Zod validators to JSON Schema using `zod-to-json-schema` and embed them in the AST node.

```ts
import { zodToJsonSchema } from "zod-to-json-schema";

// In createHandler:
const inputSchema = definition.inputValidator
  ? zodToJsonSchema(definition.inputValidator)
  : undefined;
const outputSchema = definition.outputValidator
  ? zodToJsonSchema(definition.outputValidator)
  : undefined;
```

### 3. Embed schemas in the TypeScript handler AST node

TypeScript side (`ast.ts`):

```ts
export interface TypeScriptHandler {
  kind: "TypeScript";
  module: string;
  func: string;
  inputSchema?: unknown;   // JSON Schema document
  outputSchema?: unknown;  // JSON Schema document
}
```

Rust side (`barnum_ast/src/lib.rs`):

```rust
pub struct TypeScriptHandler {
    pub module: ModulePath,
    pub func: FuncName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,   // JSON Schema document
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,  // JSON Schema document
}
```

Note on `stepConfigValidator`: for handlers with config (`createHandlerWithConfig`), the handler input is the `[value, config]` tuple produced by `All(Identity, Constant(config))`. The `inputValidator` covers the pipeline value; `stepConfigValidator` covers the config. Both get composed into a single input schema that validates the tuple structure. Alternatively, since the config is a compile-time constant embedded via `Constant`, only the pipeline input needs runtime validation — the config is already known-good. The input schema should validate the pipeline value only, not the full tuple.

### 4. Validate in Rust at runtime

Validation happens in the event loop (`barnum_event_loop`), not in the engine or builtins. The event loop is the boundary between the pure state machine and the outside world — it's where handler dispatch and completion happen.

**Input validation:** Before dispatching a TypeScript handler, validate the dispatch value against `input_schema` (if present). If validation fails, panic.

**Output validation:** After receiving a handler result, validate it against `output_schema` (if present). If validation fails, panic.

```rust
// In the dispatch loop:
if let HandlerKind::TypeScript(ts) = handler {
    if let Some(ref input_schema) = ts.input_schema {
        validate_or_panic(input_schema, &dispatch.value, "input", ts);
    }
}

// After receiving a result:
if let Some(ref output_schema) = handler.output_schema {
    validate_or_panic(output_schema, &value, "output", handler);
}
```

Panic message should include: which handler (module:func), whether it was input or output validation, and the validation errors with JSON paths.

### 5. JSON Schema validation of the schema itself

When the Rust binary deserializes the config, validate that any embedded JSON Schema documents are structurally valid JSON Schema (Draft 2020-12 or whatever `jsonschema` supports). This catches malformed schemas early rather than at first handler invocation. The `jsonschema` crate's `validator_for()` / `compile()` returns an error if the schema is invalid — use that.

This can happen at workflow init time: iterate all handlers in the flat config, compile their schemas, cache the compiled validators. Reuse them during dispatch/completion.

## Dependencies

| Package | Where | Purpose |
|---------|-------|---------|
| `zod-to-json-schema` (npm) | `@barnum/barnum` | Convert Zod schemas to JSON Schema at construction time |
| `jsonschema` (crate) | `barnum_event_loop` | Validate values against JSON Schema at runtime |

## Future optimization: eliding redundant validation

When two handlers are adjacent in a chain and the first handler's output schema is identical to the second handler's input schema, the output validation of the first and input validation of the second are redundant — one of them can be skipped. More generally, if a value flows through builtins (which are trusted) between two handlers with matching schemas, the intermediate validation can be elided.

This is a pure optimization and not part of this refactor. Implement naive validate-everything first, then add schema equality checks to skip redundant passes. Leave `// TODO: elide redundant validation when adjacent schemas match` comments at the validation call sites.

## What this does NOT include

- **No `Validate` builtin.** Validation is not a user-composable action in the pipeline. It's automatic enforcement at the handler boundary.
- **No handler deduplication / handler IDs.** Future work. For now, schemas are duplicated if the same handler appears multiple times in the AST.
- **No recovery from validation failures.** Validation failure = panic = workflow terminates. This is a contract violation, not an expected error. If we later want softer behavior (Result-based), that's a separate design.
- **No validation for builtins.** Builtins are framework code with known types. They don't cross a trust boundary.
