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

## Phase 1: TypeScript changes

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

### 1.5 Convert Zod to JSON Schema in `createHandler` implementation

**File:** `libs/barnum/src/handler.ts`

In the implementation body of `createHandler`, convert validators to JSON Schema and embed them in the Invoke action:

```ts
import { zodToJsonSchema } from "zod-to-json-schema";

// In createHandler implementation:
const inputSchema = definition.inputValidator
  ? zodToJsonSchema(definition.inputValidator)
  : undefined;
const outputSchema = definition.outputValidator
  ? zodToJsonSchema(definition.outputValidator)
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

### 1.6 Add `zod-to-json-schema` dependency

```
pnpm -C libs/barnum add zod-to-json-schema
```

---

## Phase 2: Rust changes

### 2.1 Add schema fields to `TypeScriptHandler`

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

### 2.2 Add `jsonschema` crate dependency

**File:** `crates/barnum_event_loop/Cargo.toml`

```toml
[dependencies]
jsonschema = "0.28"  # or latest
```

### 2.3 Compile schemas at workflow init

**File:** `crates/barnum_event_loop/src/lib.rs`

At the start of `run_workflow`, iterate all handlers in the flat config, compile any JSON Schema documents into validators, and store them in a map keyed by handler ID. This avoids recompiling schemas on every dispatch.

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
                let validator = Validator::new(schema)
                    .expect("invalid input JSON Schema for {ts.module}:{ts.func}");
                input.insert(handler_id, validator);
            }
            if let Some(ref schema) = ts.output_schema {
                let validator = Validator::new(schema)
                    .expect("invalid output JSON Schema for {ts.module}:{ts.func}");
                output.insert(handler_id, validator);
            }
        }
    }
    CompiledSchemas { input, output }
}
```

### 2.4 Validate at dispatch and completion

**File:** `crates/barnum_event_loop/src/lib.rs`

In the `run_workflow` loop:

```rust
// Before dispatch:
for dispatch in &dispatches {
    let handler = workflow_state.handler(dispatch.handler_id);
    // TODO: elide redundant validation when adjacent schemas match
    if let Some(validator) = compiled_schemas.input.get(&dispatch.handler_id) {
        let result = validator.validate(&dispatch.value);
        if let Err(errors) = result {
            panic!(
                "input validation failed for {}:{}: {}",
                handler.module(), handler.func(),
                format_validation_errors(errors),
            );
        }
    }
    scheduler.dispatch(dispatch, handler);
}

// After completion:
let value = result?;
// TODO: elide redundant validation when adjacent schemas match
if let Some(validator) = compiled_schemas.output.get(&handler_id_for_task) {
    let result = validator.validate(&value);
    if let Err(errors) = result {
        panic!(
            "output validation failed for handler: {}",
            format_validation_errors(errors),
        );
    }
}
```

Note: output validation needs to look up which handler produced the task. The `task_to_frame` map connects TaskId → FrameId → HandlerId. We'll need to capture the handler_id before calling `complete()` (which removes the frame). Store the mapping in the scheduler or track it separately.

---

## Phase 3: Add output validators to all demos

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

Uses `Result<string, string>` return type. The `StepResult` type:

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
