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

## Attempt as Dynamic-Scope Context

`Attempt` is implicitly a dynamic-scope mechanism. When `error()` propagates up the frame tree, the engine walks ancestor frames looking for an `Attempt` — that's dynamic scope lookup. The frame tree *is* the dynamic scope.

This suggests a generalization: a `Provide` / `Consume` mechanism where `Provide` pushes a value onto the dynamic scope and `Consume` reads the nearest one. `Attempt` would be a special case — it "provides" an error boundary, and the error propagation logic "consumes" it.

Use cases beyond error catching:
- **Retry policies**: `WithRetry { max: 3, backoff: "exponential" }` wraps a subtree. When an Invoke inside fails, the engine walks up to find the nearest retry policy and re-dispatches.
- **Timeouts**: `WithTimeout { ms: 5000 }` wraps a subtree. The runtime (not the engine — the engine is pure) uses this to set deadlines on dispatches.
- **Tracing/logging context**: `WithSpan { name: "checkout" }` wraps a subtree. Dispatches include the span context so handlers can emit structured logs.

The engine currently hard-codes `Attempt` as the only frame type that intercepts errors. A general context mechanism would let users define custom interception points. The tradeoff is complexity — dynamic scope is powerful but hard to reason about, and the engine's simplicity (pure state machine, no implicit plumbing) is a feature.

For now, `Attempt` is hard-coded. If more interception patterns emerge (retry, timeout), refactoring to a general dynamic-scope mechanism becomes worthwhile.

## Loop as Desugared Step + Branch

Loop can be desugared into existing primitives:

```
Loop(body)
≡
LoopBody = Chain(body, Branch({
  Continue: Step("LoopBody"),
  Break: identity()
}))
```

Loop = Step + Chain + Branch + self-reference. It's eliminable but worth keeping as a primitive:

1. **Frame reuse.** The engine can re-enter a Loop frame without teardown/creation per iteration. Desugared, each iteration creates and destroys a Chain frame + Branch reduction + Step redirect. For hot loops, this is 3x the frame churn.
2. **No synthetic steps.** Desugaring requires manufacturing anonymous step entries in the flat table.
3. **Debuggability.** A Loop frame is immediately recognizable in the frame tree.

Loop follows the single-child frame pattern (body completes → inspect → re-enter or propagate). Unlike the old Pipe, it doesn't cause a fundamentally different frame pattern — it's just an optimization over the desugared form.

### Step is goto

In the flat representation, `Step { target: ActionId }` is literally a `goto` — it redirects to another ActionId with no frame creation. Named steps are just labels in the flat table. This means the desugared Loop is just: run body, branch on result, goto self on Continue. No special recursion primitive needed — `goto` + `Branch` gives you fixed-point iteration for free.

This also means named steps are not a "function call" abstraction — they're jump targets. There's no stack frame, no return address, no scope. The flat table is a control flow graph and Step is an edge.

## Chain Normalization

Chains should be right-nested. `Chain(Chain(A, B), C)` is non-canonical — it's semantically equivalent to `Chain(A, Chain(B, C))` but wastes a ChildRef (the left-nested Chain in `first` is multi-entry). The canonical form is a right-leaning spine where `first` is never a Chain:

```
// Non-canonical (left-nested):
Chain(Chain(A, B), C)

// Canonical (right-nested):
Chain(A, Chain(B, C))
```

Since `pipe()` already produces right-nested chains via `reduceRight`, non-canonical forms can only arise from manual AST construction or other combinators that compose chains. Two enforcement options:

1. **Validation pass**: after deserialization, walk the tree and reject (or normalize) any Chain whose `first` is a Chain. Simple, catches bugs.

2. **Type-level enforcement**: make `Chain.first` accept a type that excludes `Chain`. In TypeScript this is straightforward — define a `NonChainAction` type that's the union minus `ChainAction`, and use it for `first`. In Rust, this would require either a newtype wrapper or a separate enum without the Chain variant, which is heavier. A validation pass is probably more practical.

The flattener could also normalize during flattening: when it encounters `Chain(Chain(A, B), C)`, rewrite to `Chain(A, Chain(B, C))` before emitting entries. This keeps the flat table canonical regardless of input shape.

## Trivial Combinator Elimination

Compile-time simplifications during flattening (or a validation/normalization pass):

- **`Parallel([A])`**: NOT a trivial elimination. `Parallel([A])` produces `[A(x)]` while `A` produces `A(x)` — different output shapes (array-wrapped vs unwrapped). Eliminating the Parallel requires also wrapping the child's output in an array, which means a builtin. Not worth pursuing until builtins exist.

- **`Parallel([])`**: Produces `[]` (empty tuple). The TS `parallel()` already compiles this to `constant([])` at build time. The Rust flattener should also handle `Parallel { actions: [] }` by rewriting to a constant empty array, as a defensive measure. Important for constant folding and dead code elimination.

Other potential simplifications to investigate as the AST matures.

## Lazy Step Flattening

Currently, flattening eagerly processes all steps in `Config::steps`, even if some are never referenced by the workflow. This is wasted work and inflates the flat table with dead entries.

Lazy flattening: only flatten a step when the flattener first encounters a `Step` reference to it. Steps that are never referenced are never flattened. This is a natural fit for the two-pass model — pass 1 reserves ActionIds for steps when they're first referenced, pass 2 resolves them. The change is to skip pre-allocating entries for unreferenced steps entirely.

Benefits:
- Smaller flat tables when configs contain library-style step registries (many steps defined, few used per workflow).
- Faster flattening for large configs.
- Dead step detection for free — any step that wasn't flattened after the walk is unreferenced.

This could go further: flatten steps on-demand during execution, not just during the flattening pass. The engine flattens the workflow root eagerly (down to the first Invoke leaves), dispatches those handlers, and while waiting for results, lazily flattens any Step targets that haven't been flattened yet. Step bodies behind a Chain's `rest` or inside a Branch case that hasn't been taken yet don't need to exist in the flat table until the engine actually reaches them. This turns flattening into an incremental process interleaved with execution — only the reachable frontier is materialized at any given time.

The current eager approach is simpler and correct. Lazy/incremental flattening is an optimization for when config sizes grow.

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
