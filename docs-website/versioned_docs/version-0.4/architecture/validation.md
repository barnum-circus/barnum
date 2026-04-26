# Validation

Barnum uses [Zod](https://zod.dev/) as a **single source of truth** for handler contracts. A single Zod schema simultaneously:

1. **Generates TypeScript types** — the handler's `value` parameter is typed from `inputValidator`, and the return type is checked against `outputValidator`.
2. **Compiles to JSON Schema** — the schema is converted to a language-independent JSON Schema document and embedded in the serialized AST.
3. **Validates at runtime** — the Rust engine compiles the JSON Schema into a validator at workflow init and checks every handler invocation.

One schema definition yields compile-time type checking, cross-language serialization contracts, and runtime validation.

## Zod as the source of truth

```ts
export const refactor = createHandler({
  inputValidator: z.object({
    file: z.string(),
    strategy: z.enum(["hooks", "signals"]),
  }),
  outputValidator: z.object({
    linesChanged: z.number().int().nonneg(),
  }),
  handle: async ({ value }) => {
    // value is typed as { file: string; strategy: "hooks" | "signals" }
    // return type must be { linesChanged: number }
  },
}, "refactor");
```

From this single definition:

- **TypeScript infers** that `value` has type `{ file: string; strategy: "hooks" | "signals" }` and that the handler must return `{ linesChanged: number }`. Mismatches are compile-time errors.
- **`zodToCheckedJsonSchema()`** converts the Zod schemas to JSON Schema Draft 7. The resulting schemas are embedded in the AST's `Invoke` node as `input_schema` and `output_schema`.
- **The Rust runtime** compiles these JSON Schemas into `jsonschema::Validator` instances and checks every handler invocation at both entry and exit.

Without this, TypeScript types, JSON Schema documents, and validation logic would be maintained separately — three artifacts that inevitably drift apart.

## Compile-time: Zod → JSON Schema

When `createHandler` is called, each validator is immediately compiled to JSON Schema:

```ts
const inputSchema = definition.inputValidator
  ? zodToCheckedJsonSchema(definition.inputValidator, `${filePath}:${funcName} input`)
  : undefined;
```

### zodToCheckedJsonSchema

This function does two things:

**1. Pre-validation** — walks the Zod schema tree and rejects patterns that don't survive the TypeScript → JSON → Rust boundary:

- **`z.intersection()`** is rejected. It produces `allOf` with `additionalProperties: false` on both sides (from `io: "output"`), making the intersection unmatchable on Draft 7. Use `z.object().extend()` or `z.object().merge()` instead.
- **`.refine()` and `.superRefine()`** are rejected. Custom validation functions are silently dropped by `toJSONSchema()`, so the Rust side would accept values that fail the refinement. Detected by checking for `check === "custom"` in the schema's checks array.

**2. Conversion** — calls Zod's built-in `toJSONSchema()` with strict options:

```ts
const raw = toJSONSchema(schema, {
  target: "draft-07",        // JSON Schema Draft 7 (widest Rust support)
  unrepresentable: "throw",  // Fail on types that can't be expressed
  io: "output",              // Generate output-facing schema (includes defaults)
  cycles: "throw",           // Reject cyclic schemas
  reused: "inline",          // Inline shared references (no $ref)
});
```

The `$schema` field is stripped from the result — embedded schemas don't need the draft URI.

### Why JSON Schema instead of Zod at runtime?

Zod is a TypeScript library. The Rust runtime has no JavaScript engine. JSON Schema is a language-independent standard with validators in every major language — including Rust's `jsonschema` crate. Converting at definition time means validation is a zero-dependency operation on the Rust side.

## Serialization: schema in the AST

The compiled JSON Schema is embedded directly in the handler's AST node:

```json
{
  "kind": "Invoke",
  "handler": {
    "kind": "TypeScript",
    "module": "./steps.ts",
    "func": "refactor",
    "input_schema": {
      "type": "object",
      "properties": {
        "file": { "type": "string" },
        "strategy": { "enum": ["hooks", "signals"] }
      },
      "required": ["file", "strategy"]
    },
    "output_schema": {
      "type": "object",
      "properties": {
        "linesChanged": { "type": "integer", "minimum": 0 }
      },
      "required": ["linesChanged"]
    }
  }
}
```

The Rust AST mirrors this structure:

```rust
struct TypeScriptHandler {
    module: ModulePath,
    func: FuncName,
    input_schema: Option<JsonSchema>,   // JsonSchema is a newtype over serde_json::Value
    output_schema: Option<JsonSchema>,
}
```

Schemas are optional. Handlers without validators have no `input_schema` or `output_schema` fields — they pass through unvalidated.

## Runtime: schema compilation at workflow init

Before the first handler is dispatched, `compile_schemas()` iterates the handler pool and compiles every embedded JSON Schema into a `jsonschema::Validator`:

```rust
struct CompiledSchemas {
    input: HashMap<HandlerId, Validator>,
    output: HashMap<HandlerId, Validator>,
}
```

Only TypeScript handlers are checked — builtins are framework code with known types and no trust boundary. Handlers without schemas produce no entries in the maps.

If a schema is not valid JSON Schema (malformed, not just a type mismatch), the workflow fails immediately with `RunWorkflowError::InvalidSchema` — at init, not during the first invocation.

## Runtime: validation at every boundary

Every handler invocation is validated at two points:

### Pre-dispatch (input validation)

Before the handler is dispatched to a subprocess, the input value is checked against the handler's `input_schema`:

```rust
EventKind::Dispatch(dispatch_event) => {
    validate_value(
        &compiled_schemas.input,
        dispatch_event.handler_id,
        &dispatch_event.value,
        SchemaDirection::Input,
        workflow_state,
    )?;
    scheduler.dispatch(&dispatch_event, handler);
}
```

This catches pipeline mismatches — when a previous step produces data that doesn't match what the next handler expects. The handler subprocess is never spawned if the input is invalid.

### Post-completion (output validation)

After a handler returns, the output value is checked against the handler's `output_schema`:

```rust
EventKind::Completion(completion_event) => {
    let handler_id = workflow_state.handler_id_for_task(completion_event.task_id);
    validate_value(
        &compiled_schemas.output,
        handler_id,
        &completion_event.value,
        SchemaDirection::Output,
        workflow_state,
    )?;
    if let Some(terminal) = complete(workflow_state, completion_event)? {
        return Ok(terminal);
    }
}
```

This catches buggy handlers — when a handler returns data that violates its own contract. The value is never delivered to downstream steps if it's invalid.

### The validate_value function

```rust
fn validate_value(
    validators: &HashMap<HandlerId, Validator>,
    handler_id: HandlerId,
    value: &Value,
    direction: SchemaDirection,
    workflow_state: &WorkflowState,
) -> Result<(), RunWorkflowError> {
    let Some(validator) = validators.get(&handler_id) else {
        return Ok(());  // No validator → no validation
    };
    let errors: Vec<_> = validator.iter_errors(value).collect();
    if errors.is_empty() {
        return Ok(());
    }
    // Build error with module, func, direction, and all validation errors
    Err(RunWorkflowError::SchemaValidation { ... })
}
```

If no validator exists for a handler (it has no schema), validation silently passes. This is intentional — validators are opt-in.

## Error types

Two error variants cover validation failures:

**`RunWorkflowError::InvalidSchema`** — the embedded JSON Schema itself is malformed (not valid per the JSON Schema specification). Caught at workflow init. Example: `{ "type": "integer", "minimum": "not-a-number" }`.

**`RunWorkflowError::SchemaValidation`** — a value failed validation against a well-formed schema. Caught at dispatch or completion. Includes the handler's module path, function name, direction (input/output), and individual validation error messages.

Both are fatal — the workflow terminates immediately. There is no automatic recovery from validation failures. For error recovery, wrap the handler invocation in `tryCatch`.

## Why validate twice?

**Input validation** catches mismatches between pipeline steps. The previous handler produced data that doesn't match what this handler expects. Without input validation, the handler would receive malformed data and either crash or produce garbage.

**Output validation** catches buggy handlers. The handler's implementation doesn't match its declared contract. Without output validation, downstream handlers would receive unexpected data types.

Together, they enforce a contract at every serialization boundary: data entering a handler is valid, and data leaving a handler is valid.
