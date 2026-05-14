# Recursive Zod Schemas

Support recursive `z.lazy()` schemas by tracking previously encountered schema objects and emitting JSON Schema `$defs`/`$ref` instead of inlining infinitely.

## Motivation

Currently, `zodToCheckedJsonSchema` passes `cycles: "throw"` to Zod's `toJSONSchema()`. Any schema that references itself via `z.lazy()` throws "Cycle detected" at handler definition time. This means handlers cannot validate recursive data structures — trees, linked lists, nested comment threads, ASTs, etc.

```ts
// This is rejected today:
const treeNodeSchema: z.ZodType<TreeNode> = z.object({
  value: z.string(),
  children: z.array(z.lazy(() => treeNodeSchema)),
});
```

Real workflows often deal with recursive structures: file system trees, org charts, threaded conversations, nested JSON configs. Forcing users to validate these as `z.unknown()` and cast defeats the purpose of the schema system.

## Current state

**`libs/barnum/src/schema.ts`:**

1. `assertNoUnsupportedPatterns()` (line 41) — traverses the schema tree with a `WeakSet<z.ZodType>` visited set. Already handles cycles safely (early return on revisit).
2. `zodToCheckedJsonSchema()` (line 91) — calls `toJSONSchema()` with `cycles: "throw"` and `reused: "inline"`.

**Rust side (`crates/barnum_engine`):**

The Rust runtime validates handler inputs/outputs against JSON Schema using a JSON Schema validator crate. Whether it already supports `$ref`/`$defs` needs verification — most JSON Schema Draft 7 validators do.

## Proposed change

Switch from `cycles: "throw"` to `cycles: "ref"` (or equivalent) in the `toJSONSchema()` call. This makes Zod emit `$defs` at the schema root with `$ref` pointers for recursive references instead of throwing.

```ts
raw = toJSONSchema(schema, {
  target: "draft-07",
  unrepresentable: "throw",
  io: "output",
  cycles: "ref",       // was: "throw"
  reused: "inline",    // non-recursive reuse still inlines
}) as Record<string, unknown>;
```

### What changes

1. **`libs/barnum/src/schema.ts`**: Change `cycles: "throw"` to `cycles: "ref"`.  Stop stripping `$defs` from the output (currently only `$schema` is stripped, so this may already work).

2. **Rust JSON Schema validation**: Verify the validator crate handles `$defs`/`$ref` correctly. If it uses `jsonschema` (the most common Rust crate), Draft 7 `$ref` resolution is built in.

3. **Config serialization**: `$defs` will add to config size. Since config now goes through a temp file (not CLI args), this is not a problem.

## Open questions

1. **Does Zod's `cycles: "ref"` produce valid Draft 7?** Zod 4 might emit Draft 2020-12 `$defs` vs Draft 7 `definitions`. Need to verify which key it uses and whether the Rust validator resolves both.

2. **Does `reused: "inline"` interact with `cycles: "ref"`?** If a schema is both reused (appears in multiple places) and recursive, does Zod correctly emit a `$def` for it? Or does `reused: "inline"` prevent the `$def` from being created?

3. **What does the Zod `cycles` option actually accept?** The exact API needs verification against Zod 4.3.6 source. It might be `"ref"`, `"$ref"`, or something else.

4. **Rust-side `$ref` resolution scope**: JSON Schema `$ref` resolution is relative to the schema root. Since each handler's schema is a standalone document (not embedded in a larger schema), `$ref: "#/$defs/TreeNode"` should resolve correctly. Verify this with the Rust validator.

## Validation

- Add a test in `libs/barnum/tests/schema.test.ts` with a recursive `z.lazy()` schema (tree node, linked list).
- Add an execution test that actually passes recursive data through a handler and validates it round-trips correctly through the Rust engine.
- Verify the Rust JSON Schema validator resolves `$ref` within `$defs` for a recursive schema.
