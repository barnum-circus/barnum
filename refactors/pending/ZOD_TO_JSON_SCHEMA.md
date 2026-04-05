# Zod-to-JSON-Schema Conversion

**Blocked by:** nothing
**Blocks:** `HANDLER_SCHEMAS_IN_AST.md` (which embeds schemas in the AST)

## TL;DR

Wrap Zod v4's built-in `toJSONSchema()` with a pre-validation walker and `$schema` stripping. The walker rejects patterns that `toJSONSchema()` handles incorrectly or silently: intersections (produce broken `allOf` on Draft 7 due to `additionalProperties: false` on both sides) and refinements (silently dropped, so the Rust side would think validation passed). No external library needed — `zod-to-json-schema` is deprecated; Zod v4 does this natively.

---

## Current state

Handlers optionally declare `inputValidator` and `outputValidator` as Zod schemas (Zod 4.3.6), but these are only used at runtime in TypeScript. No serialized representation exists.

---

## Dependency

```
pnpm -C libs/barnum add -D @types/json-schema
```

One dev dependency for the `JSONSchema7` type. Zod v4 exports `toJSONSchema` directly — `import { toJSONSchema } from "zod"` — so there is no runtime dependency to add.

---

## How `toJSONSchema()` works

Zod v4's `toJSONSchema()` walks the Zod schema tree and produces a JSON Schema document. It accepts an options object:

```ts
toJSONSchema(schema, {
  target: "draft-07",           // JSON Schema draft version
  unrepresentable: "throw",     // throw on types that can't become JSON Schema (default)
  io: "output",                 // serialize the output type, not input (default)
  cycles: "throw",              // reject recursive schemas (instead of emitting $ref)
  reused: "inline",             // inline reused sub-schemas, no $defs (default)
})
```

With `unrepresentable: "throw"` (the default), unsupported Zod types throw immediately. With `io: "output"`, objects get `additionalProperties: false` (strict output validation). With `cycles: "throw"`, recursive `z.lazy()` schemas throw instead of producing `$ref`/`$defs`.

The returned object has a `$schema` property at the root and a non-enumerable `~standard` property. Both must be stripped for embedding in the AST.

---

## Implementation

**File:** new `libs/barnum/src/schema.ts`

