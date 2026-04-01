# Handler Input/Output Validation

## Problem

Barnum's type safety is compile-time only. At runtime, handlers return opaque `serde_json::Value`. If a handler returns the wrong shape, the error surfaces far downstream (in a pick, branch, or serialization boundary), making debugging painful. LLM and API handlers make this worse — they routinely return garbage.

The existing `RUNTIME_TYPE_CHECKING.md` doc recommended worker-side-only Zod validation with a ContractViolation IPC message. This doc supersedes that design with a more compositional approach: Rust-side validation via JSON Schema, exposed as an AST-level builtin.

## Design overview

1. **Zod schemas serialize to JSON Schema at AST construction time** (in TypeScript).
2. **A `Validate` builtin runs in Rust** — validates a `serde_json::Value` against a JSON Schema inline, no worker dispatch.
3. **`Validate` always returns `Result<T, ValidationError>`** — never panics. The caller decides what to do with the Result.
4. **Surface sugar** on handlers and at call sites composes Validate with existing combinators (unwrapOr, tryCatch) to express strict/lenient modes.

## Validate as a builtin

Validate is a data transformation: `Value → Result<Value, ValidationError>`. It doesn't change control flow. It belongs alongside ExtractField, Tag, and Merge — a Rust-side operation that runs inline.

```rust
// In BuiltinKind (barnum_ast):
Validate { schema: serde_json::Value }  // JSON Schema document
```

The Rust engine processes it:

```rust
BuiltinKind::Validate { schema } => {
    let validator = jsonschema::validator_for(schema)?;
    match validator.validate(&value) {
        Ok(()) => json!({ "kind": "Ok", "value": value, "__def": null }),
        Err(errors) => json!({
            "kind": "Err",
            "value": {
                "message": format_validation_errors(&errors),
                "errors": errors.map(|e| json!({
                    "path": e.instance_path.to_string(),
                    "message": e.to_string(),
                })).collect::<Vec<_>>(),
            },
            "__def": null,
        }),
    }
}
```

Always returns a `Result<T, ValidationError>`. The `__def` field is included so `.branch()`, `.unwrapOr()`, and other Result combinators work.

### Why not a new AST node?

Validate is a pure function from Value to Value. It doesn't need a frame, doesn't change control flow, doesn't interact with effects. Builtins are the right abstraction for inline Rust operations. Adding an AST node would give it unwarranted structural weight — it's no different from ExtractField conceptually.

### Why not worker-side only?

1. **Builtins have no worker.** Identity, Tag, Merge, etc. are never dispatched to a TypeScript subprocess. If a Validate follows a builtin pipeline, keeping validation in Rust avoids an unnecessary worker round trip.
2. **Rust catches it earlier.** Validation after handler completion but before the value re-enters the engine catches errors at the source, before they propagate through chains and branches.
3. **JSON Schema is language-agnostic.** When Python/Go workers are added, the same JSON Schema validates their output. Zod is TypeScript-specific; JSON Schema works everywhere.

## Zod to JSON Schema serialization

At AST construction time, Zod schemas are converted to JSON Schema and embedded in the serialized config:

```ts
import { zodToJsonSchema } from "zod-to-json-schema";

function validate<T>(schema: z.ZodType<T>): TypedAction<unknown, Result<T, ValidationError>> {
  const jsonSchema = zodToJsonSchema(schema);
  return typedAction<unknown, Result<T, ValidationError>>({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "Validate", schema: jsonSchema },
    },
  });
}
```

The `zod-to-json-schema` package handles the conversion. It supports the full Zod type vocabulary (objects, unions, refinements, transforms, etc.). The resulting JSON Schema is a plain object that serializes to the flat config as `serde_json::Value`.

### Dependency: `zod-to-json-schema`

Added as a dependency of `@barnum/barnum`. ~5M weekly downloads, well-maintained, covers Zod 3 and 4. No alternative is needed.

### Dependency: `jsonschema` crate

Added to `barnum_builtins` (not `barnum_engine`). The engine stays domain-ignorant — it routes opaque values. The builtins crate handles JSON Schema validation when the Validate builtin is executed. This keeps the engine pure.

## Handler DX

### Current state

```ts
export const myHandler = createHandler({
  inputValidator: z.object({ file: z.string() }),
  handle: async ({ value }) => {
    return { content: readFile(value.file) };
  },
});
// Type: Handler<{ file: string }, { content: string }>
```

