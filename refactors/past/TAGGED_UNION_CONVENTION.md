# Tagged Union Convention: `{ kind, value }`

**Blocks:** nothing
**Blocked by:** CONTRAVARIANT_CASE_HANDLERS.md (branch validates handler types, so removing `extractField` actually changes behavior)

## Motivation

Discriminated unions in the codebase have arbitrary shapes per variant:

```ts
type ClassifyResult =
  | { kind: "HasErrors"; errors: TypeError[] }
  | { kind: "Clean" };
```

HasErrors has `errors`, Clean has nothing. Each variant is a different shape. This means:
- Branch case handlers must know the variant structure to extract fields (`extractField<HasErrors, "errors">("errors")`)
- Every union has a unique structure — no standard way to interact with variants

## Proposed convention

All discriminated unions use `{ kind: K; value: T }`:

```ts
type ClassifyResult =
  | { kind: "HasErrors"; value: TypeError[] }
  | { kind: "Clean"; value: void };
```

### Two things this enables

1. **Standardized structure** — every variant has `kind` and `value`, nothing else
2. **Branch auto-unwraps `value`** — branch extracts `value` before passing to the case handler, so handlers receive the payload directly. No more `extractField`.

### Precedent already in the codebase

`LoopResult`, `recur()`, `done()`, and `tag()` already use `{ kind, value }`:

```ts
// libs/barnum/src/ast.ts
type LoopResult<TContinue, TBreak> =
  | { kind: "Continue"; value: TContinue }
  | { kind: "Break"; value: TBreak };

// libs/barnum/src/builtins.ts
function recur<TValue>(): TypedAction<TValue, { kind: "Continue"; value: TValue }>
function done<TValue>(): TypedAction<TValue, { kind: "Break"; value: TValue }>
function tag<TValue, TKind extends string>(kind: TKind): TypedAction<TValue, { kind: TKind; value: TValue }>
```

## Changes

### 1. Convert handler return types to `{ kind, value }`

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
export type ClassifyResult =
  | { kind: "HasErrors"; value: TypeError[] }
  | { kind: "Clean"; value: void };

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

return { kind: "HasErrors", errors };
return { kind: "Clean" };
```

After:
```ts
export type ClassifyResult =
  | { kind: "HasErrors"; value: TypeError[] }
  | { kind: "Clean"; value: void };

return { kind: "HasErrors", value: errors };
return { kind: "Clean", value: undefined };
```

#### `demos/identify-and-address-refactors/handlers/refactor.ts`

Before:
```ts
export type ClassifyJudgmentResult =
  | { kind: "Approved" }
  | { kind: "NeedsWork"; instructions: string };

return { kind: "Approved" };
return { kind: "NeedsWork", instructions: judgment.instructions };
```

After:
```ts
export type ClassifyJudgmentResult =
  | { kind: "Approved"; value: void }
  | { kind: "NeedsWork"; value: string };

return { kind: "Approved", value: undefined };
return { kind: "NeedsWork", value: judgment.instructions };
```

### 2. Branch auto-unwraps `value`

**File:** `libs/barnum/src/ast.ts` — standalone `branch()` and postfix `.branch()` runtime implementations

The TypeScript-side branch inserts `ExtractField("value")` before each case handler in the AST. No Rust executor changes needed initially. Long-term, auto-unwrapping should move into the Rust executor as a fundamental language feature — branch should work like Rust `match`, unwrapping the payload before entering the arm.

Before (runtime implementation):
```ts
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

### 3. Update postfix `.branch()` type signature

After auto-unwrap, case handlers receive the unwrapped payload, not the full variant. The postfix `.branch()` signature (from CONTRAVARIANT_CASE_HANDLERS) needs to reflect this:

Before (after CONTRAVARIANT_CASE_HANDLERS):
```ts
branch<TCases extends { [K in KindOf<Out>]: CaseHandler<Extract<Out, { kind: K }>> }>(
  cases: TCases,
): TypedAction<In, ExtractOutput<TCases[keyof TCases & string]>, Refs | ExtractRefs<TCases[keyof TCases & string]>>;
```

After (handler receives unwrapped payload):
```ts
/** Extract the value type from a { kind, value } variant. */
type UnwrapVariant<T> = T extends { value: infer V } ? V : T;

branch<TCases extends { [K in KindOf<Out>]: CaseHandler<UnwrapVariant<Extract<Out, { kind: K }>>> }>(
  cases: TCases,
): TypedAction<In, ExtractOutput<TCases[keyof TCases & string]>, Refs | ExtractRefs<TCases[keyof TCases & string]>>;
```