```ts
import type { JSONSchema7 } from "json-schema";
import { type z, toJSONSchema } from "zod";

// Zod v4 schema def types that have child schemas.
// Verified against Zod 4.3.6 internals — every compound type's def
// shape and child property name is listed here.
const CHILD_ACCESSORS: Record<
  string,
  (def: any) => z.ZodType[]
> = {
  object: (def) => Object.values(def.shape),
  array: (def) => [def.element],
  tuple: (def) => [...def.items, ...(def.rest ? [def.rest] : [])],
  union: (def) => def.options,
  intersection: (def) => [def.left, def.right],
  record: (def) => [def.keyType, def.valueType],
  // Wrappers with a single inner type
  nullable: (def) => [def.innerType],
  optional: (def) => [def.innerType],
  nonoptional: (def) => [def.innerType],
  default: (def) => [def.innerType],
  catch: (def) => [def.innerType],
  readonly: (def) => [def.innerType],
  promise: (def) => [def.innerType],
  // Pipe has two children
  pipe: (def) => [def.in, def.out],
  // Lazy resolves to inner schema
  lazy: (def) => [def.getter()],
};

/**
 * Walk the Zod schema tree and reject patterns that `toJSONSchema()`
 * handles incorrectly or silently drops:
 *
 * - `z.intersection()` — produces `allOf` with `additionalProperties: false`
 *   on both sides (from `io: "output"`), making the intersection unmatchable
 *   on Draft 7.
 *
 * - `.refine()` / `.superRefine()` — silently stripped from JSON Schema
 *   output, so the Rust side would accept values that fail the refinement.
 *   Detected by checking for custom checks (`check._zod.def.check === "custom"`)
 *   in the schema's checks array.
 */
function assertNoUnsupportedPatterns(
  schema: z.ZodType,
  label: string,
  visited = new WeakSet<z.ZodType>(),
): void {
  if (visited.has(schema)) return;
  visited.add(schema);

  const def = (schema as any)._zod.def;

  // Reject intersections
  if (def.type === "intersection") {
    throw new Error(
      `Handler "${label}": z.intersection() is not supported. ` +
        `It produces broken JSON Schema on Draft 7 because both sides ` +
        `get additionalProperties: false. Use z.object().extend() or ` +
        `z.object().merge() instead.`,
    );
  }

  // Reject custom checks (from .refine() and .superRefine())
  const checks: any[] | undefined = def.checks;
  if (checks) {
    for (const check of checks) {
      if (check._zod.def.check === "custom") {
        throw new Error(
          `Handler "${label}": .refine() and .superRefine() are not ` +
            `supported. Custom validations cannot be expressed in JSON ` +
            `Schema and would be silently dropped.`,
        );
      }
    }
  }

  // Recurse into children
  const getChildren = CHILD_ACCESSORS[def.type];
  if (getChildren) {
    for (const child of getChildren(def)) {
      assertNoUnsupportedPatterns(child, label, visited);
    }
  }
}

/**
 * Convert a Zod schema to a JSON Schema document suitable for embedding
 * in the serialized AST. Throws if the schema contains types that can't
 * survive the TS → JSON → Rust boundary.
 */
export function zodToCheckedJsonSchema(
  schema: z.ZodType,
  label: string,
): JSONSchema7 {
  // Pre-validate: catch patterns that toJSONSchema() handles incorrectly
  assertNoUnsupportedPatterns(schema, label);

  let raw: Record<string, unknown>;
  try {
    raw = toJSONSchema(schema, {
      target: "draft-07",
      unrepresentable: "throw",
      io: "output",
      cycles: "throw",
      reused: "inline",
    }) as Record<string, unknown>;
  } catch (error) {
    const message =
      error instanceof Error ? error.message : String(error);
    throw new Error(
      `Handler "${label}": Zod schema cannot be converted to JSON Schema: ${message}`,
    );
  }

  // Strip $schema — embedded schemas don't need the draft URI.
  const { $schema: _, ...rest } = raw;
  return rest as JSONSchema7;
}
```

The implementation has two phases: `assertNoUnsupportedPatterns` walks the Zod schema tree to reject intersections and refinements before `toJSONSchema()` sees them. `toJSONSchema()` then handles the actual conversion and rejects its own set of unsupported types (bigint, symbol, etc.).

**File:** `libs/barnum/src/index.ts` — add export

```ts
// add to existing exports
export { zodToCheckedJsonSchema } from "./schema.js";
```

---

## What `toJSONSchema()` produces for each Zod type

Every assertion below is exact — these are the expected values in the test suite after `$schema` stripping. Objects include `additionalProperties: false` because we use `io: "output"`.

### Primitives

| Zod schema | JSON Schema |
|---|---|
| `z.string()` | `{ type: "string" }` |
| `z.number()` | `{ type: "number" }` |
| `z.boolean()` | `{ type: "boolean" }` |
| `z.null()` | `{ type: "null" }` |
| `z.unknown()` | `{}` |
| `z.any()` | `{}` |

### Literals

| Zod schema | JSON Schema |
|---|---|
| `z.literal("hello")` | `{ type: "string", const: "hello" }` |
| `z.literal(42)` | `{ type: "number", const: 42 }` |
| `z.literal(true)` | `{ type: "boolean", const: true }` |
| `z.literal(null)` | `{ type: "null", const: null }` |

### Enum

| Zod schema | JSON Schema |
|---|---|
| `z.enum(["a", "b", "c"])` | `{ type: "string", enum: ["a", "b", "c"] }` |

### Containers

**Object:**
```ts
z.object({ a: z.string(), b: z.number() })
```
```json
{
  "type": "object",
  "properties": {
    "a": { "type": "string" },
    "b": { "type": "number" }
  },
  "required": ["a", "b"],
  "additionalProperties": false
}
```

