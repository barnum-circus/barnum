# Deferred Features

Features removed from the initial implementation to keep the surface area minimal. To be added incrementally as needed.

## Namespaced Builtins

Current builtins are flat exports from `builtins.ts`: `identity`, `constant`, `merge`, `flatten`, `extractField`, `drop`, `dropResult`, `range`, `recur`, `done`, `tag`. This doesn't scale — as we add result combinators, option types, and more structural transforms, the flat namespace becomes a grab bag.

Proposed namespacing via exported objects:

### `result` — AttemptResult combinators

For working with `AttemptResult<T>` (produced by `attempt()`):

```ts
import { result } from "barnum/builtins";

// Extract the Ok value, discarding Err (AttemptResult<T> → T | null)
result.ok()

// Extract the Err value, discarding Ok (AttemptResult<T> → unknown | null)
result.err()

// Unwrap Ok or throw (AttemptResult<T> → T)
result.unwrap()

// Map over the Ok value (AttemptResult<T> → AttemptResult<U>)
result.map(action)

// Provide a fallback for Err (AttemptResult<T> → T)
result.unwrapOr(fallbackAction)
```

### `loop` — LoopResult signals

Replace the current `recur()` and `done()` with namespaced equivalents:

```ts
import { loop as loopBuiltins } from "barnum/builtins";

// These replace the current top-level recur() and done()
loopBuiltins.continue()  // tag as { kind: "Continue", value: input }
loopBuiltins.break()     // tag as { kind: "Break", value: input }
```

Note: import alias needed since `loop` is also an AST combinator. Alternatively, re-export from the `loop` combinator itself: `loop.continue()`, `loop.break()` — though mixing combinator + namespace is unusual.

### `data` — Structural transforms

Group the pure data manipulation builtins:

```ts
import { data } from "barnum/builtins";

data.identity()           // pass-through
data.constant(value)      // produce fixed value (no pipeline input)
data.merge()              // merge array of objects into one
data.flatten()            // flatten nested array one level
data.field("name")        // extract a single field (rename of extractField)
data.drop()               // discard pipeline value
data.dropResult(action)   // run action for side effects, discard output
data.range(start, end)    // produce integer array
data.tag("MyKind")        // wrap as { kind: "MyKind", value: input }
```

### Migration path

Since backward compatibility doesn't matter, the flat exports can be replaced directly. No re-exports or deprecation.

### Open question: are these builtins or combinators?

Some of these (`result.map`, `result.unwrapOr`) take an action argument and compose it — that makes them combinators, not just builtins. The namespace grouping is still useful, but the implementation may live in `ast.ts` rather than `builtins.ts` since they produce composite AST nodes.

## Builtin Handler Kind

Rust-native data transformations executed without FFI. Conceptually a variant of `HandlerKind` (not a separate `Action` variant — it's a type of `Invoke`).

Operations:
- **Tag**: Wraps input as `{ kind, value: input }`. Enables `loop.continue()` (Tag "Continue") and `loop.break()` (Tag "Break") for loop signals.
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

## Branch Discriminated Union Narrowing

`branch` currently uses `any` for per-case input types because runtime dispatch narrows the input per variant, but TypeScript can't express this statically with the current signature. This has two consequences:

1. **No per-case type narrowing.** Each branch case receives `any` as input instead of `Extract<TUnion, { kind: K }>`. Handlers inside cases like `data.field("errors")` work syntactically (any field name is valid on `any`) but lose output type tracking.

2. **`loop.continue()` and `loop.break()` can't be properly generic.** Semantically, `loop.continue<T>()` should be `TypedAction<T, LoopResult<T, never>>` and `loop.break<T>()` should be `TypedAction<T, LoopResult<never, T>>`. But inside branch, `T` infers as `any` (from the `any` input), so both collapse to `LoopResult<any, any>` and the complementary `never` types can't unify as branch's `Out`. Currently both return `LoopResult<any, any>` as a workaround.

The proper fix requires branch to accept a mapped type over the discriminated union:

```ts
function branch<TUnion extends { kind: string }, TOut>(
  cases: { [K in TUnion['kind']]: TypedAction<Extract<TUnion, { kind: K }>, TOut> },
): TypedAction<TUnion, TOut>
```

The challenge: `TUnion` must be inferred from the pipe context (the preceding action's output), not from the cases object. This likely requires either (a) a two-step builder like `branch<ClassifyResult>().cases({...})`, or (b) TypeScript inference improvements in future versions.

## ~~Exhaustive Branch~~ (Implemented)

Implemented via K-inference in `branch`'s signature: `branch<K extends string, Out, R>(cases: Record<K, TypedAction<any, Out, R>>): TypedAction<{ kind: K }, Out, R>`. TypeScript infers `K` from the cases keys, and pipe's contravariant input checking enforces exhaustiveness automatically. Missing cases produce compile errors; extra cases are allowed.

## AttemptResult Shape

WORKFLOW_ALGEBRA.md specifies `{ kind: "Success", value } | { kind: "Failure", error, input }`. Current TypeScript uses `{ kind: "Ok", value } | { kind: "Err", error }`. Two differences:
- Naming: Success/Failure vs Ok/Err
- Failure variant carries the original `input`, enabling retry/fallback patterns without re-computing the input

## Context

Read-only environment (`context: Value`) on `Config`, passed to all handlers. Carries API keys, workflow IDs, tenant config, etc.

Alternative: user-land Reader Monad pattern using `parallel` + `identity` + `merge` (see WORKFLOW_ALGEBRA.md). This incurs O(N) cloning cost for parallel branches, which the host-level context avoids.

## Effect Registries / Side-Effect Context

Beyond read-only context, handlers need a way to perform side effects (logging, metrics, tracing) through a structured API rather than ad-hoc I/O. This could take the form of an effect registry passed to handlers alongside the input and context — a set of capabilities the handler is allowed to use.

This overlaps with the Context feature above but is distinct: context is read-only data, effects are write-only capabilities. Both are provided by the host and available to all handlers without flowing through the data pipeline.

## Handler Error Type

Handlers currently return `Promise<TOutput>` and errors are untyped (caught as `unknown` by `attempt`). A typed error channel would let handlers declare their failure modes:

```ts
createHandler({
  stepValueValidator: z.object({ ... }),
  errorType: z.object({ code: z.string(), message: z.string() }),
  handle: async ({ value }) => { ... },
})
```

The error type defaults to `unknown` in TypeScript. The `attempt` combinator would then produce `AttemptResult<TOutput, TError>` instead of `AttemptResult<TOutput>` with `error: unknown`.

## Streams

Support for streaming data through the pipeline — actions that produce or consume async iterables rather than single values. Relevant for large datasets, real-time feeds, or incremental processing where buffering the full result is impractical.

Open question: is this a new primitive (e.g., `StreamForEach`) or a modifier on existing primitives? Could also be a handler-level concern (handlers that yield multiple values) rather than an AST-level feature.

## ~~Constant and Range Builtins~~ (Implemented)

Implemented in `libs/barnum/src/builtins.ts`. `constant<T>(value)` and `range(start, end)` are available as TypeScript builtins using placeholder `__builtin__` Invoke nodes.

## ~~Handler as Callable~~ (Implemented)

Implemented in `libs/barnum/src/handler.ts`. `createHandler` returns a `CallableHandler` — a function that produces `TypedAction` when invoked, with Handler metadata (`__filePath`, `__definition`, brand symbol) attached via `Object.assign`. Direct invocation: `setup()` or `setup({ stepConfig: { timeout: 5000 } })`. `invoke()` still works for explicit invocation.
