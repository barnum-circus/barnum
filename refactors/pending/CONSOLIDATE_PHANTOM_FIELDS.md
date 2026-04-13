# Consolidate Phantom Fields

## Motivation

`TypedAction`, `Pipeable`, and `CaseHandler` carry four phantom fields with inconsistent naming to encode variance:

```ts
// TypedAction and Pipeable (invariant In, invariant Out):
__in?: (input: In) => void;       // contravariant In
__in_co?: In;                      // covariant In
__out?: () => Out;                 // covariant Out
__out_contra?: (output: Out) => void;  // contravariant Out

// CaseHandler (contravariant In, covariant Out):
__in?: (input: TIn) => void;      // contravariant In
__out?: () => TOut;                // covariant Out
```

`CaseHandler` exists solely because invariant output rejects `never` — `throwError` (`TypedAction<TError, never>`) can't satisfy `Pipeable<TError, TValue>` because `__out_contra` requires `TValue extends never`. `CaseHandler` drops the contravariant output field to make output covariant, letting `never extends TValue` pass.

Two problems:

1. **Output invariance is unnecessary.** Input invariance prevents handlers from receiving unexpected fields (the real concern). Output invariance prevents a step producing `Dog` from connecting to a step expecting `Animal` — but that's safe. The downstream step's input invariance catches any real mismatch. Output invariance adds no safety, only friction.

2. **`CaseHandler` is a variance escape hatch, not a meaningful abstraction.** It exists to work around a problem that shouldn't exist. With covariant output, `Pipeable` handles every position `CaseHandler` currently fills.

## Design

### Variance model

- **Input: invariant.** Handlers cross serialization boundaries (Rust, Python). Extra or missing fields are runtime errors. A handler expecting `{ foo }` must not receive `{ foo, bar }`. The contravariant `__in` (rejects too-narrow callers) and covariant `__in_co` (rejects too-wide callers) together enforce exact matching.

- **Output: covariant.** A step producing `Dog` where `Animal` is expected downstream is safe — the consumer gets a superset. `never` (genuinely unreachable: `throwError`, `recur`, `done`) is assignable to any output type via standard subtyping. No escape hatch needed.

### Consolidated phantom field

A single function type encodes both:

```ts
__phantom?: (input: In) => [In, Out];
```

- `In` in parameter position (contravariant) + in return tuple (covariant) → **invariant**
- `Out` only in return tuple (covariant) → **covariant**

### Delete `CaseHandler`

With covariant output on `Pipeable`, `TypedAction<TError, never>` is assignable to `Pipeable<TError, TValue>` because `never extends TValue` is always true covariantly. Every place that uses `CaseHandler` switches to `Pipeable`:

- `branch` case constraint: `CaseHandler<BranchPayload<Out, K>, unknown>` → `Pipeable<BranchPayload<Out, K>, unknown>`
- `unwrapOr` default action: `CaseHandler<TError, TValue>` → `Pipeable<TError, TValue>`
- `Result.unwrapOr` default action: inline `Action & { __in?; __out? }` → `Pipeable<TError, TValue>`

### `drop` in branch cases

`drop` is `TypedAction<any, never>`. With `Pipeable<BranchPayload, unknown>`:

- Input: `any` vs `BranchPayload`. Invariant. `__in`: contravariance `BranchPayload extends any` ✓. `__in_co`: covariance `any extends BranchPayload` ✓ (any is special).
- Output: `never` vs `unknown`. Covariant: `never extends unknown` ✓.

Works without `CaseHandler`.

### `throwError` in `unwrapOr`

`throwError` is `TypedAction<TError, never>`. With `Pipeable<TError, TValue>`:

- Input: `TError` matches `TError` invariantly ✓.
- Output: `never extends TValue` covariantly ✓.

Works without `CaseHandler`.

## Current state

### `TypedAction` (`libs/barnum/src/ast.ts`)

