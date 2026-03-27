# Deferred Features

Features removed from the initial implementation to keep the surface area minimal. To be added incrementally as needed.

## Builtin Handler Kind

Rust-native data transformations executed without FFI. Conceptually a variant of `HandlerKind` (not a separate `Action` variant — it's a type of `Call`).

Operations:
- **Tag**: Wraps input as `{ kind, value: input }`. Enables `recur()` (Tag "Continue") and `done()` (Tag "Break") for loop signals.
- **Identity**: Passes input through unchanged.
- **Merge**: Merges an array of objects into a single object.
- **Flatten**: Flattens a nested array one level.
- **ExtractField**: Extracts a single field from an object.

Without Builtin, loop signals and structural transforms must be implemented in handler code (TypeScript).

## Handler Validator Ergonomics

Reconsider the `createHandler` validator design:

- **Optional `stepValueValidator`**: When omitted, the value type could default to `never` or `{}`, signaling "this handler doesn't consume pipeline input." Currently required.
- **`stepConfigValidator` as a type parameter instead of a runtime validator**: Instead of `stepConfigValidator?: z.ZodType<TStepConfig>`, allow passing `TStepConfig` as a generic type parameter directly (e.g., `createHandler<{ timeout: number }>({...})`). The runtime validator is needed for serialization, but the type parameter approach is more ergonomic for handlers where config shape is known statically.
- General question: should validators be the only way to specify types, or should explicit type parameters remain an option?

## matchCases Discriminated Union Narrowing

`matchCases` currently uses `any` for per-case input types because runtime dispatch narrows the input per variant, but TypeScript can't express this statically with the current signature. This has two consequences:

1. **No per-case type narrowing.** Each match case receives `any` as input instead of `Extract<TUnion, { kind: K }>`. Handlers inside cases like `extractField("errors")` work syntactically (any field name is valid on `any`) but lose output type tracking.

2. **`recur()` and `done()` can't be properly generic.** Semantically, `recur<T>()` should be `TypedAction<T, LoopResult<T, never>>` and `done<T>()` should be `TypedAction<T, LoopResult<never, T>>`. But inside matchCases, `T` infers as `any` (from the `any` input), so both collapse to `LoopResult<any, any>` and the complementary `never` types can't unify as matchCases' `Out`. Currently both return `LoopResult<any, any>` as a workaround.

The proper fix requires matchCases to accept a mapped type over the discriminated union:

```ts
function matchCases<TUnion extends { kind: string }, TOut>(
  cases: { [K in TUnion['kind']]: TypedAction<Extract<TUnion, { kind: K }>, TOut> },
): TypedAction<TUnion, TOut>
```

The challenge: `TUnion` must be inferred from the sequence context (the preceding action's output), not from the cases object. This likely requires either (a) a two-step builder like `match<ClassifyResult>().cases({...})`, or (b) TypeScript inference improvements in future versions.

## Exhaustive matchCases

WORKFLOW_ALGEBRA.md specifies exhaustive match handling: `{ [K in U['kind']]: Action }` ensures every variant has a case. Current implementation uses `Record<string, TypedAction<any, Out>>`, which allows missing or extra cases without compile-time errors.

This is blocked on matchCases narrowing above — exhaustiveness requires knowing the input union type.

## AttemptResult Shape

WORKFLOW_ALGEBRA.md specifies `{ kind: "Success", value } | { kind: "Failure", error, input }`. Current TypeScript uses `{ kind: "Ok", value } | { kind: "Err", error }`. Two differences:
- Naming: Success/Failure vs Ok/Err
- Failure variant carries the original `input`, enabling retry/fallback patterns without re-computing the input

## Context

Read-only environment (`context: Value`) on `Config`, passed to all handlers. Carries API keys, workflow IDs, tenant config, etc.

Alternative: user-land Reader Monad pattern using `All` + `Identity` + `Merge` (see WORKFLOW_ALGEBRA.md). This incurs O(N) cloning cost for parallel branches, which the host-level context avoids.