The `inputValidator` drives the TypeScript type. No output validation exists.

### Proposed: outputValidator

```ts
export const myHandler = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.object({ content: z.string() }),
  handle: async ({ value }) => {
    return { content: readFile(value.file) };
  },
});
```

When `outputValidator` is present, `createHandler` embeds the JSON Schema in the handler metadata. The flat config carries it. The question is: how does the output validation get into the pipeline?

### Three modes of handler usage

The key insight: **handlers are not the right unit for deciding validation behavior.** The same handler might be used in strict mode in one pipeline and lenient mode in another. The call site decides.

#### Mode 1: Raw (no validation) — current behavior

```ts
pipe(prepareInput, myHandler, processOutput)
```

No validation. The handler's output flows directly to the next action. This is the default for all existing code.

#### Mode 2: Validated (returns Result)

```ts
pipe(prepareInput, myHandler.validated(), handleResult)
// myHandler.validated() : TypedAction<In, Result<Out, ValidationError>>
```

The handler runs, then its output is validated against the outputValidator's JSON Schema. The result is `Result<Out, ValidationError>`. The caller branches on Ok/Err.

Desugars to:

```ts
pipe(myHandler, validate(outputJsonSchema))
```

#### Mode 3: Strict (panics on failure)

```ts
tryCatch(
  (throwError) => pipe(
    prepareInput,
    myHandler.strict(throwError),
    processOutput,
  ),
  handleValidationError,
)
// myHandler.strict(throwError) : TypedAction<In, Out>
```

The handler runs, output is validated, and if validation fails, the error is thrown via the provided throwError token. If inside a tryCatch, the error is caught. If not, the Perform bubbles up as an unhandled effect and the workflow fails — which is the correct behavior for a handler that should never return bad data.

Desugars to:

```ts
pipe(myHandler, validate(outputJsonSchema), Result.unwrapOr(throwError))
```

#### Mode 4: Standalone validate combinator

For cases where you're validating data that didn't come from a handler (e.g., parsing LLM output, validating external API responses):

```ts
const ParsedOutput = z.object({ prUrl: z.string().url(), confidence: z.number() });

pipe(
  askLLM,
  validate(ParsedOutput),
  branch({
    Ok: processValidOutput,
    Err: pipe(formatFeedback, retryLLM),
  }),
)
```

`validate(schema)` is the low-level primitive. `.validated()` and `.strict()` are sugar that compose it with the handler's own outputValidator.

## TypedAction method signatures

```ts
// On TypedAction interface, gated by Handler brand:
validated<TIn, TOut>(
  this: Handler<TIn, TOut>,
): TypedAction<TIn, Result<TOut, ValidationError>>;

strict<TIn, TOut>(
  this: Handler<TIn, TOut>,
  throwError: TypedAction<ValidationError, never>,
): TypedAction<TIn, TOut>;
```

Both require the handler to have an `outputValidator`. If it doesn't, the methods should error at the type level. We can enforce this by adding an `__hasOutputValidator` phantom field to handlers created with outputValidator, and using a `this` constraint that requires it.

Actually — a simpler approach: `.validated()` and `.strict()` are only available on handlers that were created with `outputValidator`. The Handler type can carry this info:

```ts
type ValidatedHandler<TValue, TOutput> = Handler<TValue, TOutput> & {
  __outputSchema: unknown;  // the JSON Schema, non-enumerable at runtime
  validated(): TypedAction<TValue, Result<TOutput, ValidationError>>;
  strict(throwError: TypedAction<ValidationError, never>): TypedAction<TValue, TOutput>;
};
```

`createHandler` with `outputValidator` returns `ValidatedHandler`. Without it, returns plain `Handler`. The methods don't exist on plain handlers — calling `.validated()` on a handler without outputValidator is a compile error.

## Validate as a type-check action — the AST node question

The user asked whether a "type check handler" should be a new AST node. The argument for:

- A Validate AST node would be visible to static analysis. The `barnum check` command could verify that every handler's output is validated, or that validation schemas are consistent with the pipeline's types.
- It would appear in workflow visualization as an explicit validation step.

The argument against:

- Validate is a stateless, side-effect-free data transformation. Every other data transformation (Tag, ExtractField, Merge) is a Builtin, not an AST node. Promoting Validate to an AST node creates an inconsistency.
- The Builtin variant already appears in the flat config. Static analysis can find Validate builtins as easily as Validate AST nodes.

