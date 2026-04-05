# Handler Schemas in AST

**Blocked by:** `OPTIONAL_HANDLER_TYPES.md` (done)
**Blocks:** `HANDLER_VALIDATION.md` (which adds Rust-side runtime validation using these schemas)

## TL;DR

Convert Zod validators to JSON Schema at `createHandler` call time and embed the schemas in the serialized AST so the Rust side can see them. Use proper JSON Schema types on both sides — `JSONSchema7` from `@types/json-schema` on the TS side, a `JsonSchema` newtype on the Rust side. The heavy lifting is done by the `zod-to-json-schema` library; our code is an allowlist walker that rejects Zod types that don't survive the serialization boundary.

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

## Step 1: Zod-to-JSON-Schema conversion and allowlist

### Dependencies

```
pnpm -C libs/barnum add zod-to-json-schema
pnpm -C libs/barnum add -D @types/json-schema
```

`zod-to-json-schema` handles the actual conversion — it already has parsers for 30 Zod types including intersection, union, objects, arrays, tuples, records, enums, literals, nullable, optional, and all modifiers. Our job is an allowlist walker that rejects Zod types that can't survive TS → JSON → Rust before the library ever sees them.

### Allowlist

**Allowed Zod types** (structural subset that maps to JSON Schema and survives serialization):

| Zod type | JSON Schema | Notes |
|---|---|---|
| `z.string()` | `{ "type": "string" }` | |
| `z.number()` | `{ "type": "number" }` | |
| `z.boolean()` | `{ "type": "boolean" }` | |
| `z.null()` | `{ "type": "null" }` | |
| `z.literal(v)` | `{ "const": v }` | JSON-compatible values only (string, number, boolean, null) |
| `z.object()` | `{ "type": "object", "properties": {...}, "required": [...] }` | |
| `z.array()` | `{ "type": "array", "items": {...} }` | |
| `z.tuple()` | `{ "type": "array", "prefixItems": [...], "minItems": N, "maxItems": N }` | |
| `z.record()` | `{ "type": "object", "additionalProperties": {...} }` | |
| `z.union()` | `{ "anyOf": [...] }` | |
| `z.discriminatedUnion()` | `{ "anyOf": [...] }` | Discriminator optimization lost, but schemas are valid |
| `z.intersection()` | `{ "allOf": [...] }` | See below |
| `z.enum()` | `{ "enum": [...] }` | |
| `z.nullable()` | `{ "anyOf": [schema, { "type": "null" }] }` | |
| `z.optional()` | Omits property from `required` | Only inside `z.object()` properties |
| `z.unknown()` | `{}` | Matches anything |
| `z.any()` | `{}` | Matches anything |

**Modifiers** (all map to JSON Schema keywords):

| Zod modifier | JSON Schema keyword |
|---|---|
| `.min()` (number) | `minimum` |
| `.max()` (number) | `maximum` |
| `.gt()` / `.lt()` | `exclusiveMinimum` / `exclusiveMaximum` |
| `.int()` | `type: "integer"` |
| `.multipleOf()` | `multipleOf` |
| `.min()` (string) | `minLength` |
| `.max()` (string) | `maxLength` |
| `.length()` (string) | `minLength` + `maxLength` |
| `.regex()` | `pattern` |
| `.email()` | `format: "email"` |
| `.url()` | `format: "uri"` |
| `.startsWith()` | `pattern: "^..."` |
| `.endsWith()` | `pattern: "...$"` |
| `.min()` (array) | `minItems` |
| `.max()` (array) | `maxItems` |

**Intersection note:** `zod-to-json-schema` converts `z.intersection()` to `allOf`, flattens nested `allOf`s, and handles `additionalProperties` stripping. The one edge case is `.strict()` objects in intersections on Draft 7 — `additionalProperties: false` on both sides means each rejects the other's properties. Draft 2019-09 solves this with `unevaluatedProperties`, but Draft 7 lacks it. Our validators don't use `.strict()` on intersected objects, so this is fine.

**Rejected Zod types** (throw at definition time):

| Zod type | Reason |
|---|---|
| `z.undefined()` | `undefined` doesn't exist in JSON. At the serialization boundary, TS `undefined` becomes `null` or absent. No JSON Schema representation. Use `z.null()`. |
| `z.optional()` standalone | `string \| undefined` — `undefined` doesn't exist in JSON. Use `z.nullable()` for `string \| null`. Fine inside `z.object()` properties (maps to omitting from `required`). |
| `z.nativeEnum()` | Takes a TS enum object at runtime with reverse mappings for numeric enums. Can't round-trip through JSON Schema. Use `z.enum()`. |
| `z.function()` | Not a data type. |
| `z.promise()` | Not a data type. |
| `z.void()` | Not a data type. Void handlers have no outputValidator. |
| `z.map()`, `z.set()` | No JSON representation. |
| `z.lazy()` | Circular references. |
| `.transform()`, `.refine()`, `.superRefine()`, `.preprocess()`, `.pipe()` | Runtime JS behavior, not schema-expressible. |

### Implementation

**File:** new `libs/barnum/src/schema.ts`

