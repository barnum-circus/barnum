# Handler Schemas in AST

**Blocked by:** `OPTIONAL_HANDLER_TYPES.md` (done), `ZOD_TO_JSON_SCHEMA.md` (done)
**Blocks:** `HANDLER_VALIDATION.md` (which adds Rust-side runtime validation using these schemas)

## TL;DR

Embed JSON Schema in the serialized AST so the Rust side can see handler input/output schemas. Use `JSONSchema7` from `@types/json-schema` on the TS side, a `JsonSchema` newtype over `serde_json::Value` on the Rust side. The Zod-to-JSON-Schema conversion itself (`zodToCheckedJsonSchema`, wrapping Zod v4's native `toJSONSchema()`) is handled by `ZOD_TO_JSON_SCHEMA.md`.

---

## Current state

`TypeScriptHandler` AST node in `libs/barnum/src/ast.ts`:

```ts
export interface TypeScriptHandler {
  kind: "TypeScript";
  module: string;
  func: string;
}
```

Rust `TypeScriptHandler` in `crates/barnum_ast/src/lib.rs`:

```rust
pub struct TypeScriptHandler {
    pub module: ModulePath,
    pub func: FuncName,
}
```

No schema information flows from TypeScript to Rust.

---

## Step 1: Add schema fields to the AST and wire up conversion

### Add `JsonSchema` type to Rust AST

**File:** `crates/barnum_ast/src/json_schema.rs` (new)

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON Schema document embedded in the AST.
///
/// Newtype over `Value` — the TS side produces it via `zod-to-json-schema`,
/// and `HANDLER_VALIDATION.md` will compile it with the `jsonschema` crate
/// at workflow init time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonSchema(pub Value);
```

No new crate dependencies — `serde_json` is already in `barnum_ast`'s deps.

### Add schema fields to `TypeScriptHandler`

**File:** `libs/barnum/src/ast.ts`

```ts
import type { JSONSchema7 } from "json-schema";

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
  inputSchema?: JSONSchema7;
  outputSchema?: JSONSchema7;
}
```

**File:** `crates/barnum_ast/src/lib.rs`

```rust
use crate::json_schema::JsonSchema;

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
    pub input_schema: Option<JsonSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonSchema>,
}
```

### Wire conversion into `createHandler` / `createHandlerWithConfig`

**File:** `libs/barnum/src/handler.ts`

```ts
import { zodToCheckedJsonSchema } from "./schema.js";

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

### Tests

Round-trip tests: TS creates a handler with validators → serializes AST to JSON → Rust deserializes → schemas are present and structurally correct.

---

## Step 2: Add output validators to all demos

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

All 6 handlers in this demo return `string` or `string[]`:
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
- `logError`: no outputValidator (returns void)

### convert-folder-to-ts/handlers/convert.ts

- `setup`: `outputValidator: z.object({ inputDir: z.string(), outputDir: z.string() })`
- `listFiles`: `outputValidator: z.array(z.object({ file: z.string(), outputPath: z.string() }))`
- `migrate`: no outputValidator (returns void)

### convert-folder-to-ts/handlers/type-check-fix.ts

```ts
const TypeErrorValidator = z.object({ file: z.string(), message: z.string() });
```

- `typeCheck`: `outputValidator: z.array(TypeErrorValidator)`
- `classifyErrors`: `outputValidator: z.union([z.object({ kind: z.literal("HasErrors"), value: z.array(TypeErrorValidator) }), z.object({ kind: z.literal("Clean"), value: z.null() })])`
- `fix`: `outputValidator: z.object({ file: z.string(), fixed: z.literal(true) })`

### identify-and-address-refactors/handlers/git.ts

- `createWorktree`: `outputValidator: z.object({ worktreePath: z.string(), branch: z.string() })`
- `deleteWorktree`: no outputValidator (returns void)
- `createPR`: `outputValidator: z.object({ prUrl: z.string() })`

### identify-and-address-refactors/handlers/refactor.ts

- `listTargetFiles`: `outputValidator: z.array(z.object({ file: z.string() }))`
- `analyze`: `outputValidator: z.array(RefactorValidator)` (extract existing Refactor shape into a Zod schema)
- `assessWorthiness`: `outputValidator: z.union([z.object({ kind: z.literal("Some"), value: RefactorValidator }), z.object({ kind: z.literal("None"), value: z.null() })])`
- `deriveBranch`: `outputValidator: z.object({ branch: z.string() })`
- `preparePRInput`: `outputValidator: z.object({ branch: z.string(), title: z.string(), body: z.string() })`
- `implement`: no outputValidator (returns void)
- `commit`: no outputValidator (returns void)
- `judgeRefactor`: `outputValidator: z.union([z.object({ approved: z.literal(true) }), z.object({ approved: z.literal(false), instructions: z.string() })])`
- `classifyJudgment`: `outputValidator: z.union([z.object({ kind: z.literal("Approved"), value: z.null() }), z.object({ kind: z.literal("NeedsWork"), value: z.string() })])`
- `applyFeedback`: no outputValidator (returns void)

### identify-and-address-refactors/handlers/type-check-fix.ts

Same as convert-folder-to-ts version.
