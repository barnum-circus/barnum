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

## Files to change

| File | Change |
|------|--------|
| `libs/barnum/src/ast.ts` | Add `CaseHandler`, `KindOf`; update postfix `.branch()` signature |
| `libs/barnum/tests/types.test.ts` | Add exhaustiveness tests; update postfix `.branch()` type assertions |
| `libs/barnum/tests/patterns.test.ts` | Verify existing postfix tests still pass |
| `libs/barnum/tests/steps.test.ts` | Verify showcase/kitchen-sink tests still pass |
| Demos | No changes expected — they already use postfix `.branch()` with the patterns that work |

## Open questions

1. **Inference stability** — Does TS reliably infer `TCases` when the constraint is a mapped type over `KindOf<Out>`? Need to prototype. If inference breaks, fallback is a conditional return type instead of a constrained parameter.

2. **Non-exhaustive branches** — Should there be an escape hatch for intentionally non-exhaustive branches (e.g., only handling some variants and letting others fall through)? The Rust executor errors on unmatched kinds, so non-exhaustive is a runtime error anyway. Enforcing exhaustiveness at compile time seems correct.