```ts
import type { JSONSchema7 } from "json-schema";
import type { z } from "zod";
import { zodToJsonSchema } from "zod-to-json-schema";

export function zodToCheckedJsonSchema(schema: z.ZodType, label: string): JSONSchema7 {
  assertJsonSchemaCompatible(schema, label);
  return zodToJsonSchema(schema) as JSONSchema7;
}

const ALLOWED_ZOD_TYPES = new Set([
  "ZodString", "ZodNumber", "ZodBoolean", "ZodNull",
  "ZodLiteral",
  "ZodObject", "ZodArray", "ZodTuple", "ZodRecord",
  "ZodUnion", "ZodDiscriminatedUnion", "ZodIntersection",
  "ZodEnum",
  "ZodNullable", "ZodOptional",
  "ZodUnknown", "ZodAny",
  "ZodDefault",
]);

function assertJsonSchemaCompatible(schema: z.ZodType, label: string): void {
  // Walk the Zod schema's internal _def structure.
  // Each Zod type has a _def.typeName (e.g., "ZodString", "ZodObject", etc.).
  // Reject any typeName not in ALLOWED_ZOD_TYPES.
  // For compound types (ZodObject, ZodArray, ZodUnion, etc.), recurse into children.
  //
  // Special case: ZodOptional — reject at top level, allow inside ZodObject properties.
  //
  // Throws: `Error: Handler "${label}": Zod type "${typeName}" cannot be
  //          expressed as JSON Schema. Use only structural types.`
}
```

### Tests

**File:** new `libs/barnum/tests/schema.test.ts`

Tests for the allowlist walker and end-to-end conversion.

**Allowlist acceptance tests** — each allowed Zod type produces valid JSON Schema:

```ts
// Primitives
z.string()                    → { type: "string" }
z.number()                    → { type: "number" }
z.boolean()                   → { type: "boolean" }
z.null()                      → { type: "null" }
z.literal("hello")            → { const: "hello" }
z.literal(42)                 → { const: 42 }
z.literal(true)               → { const: true }
z.literal(null)               → { const: null }

// Containers
z.object({ a: z.string() })   → { type: "object", properties: { a: { type: "string" } }, required: ["a"] }
z.array(z.number())            → { type: "array", items: { type: "number" } }
z.tuple([z.string(), z.number()])
z.record(z.string(), z.number())

// Composition
z.union([z.string(), z.number()])                     → { anyOf: [...] }
z.discriminatedUnion("kind", [...])                    → { anyOf: [...] }
z.intersection(z.object({ a: z.string() }), z.object({ b: z.number() }))  → { allOf: [...] }
z.enum(["a", "b", "c"])                               → { enum: ["a", "b", "c"] }
z.nullable(z.string())                                 → { anyOf: [{ type: "string" }, { type: "null" }] }

// Optional inside object (not standalone)
z.object({ a: z.string(), b: z.number().optional() }) → required: ["a"] (b omitted)

// Wildcards
z.unknown()                    → {}
z.any()                        → {}
```

**Modifier tests** — each Zod modifier produces the correct JSON Schema keyword:

```ts
z.string().min(3)              → { type: "string", minLength: 3 }
z.string().max(10)             → { type: "string", maxLength: 10 }
z.string().length(5)           → { type: "string", minLength: 5, maxLength: 5 }
z.string().regex(/^foo/)       → { type: "string", pattern: "^foo" }
z.string().email()             → { type: "string", format: "email" }
z.string().url()               → { type: "string", format: "uri" }
z.string().startsWith("foo")   → { type: "string", pattern: "^foo" }
z.string().endsWith("bar")     → { type: "string", pattern: "bar$" }
z.number().min(0)              → { type: "number", minimum: 0 }
z.number().max(100)            → { type: "number", maximum: 100 }
z.number().gt(0)               → { type: "number", exclusiveMinimum: 0 }
z.number().lt(100)             → { type: "number", exclusiveMaximum: 100 }
z.number().int()               → { type: "integer" }
z.number().multipleOf(5)       → { type: "number", multipleOf: 5 }
z.array(z.string()).min(1)     → { type: "array", items: { type: "string" }, minItems: 1 }
z.array(z.string()).max(10)    → { type: "array", items: { type: "string" }, maxItems: 10 }
```

**Rejection tests** — each rejected type throws at definition time:

```ts
z.undefined()                  → throws
z.string().optional()          → throws (standalone, not inside object)
z.nativeEnum(SomeEnum)         → throws
z.function()                   → throws
z.promise(z.string())          → throws
z.void()                       → throws
z.map(z.string(), z.number())  → throws
z.set(z.string())              → throws
z.lazy(() => z.string())       → throws
z.string().transform(...)      → throws
z.string().refine(...)         → throws
```

**Nested rejection tests** — rejected types inside allowed containers still throw:

```ts
z.object({ a: z.function() })           → throws
z.array(z.set(z.string()))              → throws
z.union([z.string(), z.undefined()])    → throws
z.intersection(z.object({ a: z.string() }), z.object({ b: z.map(z.string(), z.number()) }))  → throws
```

**Domain-specific pattern tests** — our actual patterns produce correct schemas:

```ts
// TaggedUnion
z.union([
  z.object({ kind: z.literal("HasErrors"), value: z.array(TypeErrorValidator) }),
  z.object({ kind: z.literal("Clean"), value: z.null() }),
])

// Result<string, string>
z.union([
  z.object({ kind: z.literal("Ok"), value: z.string() }),
  z.object({ kind: z.literal("Err"), value: z.string() }),
])

// Option<Refactor>
z.union([
  z.object({ kind: z.literal("Some"), value: RefactorValidator }),
  z.object({ kind: z.literal("None"), value: z.null() }),
])

// JudgmentResult
z.union([
  z.object({ approved: z.literal(true) }),
  z.object({ approved: z.literal(false), instructions: z.string() }),
])
```

---

## Step 2: Add schema fields to the AST and wire up conversion

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

## Step 3: Add output validators to all demos

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