**Object with optional field:**
```ts
z.object({ a: z.string(), b: z.number().optional() })
```
```json
{
  "type": "object",
  "properties": {
    "a": { "type": "string" },
    "b": { "type": "number" }
  },
  "required": ["a"],
  "additionalProperties": false
}
```

**Array:**
```ts
z.array(z.number())
```
```json
{ "type": "array", "items": { "type": "number" } }
```

**Tuple (Draft 7 format):**
```ts
z.tuple([z.string(), z.number()])
```
```json
{
  "type": "array",
  "items": [{ "type": "string" }, { "type": "number" }]
}
```

**Record:**
```ts
z.record(z.string(), z.number())
```
```json
{
  "type": "object",
  "propertyNames": { "type": "string" },
  "additionalProperties": { "type": "number" }
}
```

### Composition

**Union:**
```ts
z.union([z.string(), z.number()])
```
```json
{ "anyOf": [{ "type": "string" }, { "type": "number" }] }
```

**Nullable:**
```ts
z.nullable(z.string())
```
```json
{ "anyOf": [{ "type": "string" }, { "type": "null" }] }
```

### Modifiers (string)

| Zod schema | JSON Schema |
|---|---|
| `z.string().min(3)` | `{ type: "string", minLength: 3 }` |
| `z.string().max(10)` | `{ type: "string", maxLength: 10 }` |
| `z.string().length(5)` | `{ type: "string", minLength: 5, maxLength: 5 }` |
| `z.string().regex(/^foo/)` | `{ type: "string", pattern: "^foo" }` |
| `z.string().email()` | `{ type: "string", format: "email" }` |
| `z.string().url()` | `{ type: "string", format: "uri" }` |
| `z.string().startsWith("foo")` | `{ type: "string", pattern: "^foo" }` |
| `z.string().endsWith("bar")` | `{ type: "string", pattern: "bar$" }` |

### Modifiers (number)

| Zod schema | JSON Schema |
|---|---|
| `z.number().min(0)` | `{ type: "number", minimum: 0 }` |
| `z.number().max(100)` | `{ type: "number", maximum: 100 }` |
| `z.number().gt(0)` | `{ type: "number", exclusiveMinimum: 0 }` |
| `z.number().lt(100)` | `{ type: "number", exclusiveMaximum: 100 }` |
| `z.number().int()` | `{ type: "integer" }` |
| `z.number().multipleOf(5)` | `{ type: "number", multipleOf: 5 }` |

### Modifiers (array)

| Zod schema | JSON Schema |
|---|---|
| `z.array(z.string()).min(1)` | `{ type: "array", items: { type: "string" }, minItems: 1 }` |
| `z.array(z.string()).max(10)` | `{ type: "array", items: { type: "string" }, maxItems: 10 }` |

### Transparent wrappers

These Zod types are invisible to JSON Schema — the output is the inner type's schema:

| Zod wrapper | Behavior |
|---|---|
| `.optional()` | Transparent. Inside objects, omits from `required`. Standalone, produces inner type's schema. |
| `.default(value)` | Adds `"default": value` to the inner type's schema. |
| `.readonly()` | Adds `"readOnly": true` to the inner type's schema. |

---

## What we reject (pre-validation walker)

The `assertNoUnsupportedPatterns` walker runs before `toJSONSchema()` and rejects patterns that `toJSONSchema()` handles incorrectly or silently drops:

| Pattern | Error message |
|---|---|
| `z.intersection(A, B)` | `Handler "${label}": z.intersection() is not supported. It produces broken JSON Schema on Draft 7 because both sides get additionalProperties: false. Use z.object().extend() or z.object().merge() instead.` |
| `.refine(fn)` | `Handler "${label}": .refine() and .superRefine() are not supported. Custom validations cannot be expressed in JSON Schema and would be silently dropped.` |
| `.superRefine(fn)` | Same as `.refine()`. |

**Why intersections are rejected:** `toJSONSchema()` with `io: "output"` adds `additionalProperties: false` to every object. An intersection of `{a: string}` and `{b: number}` produces `allOf` where side A rejects `b` and side B rejects `a`. On Draft 7 there's no `unevaluatedProperties` to fix this — the intersection is unmatchable. Rather than producing silently broken schemas, we reject at build time.

