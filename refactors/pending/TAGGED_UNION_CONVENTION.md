# Tagged Union Convention: `{ kind, value }` + Phantom `__def`

**Blocks:** nothing
**Blocked by:** CONTRAVARIANT_CASE_HANDLERS.md (for full benefit in branch cases)

## Motivation

Discriminated unions in the codebase have arbitrary shapes per variant:

```ts
type ClassifyResult =
  | { kind: "HasErrors"; errors: TypeError[] }
  | { kind: "Clean" };
```

HasErrors has `errors`, Clean has nothing. Each variant is a different shape. This means:
- Branch case handlers must know the variant structure to extract fields (`extractField<HasErrors, "errors">("errors")`)
- The standalone `branch()` derives its input type from handler inputs — when handlers are input-agnostic (drop, done), the derivation fails
- No way to recover the full union definition from a single variant

## Proposed convention

All discriminated unions use `{ kind: K; value: T }`, with a phantom `__def` field carrying the full variant map:

```ts
/** Standard tagged union type. Each variant has { kind, value, __def }. */
type TaggedUnion<TDef extends Record<string, unknown>> = {
  [K in keyof TDef & string]: { kind: K; value: TDef[K]; __def?: TDef };
}[keyof TDef & string];
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

### Three things this enables

1. **Standardized structure** — every variant has `kind` and `value`, nothing else (plus phantom `__def`)
2. **Branch auto-unwraps** — branch extracts `value` before passing to the case handler, so handlers receive the payload directly
3. **`__def` carries the full definition** — the standalone `branch()` can extract the variant map from any handler that has a concrete input type, recovering the full union even when other handlers use `drop()`

### Precedent already in the codebase

`LoopResult`, `recur()`, `done()`, and `tag()` already use `{ kind, value }`:

```ts
// libs/barnum/src/ast.ts:473
type LoopResult<TContinue, TBreak> =
  | { kind: "Continue"; value: TContinue }
  | { kind: "Break"; value: TBreak };

// libs/barnum/src/builtins.ts:65
function recur<TValue>(): TypedAction<TValue, { kind: "Continue"; value: TValue }>
function done<TValue>(): TypedAction<TValue, { kind: "Break"; value: TValue }>
function tag<TValue, TKind extends string>(kind: TKind): TypedAction<TValue, { kind: TKind; value: TValue }>
```

## Changes

### 1. Add `TaggedUnion` and `ExtractDef` to `ast.ts`

**File:** `libs/barnum/src/ast.ts`

```ts
// New type utilities

/** Standard tagged union. Variants carry { kind, value } plus phantom __def. */
export type TaggedUnion<TDef extends Record<string, unknown>> = {
  [K in keyof TDef & string]: { kind: K; value: TDef[K]; __def?: TDef };
}[keyof TDef & string];

/** Extract the variant map definition from a tagged union's phantom __def. */
type ExtractDef<T> = T extends { __def?: infer D } ? D : never;
```

### 2. Branch auto-unwraps `value`

**File:** `libs/barnum/src/ast.ts` — standalone `branch()` and postfix `.branch()` runtime implementations

The TypeScript-side branch inserts `ExtractField("value")` before each case handler in the AST. No Rust executor changes needed — the executor still passes the full value, but the first step of each case extracts `value`.

Before (runtime implementation):
```ts
// libs/barnum/src/ast.ts — branchMethod (~line 208)
function branchMethod(this: TypedAction, cases: Record<string, Action>): TypedAction {
  return typedAction({ kind: "Chain", first: this, rest: { kind: "Branch", cases } });
}
```

After:
```ts
function branchMethod(this: TypedAction, cases: Record<string, Action>): TypedAction {
  // Auto-unwrap: insert ExtractField("value") before each case handler
  const unwrappedCases: Record<string, Action> = {};
  for (const key of Object.keys(cases)) {
    unwrappedCases[key] = {
      kind: "Chain",
      first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "value" } } },
      rest: cases[key],
    };
  }
  return typedAction({ kind: "Chain", first: this, rest: { kind: "Branch", cases: unwrappedCases } });
}
```

Same change for the standalone `branch()` function body.

### 3. Update `BranchInput` to use `__def`

**File:** `libs/barnum/src/ast.ts`

Before:
```ts
type BranchInput<TCases> = {
  [K in keyof TCases & string]: { kind: K } & ExtractInput<TCases[K]>;
}[keyof TCases & string];
```

After:
```ts
/**
 * Compute the branch input type. Prefers __def (the union definition carried
 * as phantom data) when available — this recovers the full union even when
 * some case handlers are input-agnostic (drop, done).
 *
 * Falls back to { kind: K } & ExtractInput<handler> when no handler carries __def.
 */
type BranchDefFromCases<TCases> = ExtractDef<ExtractInput<TCases[keyof TCases & string]>>;

