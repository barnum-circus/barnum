# Contravariant Case Handlers

**Blocks:** nothing
**Blocked by:** nothing

## Motivation

The postfix `.branch()` currently accepts any `Record<string, Action>` — no exhaustiveness check, no per-case type validation. Adding those checks requires a way to validate case handlers against expected variant types. Under full invariance, `drop()` (input `unknown`) doesn't match `Pipeable<HasErrors, ...>`. But case handlers are consumers — they receive a value and process it. A handler that accepts `unknown` can handle any variant. This is contravariant, not invariant.

## Current state

### Postfix `.branch()` signature (`libs/barnum/src/ast.ts:131`)

```ts
branch<TCases extends Record<string, Action>>(
  cases: TCases,
): TypedAction<In, ExtractOutput<TCases[keyof TCases & string]>, Refs | ExtractRefs<TCases[keyof TCases & string]>>;
```

No validation of `Out` against the cases. No exhaustiveness. Any object with string keys passes.

### Postfix `.branch()` usage (tests, demos)

```ts
// tests/types.test.ts:358 — works today because no input validation
classifyErrors.branch({
  HasErrors: pipe(
    extractField<Extract<ClassifyResult, { kind: "HasErrors" }>, "errors">("errors"),
    forEach(fix),
  ),
  Clean: drop(),  // no type param — works because .branch() doesn't check
})

// demos/convert-folder-to-ts/run.ts:59
pipe(typeCheck, classifyErrors).branch({
  HasErrors: pipe(
    extractField<Extract<ClassifyResult, { kind: "HasErrors" }>, "errors">("errors"),
    forEach(fix).drop(),
    recur<any>(),
  ),
  Clean: done<any>(),
})
```

### Standalone `branch()` usage (requires typed drops)

```ts
// tests/types.test.ts:473
branch({ HasErrors: drop<HasErrors>(), Clean: drop<Clean>() })

// tests/steps.test.ts:87
branch({
  HasErrors: pipe(
    extractField<HasErrors, "errors">("errors"),
    forEach(fix),
    recur<any>(),
  ),
  Clean: done<Clean>(),
})
```

## Proposed changes

### 1. Add `CaseHandler` type

**File:** `libs/barnum/src/ast.ts`, after `Pipeable` definition (~line 185)

```ts
/**
 * Contravariant-only input checking for branch case handler positions.
 *
 * Omits __in (covariant input) and __phantom_out_check (contravariant output)
 * compared to TypedAction/Pipeable. This gives:
 *   In:  contravariant only (via __phantom_in)
 *   Out: covariant only (via __phantom_out)
 *
 * Why contravariant input: a handler that accepts `unknown` (like drop())
 * can handle any variant. (input: unknown) => void is assignable to
 * (input: HasErrors) => void because HasErrors extends unknown.
 *
 * Why covariant output: the constraint doesn't restrict output types —
 * they're inferred from the actual case handlers via ExtractOutput.
 * TypedAction's invariant __phantom_out_check with TOut=unknown would
 * reject any handler with a specific output type, so we omit it.
 */
type CaseHandler<TIn = unknown, TOut = unknown, TRefs extends string = never> = Action & {
  __phantom_in?: (input: TIn) => void;
  __phantom_out?: () => TOut;
  __refs?: { _brand: TRefs };
};
```

TypedAction is assignable to CaseHandler because CaseHandler only requires a subset of TypedAction's phantom fields. Extra fields (`__in`, `__phantom_out_check`) don't prevent assignability.

### 2. Add `KindOf` helper type

**File:** `libs/barnum/src/ast.ts`, near type extraction utilities (~line 303)

```ts
/** Extract all `kind` string literals from a discriminated union. */
type KindOf<T> = T extends { kind: infer K extends string } ? K : never;
```

### 3. Update postfix `.branch()` signature

**File:** `libs/barnum/src/ast.ts`, TypedAction type (~line 131)

Before:
```ts
branch<TCases extends Record<string, Action>>(
  cases: TCases,
): TypedAction<In, ExtractOutput<TCases[keyof TCases & string]>, Refs | ExtractRefs<TCases[keyof TCases & string]>>;
```

After:
```ts
/** Dispatch on a tagged union output. Requires exhaustive case coverage. */
branch<TCases extends { [K in KindOf<Out>]: CaseHandler<Extract<Out, { kind: K }>> }>(
  cases: [KindOf<Out>] extends [never] ? never : TCases,
): TypedAction<In, ExtractOutput<TCases[keyof TCases & string]>, Refs | ExtractRefs<TCases[keyof TCases & string]>>;
```

The `[KindOf<Out>] extends [never] ? never : TCases` conditional makes `.branch()` unavailable when `Out` has no `kind` field — passing any argument to a `never` parameter is a compile error.

This enforces:
- **Exhaustiveness** — `[K in KindOf<Out>]` requires a key for every `kind` in `Out`
- **Per-case type matching** — `CaseHandler<Extract<Out, { kind: K }>>` validates each handler contravariantly
- **`drop()` works** — `(input: unknown) => void` satisfies `(input: HasErrors) => void` via contravariance