### 4. Update `BranchInput` for auto-unwrap

The standalone `branch()` derives its input type from handler inputs. After auto-unwrap, handlers receive payloads, so `BranchInput` wraps them back into `{ kind, value }`:

Before:
```ts
type BranchInput<TCases> = {
  [K in keyof TCases & string]: { kind: K } & ExtractInput<TCases[K]>;
}[keyof TCases & string];
```

After:
```ts
type BranchInput<TCases> = {
  [K in keyof TCases & string]: { kind: K; value: ExtractInput<TCases[K]> };
}[keyof TCases & string];
```

Note: `drop()` cases produce `{ kind: K; value: unknown }`. Under invariance, this doesn't match `{ kind: K; value: void }`. Standalone `branch()` with `drop()` cases still needs explicit type params. This is a known limitation — use postfix `.branch()` to avoid it.

### 5. Remove `extractField` from branch case handlers

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
  HasErrors: pipe(forEach(fix), recur<any>()),
  Clean: done<void>(),
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

## Implementation strategy: test-first

### Commit 1: Add failing tests

Add to `libs/barnum/tests/types.test.ts`:

```ts
describe("{ kind, value } convention", () => {
  it("ClassifyResult uses { kind, value } form", () => {
    // @ts-expect-error — remove after converting: HasErrors uses `errors` field, not `value`
    assertExact<IsExact<
      Extract<ClassifyResult, { kind: "HasErrors" }>,
      { kind: "HasErrors"; value: TypeError[] }
    >>();
  });

  it("branch auto-unwraps: HasErrors handler receives TypeError[] directly", () => {
    // After auto-unwrap, forEach(fix) receives TypeError[] directly — no extractField needed.
    // @ts-expect-error — remove after implementing: forEach(fix) input doesn't match HasErrors variant
    classifyErrors.branch({
      HasErrors: forEach(fix),
      Clean: drop(),
    });
  });
});
```

Test 1: The assertion fails because `HasErrors` currently has `errors: TypeError[]`, not `value: TypeError[]`. The `@ts-expect-error` suppresses it.

Test 2: After CONTRAVARIANT_CASE_HANDLERS, `.branch()` validates handler types. `forEach(fix)` expects `TypeError[]` but gets the full `HasErrors` variant. The `@ts-expect-error` suppresses the error.

### Commit 2+: Implement

1. Convert handler return types to `{ kind, value }`
2. Update branch to auto-unwrap `value`
3. Update postfix `.branch()` type signature for unwrapped payloads
4. Update `BranchInput` for auto-unwrap
5. Remove `extractField` from branch cases
6. **Remove `@ts-expect-error`** from the tests added in commit 1 — they now compile, proving the fix works.

## Files to change

| File | What changes |
|------|-------------|
| `libs/barnum/src/ast.ts` | Add `UnwrapVariant`; update `.branch()` signature; update `BranchInput`; branch auto-unwraps `value` in runtime impls |
| `libs/barnum/src/builtins.ts` | No changes — `tag()`, `recur()`, `done()` already produce `{ kind, value }` |
| `libs/barnum/tests/handlers.ts` | `ClassifyResult` uses `{ kind, value }` form; handler returns `{ kind, value }` |
| `libs/barnum/tests/types.test.ts` | Add failing tests (commit 1); remove `extractField` from branch cases; update type assertions; remove `@ts-expect-error` (commit 2) |
| `libs/barnum/tests/patterns.test.ts` | Update branch test cases for auto-unwrapping |
| `libs/barnum/tests/steps.test.ts` | Remove `extractField` from branch cases |
| `libs/barnum/tests/round-trip.test.ts` | Update Branch test constant to use `{ kind, value }` shape |
| `demos/convert-folder-to-ts/handlers/type-check-fix.ts` | Handler returns `{ kind, value }` |
| `demos/convert-folder-to-ts/run.ts` | Remove `extractField` from branch cases |
| `demos/identify-and-address-refactors/handlers/refactor.ts` | Handler returns `{ kind, value }` |
| `demos/identify-and-address-refactors/run.ts` | Remove `extractField` from branch cases |

## Open questions

1. **Rust executor** — Auto-unwrap starts as `ExtractField("value")` inserted in the TS AST. Long-term, this should be a fundamental part of the Rust executor's branch semantics (like Rust `match`). Separate follow-up.

2. **`void` vs `undefined` vs `null` for empty variants** — `{ kind: "Clean"; value: void }` — at runtime, `value` is `undefined`. The Rust executor sees `"value": null` in JSON. Need to verify serde handles this. Hopefully nothing breaks.
