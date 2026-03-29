# Phantom `__def` on Tagged Unions

**Blocks:** nothing
**Blocked by:** TAGGED_UNION_CONVENTION.md (needs `{ kind, value }` convention first)

## Motivation

After the `{ kind, value }` convention, unions are standardized but each variant doesn't carry information about the full union it belongs to. In Rust, `Option::None` is namespaced to `Option` — you can recover the full enum from any variant. In TypeScript, `{ kind: "None"; value: void }` is structurally identical regardless of which union it belongs to.

Two problems this causes:

1. **`tag()` doesn't know the full union** — `tag("Ok")` produces `{ kind: "Ok"; value: T }` but has no idea what the other variants are. You can't validate at the type level that "Ok" is a valid variant of the target union.

2. **Inference stability** — After CONTRAVARIANT_CASE_HANDLERS, the postfix `.branch()` signature uses `KindOf<Out>` (a conditional type distributing over the union) and `Extract<Out, { kind: K }>` (another conditional). These work, but TS sometimes struggles to infer generics through nested conditional types. With `__def`, the constraint becomes `keyof ExtractDef<Out>` (plain `keyof` on a record) and `ExtractDef<Out>[K]` (simple indexing) — no conditional types at all.

## Proposed change

Add a phantom `__def` field to tagged union variants, carrying the full variant map:

```ts
/** Standard tagged union. Variants carry { kind, value } plus phantom __def. */
export type TaggedUnion<TDef extends Record<string, unknown>> = {
  [K in keyof TDef & string]: { kind: K; value: TDef[K]; __def?: TDef };
}[keyof TDef & string];

/** Extract the variant map definition from a tagged union's phantom __def. */
export type ExtractDef<T> = T extends { __def?: infer D } ? D : never;
```

Unions are defined as variant maps:

```ts
type ClassifyResultDef = {
  HasErrors: TypeError[];
  Clean: void;
};

type ClassifyResult = TaggedUnion<ClassifyResultDef>;
// = { kind: "HasErrors"; value: TypeError[]; __def?: ClassifyResultDef }
// | { kind: "Clean"; value: void; __def?: ClassifyResultDef }
```

## Changes

### 1. Add `TaggedUnion` and `ExtractDef` to `ast.ts`

**File:** `libs/barnum/src/ast.ts`

```ts
export type TaggedUnion<TDef extends Record<string, unknown>> = {
  [K in keyof TDef & string]: { kind: K; value: TDef[K]; __def?: TDef };
}[keyof TDef & string];

export type ExtractDef<T> = T extends { __def?: infer D } ? D : never;
```

### 2. Convert union types to use `TaggedUnion<Def>`

#### `libs/barnum/tests/handlers.ts`

Before:
```ts
export type ClassifyResult =
  | { kind: "HasErrors"; value: TypeError[] }
  | { kind: "Clean"; value: void };
```

After:
```ts
export type ClassifyResultDef = {
  HasErrors: TypeError[];
  Clean: void;
};
export type ClassifyResult = TaggedUnion<ClassifyResultDef>;
```

Same pattern for demos:
- `demos/convert-folder-to-ts/handlers/type-check-fix.ts`
- `demos/identify-and-address-refactors/handlers/refactor.ts`

#### `libs/barnum/src/ast.ts` — `LoopResult`

Before:
```ts
export type LoopResult<TContinue, TBreak> =
  | { kind: "Continue"; value: TContinue }
  | { kind: "Break"; value: TBreak };
```

After:
```ts
type LoopResultDef<TContinue, TBreak> = {
  Continue: TContinue;
  Break: TBreak;
};

export type LoopResult<TContinue, TBreak> = TaggedUnion<LoopResultDef<TContinue, TBreak>>;
```

### 3. Update `tag()` to know the full union

`tag("Ok")` today produces `{ kind: "Ok"; value: T }` but has no idea what union it belongs to. Every call to `tag()` should know the full union definition.

Before:
```ts
function tag<TValue, TKind extends string>(kind: TKind): TypedAction<TValue, { kind: TKind; value: TValue }>
```

After:
```ts
function tag<TDef extends Record<string, unknown>, TKind extends keyof TDef & string>(
  kind: TKind,
): TypedAction<TDef[TKind], TaggedUnion<TDef>>
```

Usage:
```ts
// Before
verify.tag("Ok")  // TypedAction<..., { kind: "Ok"; value: { verified: boolean } }>

// After — tag knows the full union
type ResultDef = { Ok: { verified: boolean }; Err: string };
verify.then(tag<ResultDef, "Ok">("Ok"))  // TypedAction<..., TaggedUnion<ResultDef>>
```

The output type is the full `TaggedUnion<TDef>`, not just the single variant. This is correct: `tag("Ok")` produces a value that IS a member of the full union, and the type should reflect that.

### 4. Improve postfix `.branch()` using `ExtractDef`

After `__def`, the postfix `.branch()` signature can avoid conditional types entirely:

Before (from CONTRAVARIANT_CASE_HANDLERS + TAGGED_UNION_CONVENTION):
```ts
type KindOf<T> = T extends { kind: infer K extends string } ? K : never;
type UnwrapVariant<T> = T extends { value: infer V } ? V : T;

branch<TCases extends { [K in KindOf<Out>]: CaseHandler<UnwrapVariant<Extract<Out, { kind: K }>>> }>(
  cases: TCases,
): ...;
```