### 4. Standalone `branch()` stays unchanged

The standalone `branch()` still uses `BranchInput<TCases>` derived from handler inputs. Contravariant checking doesn't help here because there's no external type source. This is fixed by the tagged union convention (separate doc).

### What changes for users

**Postfix `.branch()` gains type safety it didn't have before:**

```ts
// BEFORE: compiles but wrong — missing "Clean" case, no error
classifyErrors.branch({
  HasErrors: drop(),
})

// AFTER: compile error — missing "Clean" case
classifyErrors.branch({
  HasErrors: drop(),
})

// AFTER: compiles — exhaustive
classifyErrors.branch({
  HasErrors: drop(),
  Clean: drop(),
})
```

**`drop()` keeps working without type params in postfix `.branch()`:**

```ts
// Works today (no input validation), keeps working (contravariant)
classifyErrors.branch({
  HasErrors: pipe(extractField<HasErrors, "errors">("errors"), forEach(fix)),
  Clean: drop(),
})
```

**`done()` still needs its type param, but for the output, not the input:**

```ts
// done<Clean>() is needed so the loop output type is Clean, not unknown.
// The type param is semantically meaningful — it declares what the break
// value is, not what the handler accepts.
pipe(typeCheck, classifyErrors).branch({
  HasErrors: pipe(..., recur<any>()),
  Clean: done<Clean>(),
})
```

### What about `Out` without `kind`?

If `Out` doesn't have a `kind` field, `KindOf<Out>` = `never`, and the constraint becomes `{ [K in never]: ... }` = `{}`. Any cases object satisfies `{}`, which means no validation.

**Implemented:** The cases parameter uses a conditional: `[KindOf<Out>] extends [never] ? never : TCases`. When `Out` has no `kind`, the parameter type is `never`, so passing any object is a compile error. This catches misuse at the call site.

## Implementation strategy: test-first

### Commit 1: Add failing tests

Add to `libs/barnum/tests/types.test.ts`:

```ts
describe("postfix .branch() type safety", () => {
  it("rejects non-exhaustive postfix branch", () => {
    // @ts-expect-error — non-exhaustive: missing "Clean" case
    classifyErrors.branch({ HasErrors: drop() });
  });

  it("rejects wrong handler type in postfix branch", () => {
    classifyErrors.branch({
      // @ts-expect-error — deploy expects { verified: boolean }, not HasErrors
      HasErrors: deploy,
      Clean: drop(),
    });
  });

  it("accepts exhaustive postfix branch with bare drop()", () => {
    classifyErrors.branch({
      HasErrors: drop(),
      Clean: drop(),
    });
  });

  it("rejects .branch() on non-discriminated output", () => {
    // deploy output is { deployed: boolean } — no `kind` field
    // @ts-expect-error — Out has no kind, .branch() unavailable
    deploy.branch({ A: drop() });
  });
});
```

Tests 1, 2, 4: `@ts-expect-error` is currently **unused** (the lines compile because `.branch()` doesn't validate). Unused `@ts-expect-error` is a TS error → tests are "broken".

Test 3: Compiles today, will still compile after fix (contravariant handlers accept `drop()` with `unknown` input).

### Commit 2: Implement

1. Add `CaseHandler`, `KindOf` types to `ast.ts`
2. Update postfix `.branch()` signature
3. The `@ts-expect-error` directives in tests 1, 2, 4 become valid (the lines now error as expected) → tests pass

The unused `@ts-expect-error` directives becoming valid **proves the fix works**.

### Existing test updates

- `libs/barnum/tests/patterns.test.ts:198` — `deploy.branch({ A: drop(), B: drop() })` breaks because `deploy` output has no `kind`. Change to use `classifyErrors.branch({ HasErrors: drop(), Clean: drop() })`.
- All other existing postfix `.branch()` calls still compile — handlers satisfy `CaseHandler` via contravariance, and existing calls are already exhaustive.

## Files to change

| File | Change |
|------|--------|
| `libs/barnum/src/ast.ts` | Add `CaseHandler`, `KindOf`; update postfix `.branch()` signature |
| `libs/barnum/tests/types.test.ts` | Add failing tests (commit 1); verify they pass after implementation (commit 2) |
| `libs/barnum/tests/patterns.test.ts` | Update `deploy.branch()` test to use discriminated output; verify others still pass |
| `libs/barnum/tests/steps.test.ts` | Verify showcase/kitchen-sink tests still pass |
| Demos | No changes expected — they already use postfix `.branch()` with exhaustive cases |

## Open questions

1. **Inference stability** — ✅ Resolved. The mapped type constraint works: TypeScript correctly infers `TCases` from the argument and checks it against the constraint. `ExtractOutput` resolves from the inferred argument types, not the constraint. No fallback needed.

2. **Non-exhaustive branches** — Exhaustive matching only. No escape hatch for partial matching. To handle a subset of variants, use a `pick`-style combinator to narrow first, then exhaustively match including a `none`/`otherwise` case for unmatched variants. This is a separate feature to design later.
