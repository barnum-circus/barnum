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

---

## Task 1: Embed schemas in the AST

Add `inputSchema` and `outputSchema` fields to the TypeScript handler AST node (both TS and Rust sides). Convert Zod validators to JSON Schema at `createHandler` / `createHandlerWithConfig` call time. Validate that the Zod schemas use only JSON-Schema-expressible types (reject `.transform()`, `.refine()`, `.pipe()`, `.preprocess()`, `z.map()`, `z.set()`, `z.promise()`, `z.function()`, `z.lazy()`, etc. at conversion time with a clear error).

### 1.1 Add `outputValidator` to `HandlerDefinition`

**File:** `libs/barnum/src/handler.ts`

```ts
// Before
export interface HandlerDefinition<TValue = unknown, TOutput = unknown, TStepConfig = unknown> {
  inputValidator?: z.ZodType<TValue>;
  stepConfigValidator?: z.ZodType<TStepConfig>;
  handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
}

// After
export interface HandlerDefinition<TValue = unknown, TOutput = unknown, TStepConfig = unknown> {
  inputValidator?: z.ZodType<TValue>;
  outputValidator?: z.ZodType<TOutput>;
  stepConfigValidator?: z.ZodType<TStepConfig>;
  handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
}
```

### 1.2 Add `outputValidator` to `createHandler` overloads

**File:** `libs/barnum/src/handler.ts`

```ts
// Before — with inputValidator
export function createHandler<TValue, TOutput>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;

// Before — without inputValidator
export function createHandler<TOutput>(
  definition: {
    handle: () => Promise<TOutput>;
  },
  exportName?: string,
): Handler<never, HandlerOutput<TOutput>>;

// After — with inputValidator
export function createHandler<TValue, TOutput>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    outputValidator?: z.ZodType<TOutput>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;

// After — without inputValidator
export function createHandler<TOutput>(
  definition: {
    outputValidator?: z.ZodType<TOutput>;
    handle: () => Promise<TOutput>;
  },
  exportName?: string,
): Handler<never, HandlerOutput<TOutput>>;
```

### 1.3 Add `outputValidator` to `createHandlerWithConfig` overloads

**File:** `libs/barnum/src/handler.ts`

```ts
// Before — with inputValidator
export function createHandlerWithConfig<TValue, TOutput, TStepConfig>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// Before — without inputValidator
export function createHandlerWithConfig<TOutput, TStepConfig>(
  definition: {
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<never, HandlerOutput<TOutput>>;

// After — with inputValidator
export function createHandlerWithConfig<TValue, TOutput, TStepConfig>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    outputValidator?: z.ZodType<TOutput>;
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// After — without inputValidator
export function createHandlerWithConfig<TOutput, TStepConfig>(
  definition: {
    outputValidator?: z.ZodType<TOutput>;
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<never, HandlerOutput<TOutput>>;
```

### 1.4 Add schema fields to `TypeScriptHandler` AST node

**File:** `libs/barnum/src/ast.ts`

```ts
// Before
export interface TypeScriptHandler {
  kind: "TypeScript";
  module: string;
  func: string;
}

// After
export interface TypeScriptHandler {
  kind: "TypeScript";
  module: string;
  func: string;
  inputSchema?: unknown;
  outputSchema?: unknown;
}
```

### 1.5 Zod-to-JSON-Schema conversion with allowlist

**File:** `libs/barnum/src/handler.ts` (or a new `libs/barnum/src/schema.ts`)

Convert Zod validators to JSON Schema using `zod-to-json-schema`. Before conversion, walk the Zod schema tree and reject types that can't be expressed as JSON Schema. This throws at handler definition time (module load), not at runtime.

**Allowed Zod types** (the structural subset that maps cleanly to JSON Schema):
- `z.string()`, `z.number()`, `z.boolean()`, `z.null()`, `z.undefined()`
- `z.literal()`
- `z.object()`, `z.array()`, `z.tuple()`, `z.record()`
- `z.union()`, `z.discriminatedUnion()`, `z.intersection()`
- `z.enum()`, `z.nativeEnum()`
- `z.optional()`, `z.nullable()`
- `z.unknown()`, `z.any()`
- Modifiers: `.min()`, `.max()`, `.length()`, `.regex()`, `.email()`, `.url()`, etc.