**Recommendation: Builtin, not AST node.** Same reasoning as all other data transforms. If we later need special-case handling for validation in static analysis or visualization, we can pattern-match on `BuiltinKind::Validate` the same way we'd pattern-match on a hypothetical `FlatAction::Validate`.

## Input validation

Input validation is less critical than output validation because:
1. TypeScript's type system already ensures correct types at pipeline connection points.
2. Input validation failures are bugs (the pipeline wired wrong types together), not expected failures.

But it's still useful as a safety net, especially at step boundaries where a Step jump transfers control with no type checking.

When `inputValidator` is present on a handler, we could optionally insert a Validate builtin before the Invoke:

```
// Without input validation:
Invoke(handler)

// With input validation (opt-in):
Chain(Validate(inputJsonSchema), Invoke(handler))
```

This is a construction-time transform in `createHandler` — the handler AST gains a validation prefix. In strict mode (the default for input validation), a validation failure means a bug, so it should fire through tryCatch or kill the workflow.

For step boundaries, `registerSteps` could accept an `inputValidator` that inserts a Validate at the step entry:

```ts
registerSteps({
  TypeCheck: defineStep({
    inputValidator: z.object({ branch: z.string(), repo: z.string() }),
    action: pipe(...),
  }),
})
```

## ValidationError type

```ts
type ValidationError = {
  message: string;
  errors: Array<{
    path: string;      // JSON pointer to the failing field
    message: string;   // human-readable error
  }>;
};
```

This is the Err payload of the Result returned by Validate. It's a tagged union variant like any other — you can branch on it, map it, log it, or augment it with context before retrying.

## Implementation plan

### Phase 1: Validate builtin

1. Add `zod-to-json-schema` dependency to `@barnum/barnum`
2. Add `jsonschema` crate to `barnum_builtins`
3. Add `Validate { schema: Value }` variant to `BuiltinKind` (TS and Rust)
4. Implement Rust-side validation in the builtins executor
5. Add `validate<T>(schema: z.ZodType<T>)` combinator in TypeScript
6. Tests: valid input returns Ok, invalid input returns Err with path info

### Phase 2: Handler integration

1. Add optional `outputValidator` to `createHandler` / `createHandlerWithConfig`
2. Serialize outputValidator to JSON Schema at construction time, store on handler
3. Add `.validated()` and `.strict()` methods to ValidatedHandler type
4. Tests: handler.validated() wraps output in Result, handler.strict() throws on failure

### Phase 3: Input validation (optional)

1. When inputValidator is present, optionally insert Validate before Invoke at construction time
2. Step input validation via `defineStep({ inputValidator, action })`
3. Tests: bad input caught at step entry, at handler entry

## Rust-side changes

| Crate | Change |
|-------|--------|
| `barnum_ast` | Add `Validate { schema: Value }` to `BuiltinKind` enum |
| `barnum_builtins` | Add `jsonschema` dependency, implement validation |
| `barnum_engine` | No changes — engine routes opaque values, builtins handle validation |

## Builtins don't need type-safety validation

Builtins (Identity, Tag, Merge, ExtractField, Flatten, Validate itself, etc.) are implemented in Rust by us. Their input/output types are fully known at construction time and enforced by TypeScript generics — the type signatures in the DSL guarantee correct wiring. There is no untrusted boundary: no user-written code runs, no IPC crosses, no external process returns opaque data. A builtin that receives the wrong type is a bug in the framework, not in user code, and would be caught by TypeScript long before it reaches Rust.

Runtime validation is for handler outputs (user-written code that crosses the worker boundary) and step inputs (control flow jumps that bypass TypeScript's pipeline type checking). Builtins are neither.

## Open questions

1. **Schema caching.** Should the Rust side compile JSON Schema validators once and cache them, or validate fresh each time? For workflows with loops that re-validate on each iteration, caching matters. The `jsonschema` crate supports compiled validators — we should use them. The flat config could pre-compile all Validate schemas at workflow init time.

2. **Zod transforms and refinements.** `zod-to-json-schema` handles most Zod types, but Zod transforms (`.transform()`) and some refinements (`.refine()`) don't have JSON Schema equivalents. We should document which Zod features are supported in validators and which silently pass through. Alternatively, the TS surface could warn at construction time when a schema uses unsupported features.

3. **ValidationError as a first-class type.** Should `ValidationError` be a proper TaggedUnion variant with a namespace (like Option, Result)? Probably not — it's an error payload, not a control flow type. Plain object is fine.