**Why refinements are rejected:** `toJSONSchema()` silently drops `.refine()` and `.superRefine()` — it produces the inner type's schema with no trace of the custom check. The Rust side would then validate with JSON Schema and think the value is valid when it might not be. A refinement that can't cross the serialization boundary must be a build-time error, not a silent hole.

**Detection mechanism:** Intersections are detected by `schema._zod.def.type === "intersection"`. Refinements are detected by scanning `schema._zod.def.checks` for entries where `check._zod.def.check === "custom"`. Built-in checks (min, max, regex, etc.) use specific check type strings like `"min_length"`, `"less_than"`, `"string_format"`, etc. — `"custom"` is exclusively produced by `.refine()`, `.superRefine()`, and `.check(fn)`.

---

## What `toJSONSchema()` rejects

These types throw when `unrepresentable: "throw"` (our configuration):

| Zod type | Error message |
|---|---|
| `z.undefined()` | `"Undefined cannot be represented in JSON Schema"` |
| `z.void()` | `"Void cannot be represented in JSON Schema"` |
| `z.bigint()` | `"BigInt cannot be represented in JSON Schema"` |
| `z.symbol()` | `"Symbols cannot be represented in JSON Schema"` |
| `z.date()` | `"Date cannot be represented in JSON Schema"` |
| `z.nan()` | `"NaN cannot be represented in JSON Schema"` |
| `z.function()` | `"Function types cannot be represented in JSON Schema"` |
| `z.map(...)` | `"Map cannot be represented in JSON Schema"` |
| `z.set(...)` | `"Set cannot be represented in JSON Schema"` |
| `z.custom(...)` | `"Custom types cannot be represented in JSON Schema"` |
| `.transform(fn)` | `"Transforms cannot be represented in JSON Schema"` |
| `z.literal(undefined)` | `"Literal \`undefined\` cannot be represented in JSON Schema"` |
| `z.literal(1n)` | `"BigInt literals cannot be represented in JSON Schema"` |

Recursive schemas (via `z.lazy()`) throw because we set `cycles: "throw"`:

| Zod type | Error message |
|---|---|
| `z.lazy(() => schema)` (recursive) | `"Cycle detected: #/.../<root>"` |

Non-recursive `z.lazy()` resolves transparently to the inner type.

`z.nativeEnum()` doesn't exist in Zod v4. Use `z.enum()`.

---

## Tests

**File:** new `libs/barnum/tests/schema.test.ts`

All tests call `zodToCheckedJsonSchema(schema, "test")` and assert the output with `expect(...).toEqual(...)`. The `label` parameter is `"test"` in all tests; tests that verify error messages use a descriptive label.

### Primitive tests

```ts
import { describe, it, expect } from "vitest";
import { z } from "zod";
import { zodToCheckedJsonSchema } from "../src/schema.js";

const convert = (schema: z.ZodType) => zodToCheckedJsonSchema(schema, "test");

describe("zodToCheckedJsonSchema", () => {
  describe("primitives", () => {
    it("converts z.string()", () => {
      expect(convert(z.string())).toEqual({ type: "string" });
    });

    it("converts z.number()", () => {
      expect(convert(z.number())).toEqual({ type: "number" });
    });

    it("converts z.boolean()", () => {
      expect(convert(z.boolean())).toEqual({ type: "boolean" });
    });

    it("converts z.null()", () => {
      expect(convert(z.null())).toEqual({ type: "null" });
    });

    it("converts z.unknown()", () => {
      expect(convert(z.unknown())).toEqual({});
    });

    it("converts z.any()", () => {
      expect(convert(z.any())).toEqual({});
    });
  });
```

### Literal tests

```ts
  describe("literals", () => {
    it("converts string literal", () => {
      expect(convert(z.literal("hello"))).toEqual({
        type: "string",
        const: "hello",
      });
    });

    it("converts number literal", () => {
      expect(convert(z.literal(42))).toEqual({
        type: "number",
        const: 42,
      });
    });

    it("converts boolean literal", () => {
      expect(convert(z.literal(true))).toEqual({
        type: "boolean",
        const: true,
      });
    });

    it("converts null literal", () => {
      expect(convert(z.literal(null))).toEqual({
        type: "null",
        const: null,
      });
    });
  });
```