**Rejected Zod types** (no JSON Schema equivalent — throw at definition time):
- `z.function()`, `z.promise()`, `z.void()` (as validator — void handlers just have no outputValidator)
- `z.map()`, `z.set()`
- `z.lazy()` (circular references)
- `.transform()`, `.refine()`, `.superRefine()`, `.preprocess()`, `.pipe()`

```ts
import { zodToJsonSchema } from "zod-to-json-schema";

function zodToCheckedJsonSchema(schema: z.ZodType, label: string): unknown {
  assertJsonSchemaCompatible(schema, label);
  return zodToJsonSchema(schema);
}

function assertJsonSchemaCompatible(schema: z.ZodType, label: string): void {
  // Walk the Zod schema's internal _def structure.
  // Each Zod type has a _def.typeName (e.g., "ZodString", "ZodObject", etc.).
  // Reject any typeName not in the allowlist.
  // For compound types (ZodObject, ZodArray, ZodUnion, etc.), recurse into children.
  //
  // Throws: `Error: Handler "${label}": Zod type "${typeName}" cannot be
  //          expressed as JSON Schema. Use only structural types.`
}
```

### 1.6 Wire conversion into `createHandler` / `createHandlerWithConfig` implementations

**File:** `libs/barnum/src/handler.ts`

```ts
// In createHandler implementation:
const inputSchema = definition.inputValidator
  ? zodToCheckedJsonSchema(definition.inputValidator, `${filePath}:${funcName} input`)
  : undefined;
const outputSchema = definition.outputValidator
  ? zodToCheckedJsonSchema(definition.outputValidator, `${filePath}:${funcName} output`)
  : undefined;

const action = typedAction({
  kind: "Invoke",
  handler: {
    kind: "TypeScript",
    module: filePath,
    func: funcName,
    ...(inputSchema && { inputSchema }),
    ...(outputSchema && { outputSchema }),
  },
});
```

Same pattern in `createHandlerWithConfig`.

### 1.7 Rust: add schema fields to `TypeScriptHandler`

**File:** `crates/barnum_ast/src/lib.rs`

```rust
// Before
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeScriptHandler {
    pub module: ModulePath,
    pub func: FuncName,
}

// After
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeScriptHandler {
    pub module: ModulePath,
    pub func: FuncName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}
```

Note: `PartialEq` and `Eq` may need to be dropped or manually implemented since `serde_json::Value` doesn't impl `Eq`. Alternatively, wrap in a newtype.

### 1.8 Add `zod-to-json-schema` dependency

```
pnpm -C libs/barnum add zod-to-json-schema
```

### 1.9 Add output validators to all demos

See [Appendix: Demo output validators](#appendix-demo-output-validators) for the full list.

---

## Task 2: Compile and validate schemas on Rust side

When the Rust binary deserializes the config, compile all JSON Schema documents into validators using the `jsonschema` crate. This serves two purposes: (a) validates that the schemas themselves are valid JSON Schema (catching malformed schemas at init, not at first handler invocation), and (b) caches compiled validators for use in Task 3.

### 2.1 Add `jsonschema` crate dependency

**File:** `crates/barnum_event_loop/Cargo.toml`

```toml
[dependencies]
jsonschema = "0.28"  # or latest
```

### 2.2 Compile schemas at workflow init, panic on invalid schema

**File:** `crates/barnum_event_loop/src/lib.rs`

```rust
use jsonschema::Validator;
use std::collections::HashMap;

struct CompiledSchemas {
    input: HashMap<HandlerId, Validator>,
    output: HashMap<HandlerId, Validator>,
}