type BranchInput<TCases> =
  BranchDefFromCases<TCases> extends Record<string, unknown>
    ? TaggedUnion<BranchDefFromCases<TCases>>
    : { [K in keyof TCases & string]: { kind: K } & ExtractInput<TCases[K]> }[keyof TCases & string];
```

If any case handler carries `__def`, the full union is reconstructed from the definition. Otherwise, falls back to the current per-handler intersection.

### 4. Update handler return types

Every handler that produces a discriminated union changes from arbitrary shapes to `{ kind, value }`.

#### `libs/barnum/tests/handlers.ts`

Before:
```ts
export type ClassifyResult =
  | { kind: "HasErrors"; errors: TypeError[] }
  | { kind: "Clean" };

export const classifyErrors = createHandler({
  inputValidator: z.array(z.object({ file: z.string(), message: z.string() })),
  handle: async ({ value }): Promise<ClassifyResult> =>
    value.length > 0
      ? { kind: "HasErrors", errors: value }
      : { kind: "Clean" },
}, "classifyErrors");
```

After:
```ts
export type ClassifyResultDef = {
  HasErrors: TypeError[];
  Clean: void;
};
export type ClassifyResult = TaggedUnion<ClassifyResultDef>;

export const classifyErrors = createHandler({
  inputValidator: z.array(z.object({ file: z.string(), message: z.string() })),
  handle: async ({ value }): Promise<ClassifyResult> =>
    value.length > 0
      ? { kind: "HasErrors", value }
      : { kind: "Clean", value: undefined },
}, "classifyErrors");
```

#### `demos/convert-folder-to-ts/handlers/type-check-fix.ts`

Before:
```ts
export type ClassifyResult =
  | { kind: "HasErrors"; errors: TypeError[] }
  | { kind: "Clean" };

// handle returns:
return { kind: "HasErrors", errors };
return { kind: "Clean" };
```

After:
```ts
export type ClassifyResultDef = {
  HasErrors: TypeError[];
  Clean: void;
};
export type ClassifyResult = TaggedUnion<ClassifyResultDef>;

// handle returns:
return { kind: "HasErrors", value: errors };
return { kind: "Clean", value: undefined };
```

#### `demos/identify-and-address-refactors/handlers/refactor.ts`

Before:
```ts
export type ClassifyJudgmentResult =
  | { kind: "Approved" }
  | { kind: "NeedsWork"; instructions: string };

// handle returns:
return { kind: "Approved" };
return { kind: "NeedsWork", instructions: judgment.instructions };
```

After:
```ts
export type ClassifyJudgmentResultDef = {
  Approved: void;
  NeedsWork: string;
};
export type ClassifyJudgmentResult = TaggedUnion<ClassifyJudgmentResultDef>;