### Enum test

```ts
  describe("enum", () => {
    it("converts z.enum()", () => {
      expect(convert(z.enum(["a", "b", "c"]))).toEqual({
        type: "string",
        enum: ["a", "b", "c"],
      });
    });
  });
```

### Container tests

```ts
  describe("containers", () => {
    it("converts z.object()", () => {
      expect(convert(z.object({ a: z.string(), b: z.number() }))).toEqual({
        type: "object",
        properties: {
          a: { type: "string" },
          b: { type: "number" },
        },
        required: ["a", "b"],
        additionalProperties: false,
      });
    });

    it("converts z.object() with optional field", () => {
      expect(
        convert(z.object({ a: z.string(), b: z.number().optional() })),
      ).toEqual({
        type: "object",
        properties: {
          a: { type: "string" },
          b: { type: "number" },
        },
        required: ["a"],
        additionalProperties: false,
      });
    });

    it("converts z.array()", () => {
      expect(convert(z.array(z.number()))).toEqual({
        type: "array",
        items: { type: "number" },
      });
    });

    it("converts z.tuple()", () => {
      expect(convert(z.tuple([z.string(), z.number()]))).toEqual({
        type: "array",
        items: [{ type: "string" }, { type: "number" }],
      });
    });

    it("converts z.record()", () => {
      expect(convert(z.record(z.string(), z.number()))).toEqual({
        type: "object",
        propertyNames: { type: "string" },
        additionalProperties: { type: "number" },
      });
    });
  });
```

### Composition tests

```ts
  describe("composition", () => {
    it("converts z.union()", () => {
      expect(convert(z.union([z.string(), z.number()]))).toEqual({
        anyOf: [{ type: "string" }, { type: "number" }],
      });
    });

    it("converts z.nullable()", () => {
      expect(convert(z.nullable(z.string()))).toEqual({
        anyOf: [{ type: "string" }, { type: "null" }],
      });
    });
  });
```

### Modifier tests (string)

```ts
  describe("string modifiers", () => {
    it("min length", () => {
      expect(convert(z.string().min(3))).toEqual({
        type: "string",
        minLength: 3,
      });
    });

    it("max length", () => {
      expect(convert(z.string().max(10))).toEqual({
        type: "string",
        maxLength: 10,
      });
    });

    it("exact length", () => {
      expect(convert(z.string().length(5))).toEqual({
        type: "string",
        minLength: 5,
        maxLength: 5,
      });
    });

    it("regex", () => {
      expect(convert(z.string().regex(/^foo/))).toEqual({
        type: "string",
        pattern: "^foo",
      });
    });

    it("email format", () => {
      expect(convert(z.string().email())).toEqual({
        type: "string",
        format: "email",
      });
    });

    it("url format", () => {
      expect(convert(z.string().url())).toEqual({
        type: "string",
        format: "uri",
      });
    });

    it("startsWith", () => {
      expect(convert(z.string().startsWith("foo"))).toEqual({
        type: "string",
        pattern: "^foo",
      });
    });

    it("endsWith", () => {
      expect(convert(z.string().endsWith("bar"))).toEqual({
        type: "string",
        pattern: "bar$",
      });
    });
  });
```

### Modifier tests (number)

```ts
  describe("number modifiers", () => {
    it("minimum", () => {
      expect(convert(z.number().min(0))).toEqual({
        type: "number",
        minimum: 0,
      });
    });

    it("maximum", () => {
      expect(convert(z.number().max(100))).toEqual({
        type: "number",
        maximum: 100,
      });
    });

    it("exclusive minimum", () => {
      expect(convert(z.number().gt(0))).toEqual({
        type: "number",
        exclusiveMinimum: 0,
      });
    });

    it("exclusive maximum", () => {
      expect(convert(z.number().lt(100))).toEqual({
        type: "number",
        exclusiveMaximum: 100,
      });
    });

    it("integer", () => {
      expect(convert(z.number().int())).toEqual({ type: "integer" });
    });

    it("multipleOf", () => {
      expect(convert(z.number().multipleOf(5))).toEqual({
        type: "number",
        multipleOf: 5,
      });
    });
  });
```