fn compile_schemas(workflow_state: &WorkflowState) -> CompiledSchemas {
    let mut input = HashMap::new();
    let mut output = HashMap::new();
    for (handler_id, handler) in workflow_state.handlers() {
        if let HandlerKind::TypeScript(ts) = handler {
            if let Some(ref schema) = ts.input_schema {
                let validator = Validator::new(schema).unwrap_or_else(|err| {
                    panic!(
                        "invalid input JSON Schema for {}:{}: {err}",
                        ts.module.lookup(), ts.func.lookup(),
                    )
                });
                input.insert(handler_id, validator);
            }
            if let Some(ref schema) = ts.output_schema {
                let validator = Validator::new(schema).unwrap_or_else(|err| {
                    panic!(
                        "invalid output JSON Schema for {}:{}: {err}",
                        ts.module.lookup(), ts.func.lookup(),
                    )
                });
                output.insert(handler_id, validator);
            }
        }
    }
    CompiledSchemas { input, output }
}
```

Call `compile_schemas` at the top of `run_workflow`, before the main loop. Any invalid schema panics immediately with a clear message identifying the handler.

---

## Task 3: Validate handler inputs and outputs at runtime

Use the compiled schemas from Task 2 to validate values at the handler boundary. Panic on failure.

### 3.1 Validate input before dispatch

**File:** `crates/barnum_event_loop/src/lib.rs`

In the `run_workflow` dispatch loop:

```rust
for dispatch in &dispatches {
    let handler = workflow_state.handler(dispatch.handler_id);
    // TODO: elide redundant validation when adjacent schemas match
    if let Some(validator) = compiled_schemas.input.get(&dispatch.handler_id) {
        let errors: Vec<_> = validator.iter_errors(&dispatch.value).collect();
        if !errors.is_empty() {
            panic!(
                "input validation failed for {}:{}:\n{}",
                ts.module.lookup(), ts.func.lookup(),
                format_validation_errors(&errors),
            );
        }
    }
    scheduler.dispatch(dispatch, handler);
}
```

### 3.2 Validate output after completion

**File:** `crates/barnum_event_loop/src/lib.rs`

After receiving a handler result, before passing to `complete()`:

```rust
let value = result?;

// Look up which handler produced this task.
// task_to_frame maps TaskId → FrameId, frame has handler_id.
// We need to read handler_id BEFORE complete() removes the frame.
let handler_id = workflow_state.handler_id_for_task(task_id);

// TODO: elide redundant validation when adjacent schemas match
if let Some(validator) = compiled_schemas.output.get(&handler_id) {
    let errors: Vec<_> = validator.iter_errors(&value).collect();
    if !errors.is_empty() {
        let handler = workflow_state.handler(handler_id);
        panic!(
            "output validation failed for {}:{}:\n{}",
            handler.module(), handler.func(),
            format_validation_errors(&errors),
        );
    }
}

if let Some(terminal_value) = workflow_state.complete(task_id, value)? {
    return Ok(terminal_value);
}
```

Note: `handler_id_for_task` is a new method on `WorkflowState` that reads from `task_to_frame` without removing the entry. Currently `complete()` does `task_to_frame.remove()`. We need to read the handler_id first (via `task_to_frame.get()` → frame → handler_id) before `complete()` consumes it.

### 3.3 `format_validation_errors` helper

```rust
fn format_validation_errors(errors: &[jsonschema::ValidationError]) -> String {
    errors
        .iter()
        .map(|e| format!("  - {}: {}", e.instance_path, e))
        .collect::<Vec<_>>()
        .join("\n")
}
```

---

## Appendix: Demo output validators

### simple-workflow/handlers/steps.ts

```ts
// Before
export const listFiles = createHandler({
  handle: async () => {
    // ...
    return ["auth.ts", "database.ts", "routes.ts"];
  },
}, "listFiles");

