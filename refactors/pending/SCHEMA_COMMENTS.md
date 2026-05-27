# Schema Comments

## Motivation

JSON Schema supports `$comment` (Draft-07+) for author-facing annotations that validators ignore. Zod has no equivalent — `.describe()` maps to `description`, which is a user-facing documentation field with different semantics.

When writing handler validators, authors sometimes need to annotate fields with context that's useful for debugging or schema inspection ("this field is deprecated", "must match the format from X system", "see handler Y for the producer of this shape") without that annotation appearing as user-visible documentation.

Currently there's no way to add `$comment` to the JSON Schema that `zodToCheckedJsonSchema` produces.

## Current state

`zodToCheckedJsonSchema` (`libs/barnum/src/schema.ts`) calls `toJSONSchema()` from Zod v4, strips `$schema`, and returns the result. It performs pre-validation (rejects intersections and refinements) but does no post-processing to inject additional JSON Schema fields.

Zod has no `.comment()` method. The only annotation method is `.describe(text)` which produces `{ "description": "text" }` in the JSON Schema output.

The Rust side (`crates/barnum_ast/src/json_schema.rs`) stores the schema as an opaque `Value` — it doesn't parse or strip any fields, so `$comment` would survive the round-trip without Rust changes.

## Proposed design

Add a `comment` helper that attaches a `$comment` string to any Zod schema via Zod's metadata system, and teach `zodToCheckedJsonSchema` to walk the output and inject `$comment` where metadata is present.

### Option A: Post-process via Zod metadata

Zod v4 has `.meta()` for arbitrary metadata. Attach comments there:

```ts
// Helper
export function withComment<T extends z.ZodType>(schema: T, comment: string): T {
  return schema.meta({ $comment: comment });
}

// Usage
const batchStateSchema = z.object({
  batch: withComment(z.array(itemSchema), "Current items processing in parallel"),
  rest: z.array(itemSchema),
});
```

Then in `zodToCheckedJsonSchema`, after `toJSONSchema()` produces the JSON Schema, walk the Zod tree and the JSON Schema tree in parallel, injecting `$comment` from any `.meta({ $comment })` annotations.

**Complication:** The parallel walk is fragile. Zod's tree and the JSON Schema output don't have a 1:1 node correspondence after `toJSONSchema` flattens/inlines things.

### Option B: Post-process the JSON Schema directly

Skip the Zod annotation entirely. Provide a utility that decorates the JSON Schema output:

```ts
const schema = zodToCheckedJsonSchema(myValidator, "myHandler input");
schema.properties.batch.$comment = "Current items processing in parallel";
```

This is manual but explicit. No magic parallel walks.

**Complication:** Breaks the encapsulation of `zodToCheckedJsonSchema` — callers would need to post-process the output, but today `createHandler` calls `zodToCheckedJsonSchema` internally and embeds the result directly.

### Option C: Extend zodToCheckedJsonSchema to accept a comment map

```ts
export function zodToCheckedJsonSchema(
  schema: z.ZodType,
  label: string,
  options?: { comment?: string },
): JSONSchema7 {
  // ... existing logic ...
  if (options?.comment) {
    raw.$comment = options.comment;
  }
  return rest as JSONSchema7;
}
```

This only supports a top-level comment per schema. Sufficient for handler-level annotations ("this handler is deprecated", "output shape matches X"). Doesn't support per-field comments.

## Decision

Option A (Zod `.describe()` → JSON Schema `$comment`/`description`). The only consumer is the JSON Schema as printed for the LLM. Everything else in this doc is unrelated scope creep — remove or ignore.