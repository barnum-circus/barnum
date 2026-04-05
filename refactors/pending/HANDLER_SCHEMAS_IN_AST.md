# Handler Schemas in AST

**Blocked by:** `OPTIONAL_HANDLER_TYPES.md` (which adds `outputValidator` and makes all validators optional)
**Blocks:** `HANDLER_VALIDATION.md` (which adds Rust-side runtime validation using these schemas)

## TL;DR

Convert Zod validators to JSON Schema at `createHandler` call time and embed the schemas in the serialized AST so the Rust side can see them. `OPTIONAL_HANDLER_TYPES.md` has already added `outputValidator` and made all validators optional — this doc wires the Zod-to-JSON-Schema conversion and adds schema fields to the AST.

## Current state (after OPTIONAL_HANDLER_TYPES)

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

No schema information flows from TypeScript to Rust. Validators exist on the TS side but are invisible to the Rust event loop.

---

## 1. Add schema fields to `TypeScriptHandler` AST node

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

## 2. Zod-to-JSON-Schema conversion with allowlist

**File:** new `libs/barnum/src/schema.ts`

Convert Zod validators to JSON Schema using `zod-to-json-schema`. Before conversion, walk the Zod schema tree and reject types that can't be expressed as JSON Schema. This throws at handler definition time (module load), not at workflow runtime.

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

## 3. Wire conversion into `createHandler` / `createHandlerWithConfig`

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

## 4. Add `zod-to-json-schema` dependency

```
pnpm -C libs/barnum add zod-to-json-schema
```

## 5. Add output validators to all demos

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
