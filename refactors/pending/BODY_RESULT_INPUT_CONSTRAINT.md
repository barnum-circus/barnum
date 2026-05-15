# BodyResult Should Constrain Input Type

## Motivation

`withResource` used as the bare return of a `bindInput` body passes type-checking but panics at runtime. The body receives `void` (from internal `drop`), but `withResource` needs `Record<string, unknown>` as input for its merge step. TypeScript doesn't catch this because `BodyResult<TOut>` only checks the output type.

## Current state

`BodyResult` in `libs/barnum/src/bind.ts`:

```typescript
type BodyResult<TOut> = Action & {
  __out?: () => TOut;
};
```

`bindInput` implementation:

```typescript
export function bindInput<TIn, TOut = any>(
  body: (input: VarRef<TIn>) => BodyResult<TOut>,
): TypedAction<TIn, TOut> {
  return bind([identity()], ([input]) => pipe(drop, body(input)));
}
```

The body expression is preceded by `drop` (output: `void`). `pipe(drop, body(input))` should verify that `body(input)`'s input type accepts `void` — but `BodyResult` has no `__in` field, so `pipe` can't check it.

## Why BodyResult omits `__in`

Body actions typically start from VarRefs, which have input type `any`. If `BodyResult` required `__in?: (input: void) => void`, then `input.then(handler)` (which has input `any`, not `void`) would fail. The omission was intentional to allow VarRef-rooted expressions.

## The gap

Actions that DON'T start from a VarRef — like `withResource(...)` placed directly as the body return — have a real input requirement that goes unchecked.

## Proposed fix

Add an input constraint to `BodyResult` that accepts `any` OR `void` but rejects concrete non-void types:

```typescript
type BodyResult<TOut> = Action & {
  __in?: ((input: void) => void) | undefined;  // must accept void
  __out?: () => TOut;
};
```

This would reject `withResource(...)` (whose `__in` is `(input: Record<string, unknown>) => void`) while still accepting VarRef chains (whose `__in` is `(input: any) => void`, which is assignable to `(input: void) => void`).

## Risk

This might break other valid patterns where the body expression has a narrow input type but is always preceded by a VarRef in practice. Needs audit of existing `bindInput` usage across demos.