### Modifier tests (array)

```ts
  describe("array modifiers", () => {
    it("minItems", () => {
      expect(convert(z.array(z.string()).min(1))).toEqual({
        type: "array",
        items: { type: "string" },
        minItems: 1,
      });
    });

    it("maxItems", () => {
      expect(convert(z.array(z.string()).max(10))).toEqual({
        type: "array",
        items: { type: "string" },
        maxItems: 10,
      });
    });
  });
```

### Transparent wrapper tests

```ts
  describe("transparent wrappers", () => {
    it(".default() adds default value", () => {
      expect(convert(z.string().default("hello"))).toEqual({
        type: "string",
        default: "hello",
      });
    });

    it("standalone .optional() produces inner type", () => {
      expect(convert(z.string().optional())).toEqual({
        type: "string",
      });
    });
  });
```

### Rejection tests

```ts
  describe("rejected types", () => {
    it("throws for z.undefined()", () => {
      expect(() => convert(z.undefined())).toThrow(
        /Undefined cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.void()", () => {
      expect(() => convert(z.void())).toThrow(
        /Void cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.bigint()", () => {
      expect(() => convert(z.bigint())).toThrow(
        /BigInt cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.symbol()", () => {
      expect(() => convert(z.symbol())).toThrow(
        /Symbols cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.date()", () => {
      expect(() => convert(z.date())).toThrow(
        /Date cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.function()", () => {
      expect(() => convert(z.function())).toThrow(
        /Function types cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.map()", () => {
      expect(() => convert(z.map(z.string(), z.number()))).toThrow(
        /Map cannot be represented in JSON Schema/,
      );
    });

    it("throws for z.set()", () => {
      expect(() => convert(z.set(z.string()))).toThrow(
        /Set cannot be represented in JSON Schema/,
      );
    });

    it("throws for .transform()", () => {
      expect(() =>
        convert(z.string().transform((s) => parseInt(s, 10))),
      ).toThrow(/Transforms cannot be represented in JSON Schema/);
    });
  });
```

### Nested rejection tests

```ts
  describe("nested rejected types", () => {
    it("throws for rejected type inside object", () => {
      expect(() =>
        convert(z.object({ a: z.function() })),
      ).toThrow(/Function types cannot be represented/);
    });

    it("throws for rejected type inside array", () => {
      expect(() =>
        convert(z.array(z.set(z.string()))),
      ).toThrow(/Set cannot be represented/);
    });

    it("throws for rejected type inside union", () => {
      expect(() =>
        convert(z.union([z.string(), z.undefined()])),
      ).toThrow(/Undefined cannot be represented/);
    });
  });
```

### Pre-validation rejection tests

```ts
  describe("pre-validation rejections", () => {
    it("throws for z.intersection()", () => {
      expect(() =>
        convert(
          z.intersection(
            z.object({ a: z.string() }),
            z.object({ b: z.number() }),
          ),
        ),
      ).toThrow(/z\.intersection\(\) is not supported/);
    });

    it("throws for .refine()", () => {
      expect(() =>
        convert(z.string().refine((s) => s.length > 0)),
      ).toThrow(/\.refine\(\) and \.superRefine\(\) are not supported/);
    });

    it("throws for .superRefine()", () => {
      expect(() =>
        convert(z.string().superRefine(() => {})),
      ).toThrow(/\.refine\(\) and \.superRefine\(\) are not supported/);
    });

    it("throws for .refine() on an object", () => {
      expect(() =>
        convert(
          z.object({ a: z.string() }).refine((o) => o.a.length > 0),
        ),
      ).toThrow(/\.refine\(\) and \.superRefine\(\) are not supported/);
    });

    it("throws for .refine() nested inside an object value", () => {
      expect(() =>
        convert(
          z.object({ a: z.string().refine((s) => s.length > 0) }),
        ),
      ).toThrow(/\.refine\(\) and \.superRefine\(\) are not supported/);
    });

    it("throws for .refine() nested inside an array", () => {
      expect(() =>
        convert(
          z.array(z.string().refine((s) => s.length > 0)),
        ),
      ).toThrow(/\.refine\(\) and \.superRefine\(\) are not supported/);
    });

    it("throws for intersection nested inside a union", () => {
      expect(() =>
        convert(
          z.union([
            z.string(),
            z.intersection(
              z.object({ a: z.string() }),
              z.object({ b: z.number() }),
            ),
          ]),
        ),
      ).toThrow(/z\.intersection\(\) is not supported/);
    });

    it("allows built-in checks like .min() alongside rejection of .refine()", () => {
      // .min() is a built-in check, not custom — should not be rejected
      expect(convert(z.string().min(3))).toEqual({
        type: "string",
        minLength: 3,
      });
    });
  });
```