// After
export const listFiles = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => {
    // ...
    return ["auth.ts", "database.ts", "routes.ts"];
  },
}, "listFiles");
```

All 6 handlers in this demo return `string` or `string[]`. Add:
- `listFiles`: `outputValidator: z.array(z.string())`
- `implementRefactor`: `outputValidator: z.string()`
- `typeCheckFiles`: `outputValidator: z.string()`
- `fixTypeErrors`: `outputValidator: z.string()`
- `commitChanges`: `outputValidator: z.string()`
- `createPullRequest`: `outputValidator: z.string()`

### retry-on-error/handlers/steps.ts

Uses `Result<string, string>` return type. Extract a shared validator:

```ts
const StepResultValidator = z.union([
  z.object({ kind: z.literal("Ok"), value: z.string() }),
  z.object({ kind: z.literal("Err"), value: z.string() }),
]);
```

- `stepA`: `outputValidator: StepResultValidator`
- `stepB`: `outputValidator: StepResultValidator`
- `stepC`: `outputValidator: StepResultValidator`
- `logError`: no outputValidator (returns void → `never`)

### convert-folder-to-ts/handlers/convert.ts

- `setup`: `outputValidator: z.object({ inputDir: z.string(), outputDir: z.string() })`
- `listFiles`: `outputValidator: z.array(z.object({ file: z.string(), outputPath: z.string() }))`
- `migrate`: no outputValidator (returns void)

### convert-folder-to-ts/handlers/type-check-fix.ts

```ts
const TypeErrorValidator = z.object({ file: z.string(), message: z.string() });
```

- `typeCheck`: `outputValidator: z.array(TypeErrorValidator)`
- `classifyErrors`: `outputValidator: z.union([z.object({ kind: z.literal("HasErrors"), value: z.array(TypeErrorValidator) }), z.object({ kind: z.literal("Clean"), value: z.undefined() })])`
- `fix`: `outputValidator: z.object({ file: z.string(), fixed: z.literal(true) })`

### identify-and-address-refactors/handlers/git.ts

- `createWorktree`: `outputValidator: z.object({ worktreePath: z.string(), branch: z.string() })`
- `deleteWorktree`: no outputValidator (returns void)
- `createPR`: `outputValidator: z.object({ prUrl: z.string() })`

### identify-and-address-refactors/handlers/refactor.ts

- `listTargetFiles`: `outputValidator: z.array(z.object({ file: z.string() }))`
- `analyze`: `outputValidator: z.array(RefactorValidator)` (extract existing Refactor shape into a Zod schema)
- `assessWorthiness`: `outputValidator: z.union([z.object({ kind: z.literal("Some"), value: RefactorValidator }), z.object({ kind: z.literal("None"), value: z.undefined() })])`
- `deriveBranch`: `outputValidator: z.object({ branch: z.string() })`
- `preparePRInput`: `outputValidator: z.object({ branch: z.string(), title: z.string(), body: z.string() })`
- `implement`: no outputValidator (returns void)
- `commit`: no outputValidator (returns void)
- `judgeRefactor`: `outputValidator: z.union([z.object({ approved: z.literal(true) }), z.object({ approved: z.literal(false), instructions: z.string() })])`
- `classifyJudgment`: `outputValidator: z.union([z.object({ kind: z.literal("Approved"), value: z.undefined() }), z.object({ kind: z.literal("NeedsWork"), value: z.string() })])`
- `applyFeedback`: no outputValidator (returns void)

### identify-and-address-refactors/handlers/type-check-fix.ts

Same as convert-folder-to-ts version.

---

## Future optimization: eliding redundant validation

When two handlers are adjacent in a chain and the first handler's output schema is identical to the second handler's input schema, the output validation of the first and input validation of the second are redundant — one of them can be skipped. More generally, if a value flows through builtins (which are trusted) between two handlers with matching schemas, the intermediate validation can be elided.

This is a pure optimization and not part of this refactor. Implement naive validate-everything first, then add schema equality checks to skip redundant passes. Leave `// TODO: elide redundant validation when adjacent schemas match` comments at the validation call sites.

## What this does NOT include

- **No `Validate` builtin.** Validation is not a user-composable action in the pipeline. It's automatic enforcement at the handler boundary.
- **No handler deduplication / handler IDs.** Future work. For now, schemas are duplicated if the same handler appears multiple times in the AST.
- **No recovery from validation failures.** Validation failure = panic = workflow terminates. This is a contract violation, not an expected error. If we later want softer behavior (Result-based), that's a separate design.
- **No validation for builtins.** Builtins are framework code with known types. They don't cross a trust boundary.
- **No type-only handlers (specifying types without validators).** Separate refactor. For now, if you want runtime validation, provide a Zod schema. If you don't provide one, no validation occurs and the type defaults to `unknown` / `never` per existing behavior.

## Dependencies

| Package | Where | Purpose |
|---------|-------|---------|
| `zod-to-json-schema` (npm) | `@barnum/barnum` | Convert Zod schemas to JSON Schema at construction time |
| `jsonschema` (crate) | `barnum_event_loop` | Validate values against JSON Schema at runtime |