```ts
export type TypedAction<In = unknown, Out = unknown> = Action & {
  __in?: (input: In) => void;
  __in_co?: In;
  __out?: () => Out;
  __out_contra?: (output: Out) => void;
  // ...methods...
};
```

### `Pipeable` (`libs/barnum/src/ast.ts`)

```ts
export type Pipeable<In = unknown, Out = unknown> = Action & {
  __in?: (input: In) => void;
  __in_co?: In;
  __out?: () => Out;
  __out_contra?: (output: Out) => void;
};
```

### `CaseHandler` (`libs/barnum/src/ast.ts`)

```ts
type CaseHandler<TIn = unknown, TOut = unknown> = Action & {
  __in?: (input: TIn) => void;
  __out?: () => TOut;
};
```

### `ExtractInput` / `ExtractOutput` (`libs/barnum/src/ast.ts`)

```ts
export type ExtractInput<T> = T extends {
  __in?: (input: infer In) => void;
}
  ? In
  : never;

export type ExtractOutput<T> = T extends { __out?: () => infer Out }
  ? Out
  : never;
```

## Changes

### 1. Remove `Refs` type parameter and `__refs` field — DONE

### 2. Replace phantom fields on TypedAction

```ts
// Before (4 fields, invariant In + invariant Out)
__in?: (input: In) => void;
__in_co?: In;
__out?: () => Out;
__out_contra?: (output: Out) => void;

// After (1 field, invariant In + covariant Out)
__phantom?: (input: In) => [In, Out];
```

### 3. Replace phantom fields on Pipeable

Same change as TypedAction:

```ts
// After
__phantom?: (input: In) => [In, Out];
```

### 4. Delete `CaseHandler`

Remove the type entirely. Replace all usages with `Pipeable`:

| Location | Before | After |
|----------|--------|-------|
| `branch` constraint | `CaseHandler<BranchPayload<Out, K>, unknown>` | `Pipeable<BranchPayload<Out, K>, unknown>` |
| Postfix `unwrapOr` | `defaultAction: CaseHandler<TError, TValue>` | `defaultAction: Pipeable<TError, TValue>` |
| `Result.unwrapOr` | inline `Action & { __in?; __out? }` | `Pipeable<TError, TValue>` |

Remove the JSDoc explaining `CaseHandler`'s variance rationale.

### 5. Update ExtractInput / ExtractOutput

```ts
export type ExtractInput<T> = T extends {
  __phantom?: (input: infer In) => any;
}
  ? In
  : never;

export type ExtractOutput<T> = T extends {
  __phantom?: (input: any) => [any, infer Out];
}
  ? Out
  : never;
```

### 6. Update JSDoc comments

Replace the variance explanation on TypedAction and Pipeable:

"A single phantom field `__phantom?: (input: In) => [In, Out]` encodes variance. `In` appears in both parameter (contravariant) and return (covariant) position, making it invariant — handlers never receive unexpected fields. `Out` appears only in return position, making it covariant — `never` (unreachable: `throwError`, `recur`, `done`) is assignable to any output slot."

### 7. Update docs

Update `docs-website/docs/architecture/typescript-ast.md` if it references phantom field names or `CaseHandler`.

## Verification

All existing type tests in `types.test.ts` exercise the variance constraints:
- Pipe rejects mismatched types (invariant input)
- Branch accepts `drop` as a case handler (covariant output, `any` input)
- `unwrapOr(throwError)` works (covariant output, `never extends TValue`)
- `ExtractInput` / `ExtractOutput` assertions throughout

If the consolidated fields pass the existing type tests, the variance behavior is preserved.

## Execution order

1. ~~Remove `Refs` from all types and combinators~~ — DONE
2. Replace phantom fields on TypedAction, Pipeable (4 fields → 1)
3. Delete `CaseHandler`, replace usages with `Pipeable`
4. Update ExtractInput / ExtractOutput
5. Update JSDoc comments
6. Typecheck + run tests
7. Update docs