After (using `ExtractDef`):
```ts
branch<TCases extends { [K in keyof ExtractDef<Out> & string]: CaseHandler<ExtractDef<Out>[K]> }>(
  cases: TCases,
): ...;
```

`keyof ExtractDef<Out>` replaces `KindOf<Out>` (no conditional). `ExtractDef<Out>[K]` replaces `UnwrapVariant<Extract<Out, { kind: K }>>` (no conditional). Simpler for TS to infer through.

Falls back to `Record<string, Action>` when `ExtractDef<Out>` is `never` (non-`TaggedUnion` output).

## Implementation strategy: test-first

### Commit 1: Add failing tests

The tests should demonstrate that branch inference actually improves — not just that types were added. The key behavioral change: `tag()` produces output that `.branch()` can decompose exhaustively via `__def`, and `.branch()` uses `ExtractDef` for simpler inference.

Add to `libs/barnum/tests/types.test.ts`:

```ts
describe("phantom __def: branch inference via ExtractDef", () => {
  it("tag() output enables exhaustive .branch() decomposition", () => {
    type ResultDef = { Ok: string; Err: number };

    // tag() knows the full union. Its output is TaggedUnion<ResultDef>.
    // .branch() derives case keys and payload types from __def on the output —
    // it requires both "Ok" and "Err" cases because __def carries the full definition.
    // @ts-expect-error — remove after implementing: tag<TDef, K>() doesn't exist yet
    const tagged: TypedAction<string, TaggedUnion<ResultDef>> = tag<ResultDef, "Ok">("Ok");

    // @ts-expect-error — remove after implementing: .branch() can't decompose via __def yet
    tagged.branch({
      Ok: drop(),
      Err: drop(),
    });
  });

  it(".branch() output type inferred through ExtractDef (no conditional types)", () => {
    // After __def, .branch() constraint uses keyof ExtractDef<Out> instead of KindOf<Out>.
    // This test verifies output type inference works through the simpler path.
    // forEach(fix) output is void[], drop() output is never. Union is void[].
    const action = classifyErrors.branch({
      HasErrors: forEach(fix),
      Clean: drop(),
    });
    // @ts-expect-error — remove after implementing: output type inference via ExtractDef-based .branch()
    assertExact<IsExact<ExtractOutput<typeof action>, void[]>>();
  });
});
```

Test 1: Currently `tag<ResultDef, "Ok">()` doesn't exist → `@ts-expect-error` suppresses. After implementing, tag produces `TaggedUnion<ResultDef>` with `__def`, `.branch()` decomposes it exhaustively, and the test compiles.

Test 2: After `__def`, `.branch()` infers output types through `ExtractDef<Out>[K]` (plain index) instead of `UnwrapVariant<Extract<Out, { kind: K }>>` (nested conditionals). The assertion verifies the inference chain works end-to-end.

### Commit 2: Implement

1. Add `TaggedUnion`, `ExtractDef` to `ast.ts`
2. Convert union types to use `TaggedUnion<Def>`
3. Update `tag()` signature
4. Update `LoopResult` to use `TaggedUnion`
5. Update postfix `.branch()` to use `ExtractDef`
6. **Remove `@ts-expect-error`** from tests — they now compile, proving the fix works.

## Files to change

| File | What changes |
|------|-------------|
| `libs/barnum/src/ast.ts` | Add `TaggedUnion`, `ExtractDef`; update `LoopResult`; update `.branch()` signature to use `ExtractDef` |
| `libs/barnum/src/builtins.ts` | Update `tag()` signature to `tag<TDef, TKind>` |
| `libs/barnum/tests/handlers.ts` | `ClassifyResult = TaggedUnion<ClassifyResultDef>` |
| `libs/barnum/tests/types.test.ts` | Add failing tests (commit 1); remove `@ts-expect-error` (commit 2); update type assertions for `__def` |
| `demos/convert-folder-to-ts/handlers/type-check-fix.ts` | Use `TaggedUnion<Def>` |
| `demos/identify-and-address-refactors/handlers/refactor.ts` | Use `TaggedUnion<Def>` |

## Deferred features

1. **Rust-side deserialization** — If the TS type system knows a value is `TaggedUnion<OptionDef>`, the Rust executor could deserialize into a proper Rust `Option<T>` instead of generic JSON. The `__def` phantom doesn't exist at runtime, but the variant map definition could be embedded in the config schema, enabling the Rust side to use precise types. This eliminates the "everything is JSON" limitation.

## Open questions

1. **`__def` at runtime** — `__def` is phantom (optional, never assigned). It doesn't appear in serialized JSON. If Rust-side deserialization needs the variant map, it would come from the config schema, not from the value itself. Is this sufficient?

2. **Tag output type** — `tag<TDef, "Ok">("Ok")` outputs `TaggedUnion<TDef>` (the full union), not just `{ kind: "Ok"; value: T; __def?: TDef }` (the single variant). This is correct for type compatibility — a tagged value IS a member of the union. But it means the output type is wider than the actual runtime value. Acceptable?
