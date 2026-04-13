# Consolidate Phantom Fields → Output Covariance

**Status: LANDED**

## Motivation

`TypedAction` and `Pipeable` had four phantom fields encoding invariant input + invariant output:

```ts
__in?: (input: In) => void;           // contravariant In
__in_co?: In;                         // covariant In
__out?: () => Out;                    // covariant Out
__out_contra?: (output: Out) => void; // contravariant Out
```

Output invariance was unnecessary and harmful:
- A step producing `Dog` where `Animal` is expected downstream is safe (covariance).
- `never` (throwError, recur, done) couldn't satisfy `Pipeable<X, TValue>` because `__out_contra` required `TValue extends never` — always false.
- `CaseHandler` existed solely as a variance escape hatch for branch cases and unwrapOr, dropping `__out_contra` to get covariant output.

## What we tried

### Single `__phantom` field (FAILED)

Attempted consolidating all four fields into one:

```ts
__phantom?: (input: In) => [In, Out];
```

This fails because TypeScript handles `any` differently in property positions vs function return positions. `__in_co?: any` makes `any extends BranchPayload` pass (any is special in property assignability). But `(input: In) => [In, Out]` with `In = any` puts the covariant In check in a function return tuple, where `any` doesn't get the same special treatment. Result: `unknown` input handlers fail to satisfy specific type constraints.

### Remove `__out_contra` only (SUCCEEDED)

Kept the three-field layout, just removed the contravariant output field:

```ts
__in?: (input: In) => void;  // contravariant In
__in_co?: In;                // covariant In  
__out?: () => Out;           // covariant Out (was invariant)
```

## What changed

### TypedAction and Pipeable (`libs/barnum/src/ast.ts`)

Removed `__out_contra` from both types. Output is now covariant.

### CaseHandler — kept, purpose shifted

CaseHandler was **not** deleted. Its purpose shifted from output relaxation to **input relaxation**:

- Before: needed because invariant output rejected `never`. Dropped `__out_contra`.
- After: needed because `Pipeable` has invariant input (`__in_co`), which is too strict for branch cases where handlers with `unknown`/`any` input (like `drop`) must satisfy specific variant constraints. CaseHandler drops `__in_co` to make input contravariant-only.

```ts
// CaseHandler: contravariant input + covariant output
type CaseHandler<TIn = unknown, TOut = unknown> = Action & {
  __in?: (input: TIn) => void;
  __out?: () => TOut;
};
```

Used in `.branch()` constraint only. `unwrapOr` uses `Pipeable` since it doesn't need input relaxation.

### unwrapOr (`libs/barnum/src/ast.ts`, `libs/barnum/src/builtins.ts`)

Changed from `CaseHandler<TError, TValue>` to `Pipeable<TError, TValue>`. With covariant output, `TypedAction<TError, never>` (throwError) is assignable to `Pipeable<TError, TValue>` because `never extends TValue`.

### BodyResult (`libs/barnum/src/bind.ts`, `libs/barnum/src/recursive.ts`)

Removed `__out_contra` from the `BodyResult` type used by `bind` and `defineRecursiveFunctions`.

## What output covariance enables

- `throwError` (output `never`) works in `Pipeable<TError, TValue>` slots — `never extends TValue` passes covariantly
- `done`/`recur` (output `never`) work in pipe positions expecting specific output types
- No `as any` casts needed in demo code for these patterns

## What it does NOT fix

- `as any` casts in loop patterns where `drop` outputs `void` and loop body requires `never` — `void extends never` is false regardless of variance direction
- Internal library `as any` casts at the AST construction layer — these are fundamental to untyped→typed boundaries

## Execution order

1. ~~Remove `Refs` type parameter~~ — done in prior commit
2. ~~Single `__phantom` field~~ — failed, abandoned
3. Remove `__out_contra` from TypedAction, Pipeable, BodyResult — **DONE**
4. Keep CaseHandler for branch input relaxation — **DONE**
5. Switch unwrapOr to Pipeable — **DONE**
6. Typecheck + tests pass — **VERIFIED**
