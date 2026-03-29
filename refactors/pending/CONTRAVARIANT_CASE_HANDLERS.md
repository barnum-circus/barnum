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
 * Omits __in (the covariant input field) so only __phantom_in is checked.
 * This means the handler just needs to ACCEPT the variant type, not declare
 * it exactly. A handler with input `unknown` (like drop()) accepts any
 * variant via contravariance: (input: unknown) => void is assignable to
 * (input: HasErrors) => void because HasErrors extends unknown.
 *
 * Output remains invariant — the case handler's output type flows into
 * downstream combinators and must be exact.
 */
type CaseHandler<In = unknown, Out = unknown, Refs extends string = never> = Action & {
  __phantom_in?: (input: In) => void;
  // No __in — only contravariant input checking
  __phantom_out?: () => Out;
  __phantom_out_check?: (output: Out) => void;
  __refs?: { _brand: Refs };
};
```

TypedAction (which has `__in`) is assignable to CaseHandler (which doesn't require `__in`) — extra properties are fine.

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
  cases: TCases,
): TypedAction<In, ExtractOutput<TCases[keyof TCases & string]>, Refs | ExtractRefs<TCases[keyof TCases & string]>>;
```

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

Options:
- Accept this — if you call `.branch()` on a non-discriminated output, you're on your own
- Add a conditional that makes `.branch` unavailable when Out has no `kind` — e.g., return type is `never` when `KindOf<Out>` is `never`
- Use an overload: one for discriminated Out (validated), one fallback (current behavior)

**Recommendation:** Use a conditional. If `KindOf<Out>` is `never`, the cases parameter type should be `never` (compile error). This catches misuse at the call site.

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

1. **Inference stability** — The `.branch()` signature constrains `TCases` with a mapped type: `{ [K in KindOf<Out>]: CaseHandler<Extract<Out, { kind: K }>> }`. TypeScript must infer `TCases` from the argument while simultaneously checking it against the constraint. If TS struggles — e.g., resolves `TCases` to the constraint type instead of the argument type, breaking `ExtractOutput` — the fallback is: keep `TCases extends Record<string, Action>` (easy to infer) and use a conditional return type that produces `never` when cases don't match the constraint. Prototype during implementation.

2. **Non-exhaustive branches** — Exhaustive matching only. No escape hatch for partial matching. To handle a subset of variants, use a `pick`-style combinator to narrow first, then exhaustively match including a `none`/`otherwise` case for unmatched variants. This is a separate feature to design later.