### Error message wrapping test

```ts
  describe("error messages", () => {
    it("wraps the error with the handler label", () => {
      expect(() =>
        zodToCheckedJsonSchema(z.undefined(), "myHandler:input"),
      ).toThrow(
        'Handler "myHandler:input": Zod schema cannot be converted to JSON Schema: Undefined cannot be represented in JSON Schema',
      );
    });
  });
```

### `$schema` stripping test

```ts
  describe("output format", () => {
    it("does not include $schema property", () => {
      const result = convert(z.string());
      expect(result).not.toHaveProperty("$schema");
    });
  });
```

### Domain-specific pattern tests

```ts
  describe("domain patterns", () => {
    it("tagged union (HasErrors/Clean)", () => {
      const TypeErrorValidator = z.object({
        file: z.string(),
        message: z.string(),
      });
      const schema = z.union([
        z.object({
          kind: z.literal("HasErrors"),
          value: z.array(TypeErrorValidator),
        }),
        z.object({
          kind: z.literal("Clean"),
          value: z.null(),
        }),
      ]);
      const result = convert(schema);
      expect(result.anyOf).toHaveLength(2);
      expect((result.anyOf as Record<string, unknown>[])[0]).toEqual({
        type: "object",
        properties: {
          kind: { type: "string", const: "HasErrors" },
          value: {
            type: "array",
            items: {
              type: "object",
              properties: {
                file: { type: "string" },
                message: { type: "string" },
              },
              required: ["file", "message"],
              additionalProperties: false,
            },
          },
        },
        required: ["kind", "value"],
        additionalProperties: false,
      });
      expect((result.anyOf as Record<string, unknown>[])[1]).toEqual({
        type: "object",
        properties: {
          kind: { type: "string", const: "Clean" },
          value: { type: "null" },
        },
        required: ["kind", "value"],
        additionalProperties: false,
      });
    });

    it("Result<string, string>", () => {
      const schema = z.union([
        z.object({ kind: z.literal("Ok"), value: z.string() }),
        z.object({ kind: z.literal("Err"), value: z.string() }),
      ]);
      const result = convert(schema);
      expect(result.anyOf).toHaveLength(2);
    });

    it("JudgmentResult (heterogeneous union)", () => {
      const schema = z.union([
        z.object({ approved: z.literal(true) }),
        z.object({
          approved: z.literal(false),
          instructions: z.string(),
        }),
      ]);
      const result = convert(schema);
      expect(result.anyOf).toHaveLength(2);
      // First variant: { approved: true }
      expect((result.anyOf as Record<string, unknown>[])[0]).toEqual({
        type: "object",
        properties: {
          approved: { type: "boolean", const: true },
        },
        required: ["approved"],
        additionalProperties: false,
      });
      // Second variant: { approved: false, instructions: string }
      expect((result.anyOf as Record<string, unknown>[])[1]).toEqual({
        type: "object",
        properties: {
          approved: { type: "boolean", const: false },
          instructions: { type: "string" },
        },
        required: ["approved", "instructions"],
        additionalProperties: false,
      });
    });
  });
```

### Close the describe block

```ts
});
```