// handle returns:
return { kind: "Approved", value: undefined };
return { kind: "NeedsWork", value: judgment.instructions };
```

### 5. Update branch case handlers — remove `extractField`

Since branch auto-unwraps `value`, case handlers receive the payload directly. No more `extractField("errors")`.

#### `libs/barnum/tests/types.test.ts`

Before:
```ts
classifyErrors.branch({
  HasErrors: pipe(
    extractField<Extract<ClassifyResult, { kind: "HasErrors" }>, "errors">("errors"),
    forEach(fix),
  ),
  Clean: drop(),
})
```

After:
```ts
// Branch auto-unwraps value. HasErrors handler receives TypeError[] directly.
classifyErrors.branch({
  HasErrors: pipe(forEach(fix)),  // receives TypeError[], not { kind, errors }
  Clean: drop(),
})
```

Wait — `forEach(fix)` takes `TypeError[]` and applies `fix` to each element. That's correct. But the pipe has a single step, so it simplifies to just `forEach(fix)`:

```ts
classifyErrors.branch({
  HasErrors: forEach(fix),
  Clean: drop(),
})
```

#### `libs/barnum/tests/steps.test.ts`

Before:
```ts
branch({
  HasErrors: pipe(
    extractField<HasErrors, "errors">("errors"),
    forEach(fix),
    recur<any>(),
  ),
  Clean: done<Clean>(),
})
```

After:
```ts
branch({
  HasErrors: pipe(forEach(fix), recur<any>()),  // receives TypeError[] directly
  Clean: done(),  // contravariant: no type param on input side
                   // done() output is { kind: "Break"; value: unknown } — loop output is unknown
                   // If loop output type matters, use done<void>() to match Clean's payload
})
```

#### `demos/convert-folder-to-ts/run.ts`

Before:
```ts
pipe(typeCheck, classifyErrors).branch({
  HasErrors: pipe(
    extractField<Extract<ClassifyResult, { kind: "HasErrors" }>, "errors">("errors"),
    forEach(fix).drop(),
    recur<any>(),
  ),
  Clean: done<any>(),
})
```

After:
```ts
pipe(typeCheck, classifyErrors).branch({
  HasErrors: pipe(forEach(fix).drop(), recur<any>()),
  Clean: done<any>(),
})
```

#### `demos/identify-and-address-refactors/run.ts`

Before:
```ts
pipe(drop<any>(), judgeRefactor, classifyJudgment).branch({
  NeedsWork: pipe(
    extractField<Extract<ClassifyJudgmentResult, { kind: "NeedsWork" }>, "instructions">("instructions"),
    applyFeedback.drop(), stepRef("TypeCheck"), recur<any>(),
  ),
  Approved: done<any>(),
})
```

After:
```ts
pipe(drop<any>(), judgeRefactor, classifyJudgment).branch({
  NeedsWork: pipe(applyFeedback.drop(), stepRef("TypeCheck"), recur<any>()),
  Approved: done<any>(),
})
```

The `applyFeedback` handler accepts `z.string()` (a string). The `NeedsWork` variant's value is `string` (from `ClassifyJudgmentResultDef`). Branch auto-unwraps, so the handler receives the string directly.

### 6. Update `LoopResult` to use `TaggedUnion`

`LoopResult` already follows the `{ kind, value }` convention. Add `__def` for consistency.

Before (`libs/barnum/src/ast.ts:473`):
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

The expanded type is identical plus `__def`. No behavioral change.

## Files to change summary

| File | What changes |
|------|-------------|
| `libs/barnum/src/ast.ts` | Add `TaggedUnion`, `ExtractDef`; update `BranchInput`; update `LoopResult`; branch auto-unwraps `value` in runtime impls |
| `libs/barnum/src/builtins.ts` | No changes — `tag()`, `recur()`, `done()` already produce `{ kind, value }` |
| `libs/barnum/tests/handlers.ts` | `ClassifyResult` → `TaggedUnion<ClassifyResultDef>`; handler returns `{ kind, value }` |
| `libs/barnum/tests/types.test.ts` | Remove `extractField` from branch cases; update type assertions for ClassifyResult shape; update BranchInput assertions |
| `libs/barnum/tests/patterns.test.ts` | Update branch test cases for auto-unwrapping |
| `libs/barnum/tests/steps.test.ts` | Remove `extractField` from branch cases |
| `libs/barnum/tests/round-trip.test.ts` | Update Branch test constant to use `{ kind, value }` shape |
| `demos/convert-folder-to-ts/handlers/type-check-fix.ts` | `ClassifyResult` → `TaggedUnion`; handler returns `{ kind, value }` |
| `demos/convert-folder-to-ts/run.ts` | Remove `extractField` from branch cases |
| `demos/identify-and-address-refactors/handlers/refactor.ts` | `ClassifyJudgmentResult` → `TaggedUnion`; handler returns `{ kind, value }` |
| `demos/identify-and-address-refactors/run.ts` | Remove `extractField` from branch cases |

## Order of operations

1. **Add `TaggedUnion`, `ExtractDef` types** — pure additions, nothing breaks
2. **Convert handler return types** to `TaggedUnion<Def>` — handlers change, but branch cases still work because extractField extracts from the new `value` field instead of the old custom fields
3. **Update branch to auto-unwrap `value`** — runtime implementation change, removes the need for extractField in case handlers
4. **Remove `extractField` from branch cases** — cleanup, enabled by step 3
5. **Update `BranchInput` to use `__def`** — standalone `branch()` no longer needs typed drops (requires CONTRAVARIANT_CASE_HANDLERS for full benefit)
6. **Update `LoopResult`** to use `TaggedUnion` — consistency

Steps 2 and 3 can be combined. Steps 4 and 5 can be combined.

## Open questions

1. **`tag()` and `__def`** — The `tag("Ok")` builtin produces `{ kind: "Ok"; value: T }` but doesn't add `__def`. Should it? `tag` wraps a single variant — it doesn't know the full union. You'd need a `tagAs<TDef>("Ok")` variant that knows the definition. Or rely on the handler's return type annotation to provide `__def` via the `TaggedUnion` type.

2. **Rust executor** — No changes needed. The executor passes the full value to each case. The auto-unwrap is handled by inserting `ExtractField("value")` in the TypeScript AST. But the Rust-side type schema (`barnum_ast`) may need updating if it validates variant shapes. Need to check.

3. **`void` vs `undefined` vs `null` for empty variants** — `{ kind: "Clean"; value: void }` — at runtime, `value` would be `undefined` (since `void` is `undefined` in TS). The handler returns `{ kind: "Clean", value: undefined }`. The Rust executor would see `"value": null` in JSON. Need to verify serde handles this.

4. **Migration path** — This is a breaking change to handler return types. Since we don't care about backward compatibility (per CLAUDE.md), this is fine. But it touches many files. Break the implementation into small commits per the branching strategy.
