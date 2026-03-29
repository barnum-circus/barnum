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

- **Optional `inputValidator`**: When omitted, the value type could default to `never` or `{}`, signaling "this handler doesn't consume pipeline input." Currently required.
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

## ~~AttemptResult Shape~~ — Remove Attempt

**Status: Attempt should be removed from the AST.**

Handlers cross a process boundary and can always fail. Every handler naturally returns a Result. Error handling — retries, unwrap, propagation — is expressible via existing AST primitives (Loop + Switch + Chain). There is no need for a dedicated Attempt node in the AST or a special AttemptResult type.

- **Retry**: `Loop(Chain(Invoke(handler), Switch("ok" => Break, "err" => Continue)))` with a max iteration count.
- **Unwrap**: a builtin or AST node that extracts the Ok value or panics the workflow.
- **Map / and_then / `?`**: future AST combinators over Result values.

The `result` namespace builtins (§ Namespaced Builtins above) still make sense — they'd operate on the Result values that handlers naturally produce, not on a special AttemptResult wrapper.

See ENGINE_APPLIER.md § "Future: error handling is an AST concern" for the full rationale.

## Context

Read-only environment (`context: Value`) on `Config`, passed to all handlers. Carries API keys, workflow IDs, tenant config, etc.

Alternative: user-land Reader Monad pattern using `parallel` + `identity` + `merge` (see WORKFLOW_ALGEBRA.md). This incurs O(N) cloning cost for parallel branches, which the host-level context avoids.

## Effect Registries / Side-Effect Context

Beyond read-only context, handlers need a way to perform side effects (logging, metrics, tracing) through a structured API rather than ad-hoc I/O. This could take the form of an effect registry passed to handlers alongside the input and context — a set of capabilities the handler is allowed to use.

This overlaps with the Context feature above but is distinct: context is read-only data, effects are write-only capabilities. Both are provided by the host and available to all handlers without flowing through the data pipeline.

## ~~Attempt as Dynamic-Scope Context~~ — Subsumed by Attempt removal

With Attempt removed (see above), the dynamic-scope generalization loses its motivating example. The remaining use cases (retry policies, timeouts, tracing context) are worth revisiting independently:

- **Retry policies**: handled in userland via Loop + Switch (see above).
- **Timeouts**: likely need runtime support (not engine support) — the Scheduler or handler execution layer sets deadlines, not the AST.
- **Tracing/logging context**: could be a `Provide` / `Consume` mechanism or just part of the Context feature.

The general dynamic-scope idea (`Provide` / `Consume` pushing values onto the frame tree) may still be useful, but it should be motivated by a concrete need, not by Attempt.

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

## Handler Annotations and Dispatch Deduplication

Handlers could carry metadata annotations that enable the engine to skip redundant work:

- **Pure** (deterministic, no side effects): Given the same input, always produces the same output. The engine can **deduplicate dispatches** — if two Invoke nodes have the same `HandlerId` and the same input `Value`, the engine dispatches once and delivers the result to both Invoke frames. This is common subexpression elimination (CSE) at the dispatch level.

- **Idempotent** (safe to retry, but may have side effects): Re-executing with the same input produces the same observable effect. Useful for retry policies — the engine can safely re-dispatch on timeout without worrying about double-charging, double-emailing, etc. Doesn't enable deduplication (side effects may differ between calls), but enables automatic retry.

- **Read-only** (no side effects, but may be nondeterministic): Depends on external state that might change between calls (e.g., "get current inventory"). Safe to deduplicate within a single `advance()` expansion (the state won't change between dispatches produced in the same batch), but not across completions.

### Dispatch deduplication for pure handlers

During `advance()`, the engine accumulates dispatches. Before yielding them to the runtime, it scans for duplicates: pairs where `(handler_id, value)` are equal and the handler is annotated pure. Duplicates share a single dispatch; when the result arrives, it's delivered to all waiting Invoke frames.

Implementation sketch:
- `pending_dispatches` gains a dedup index: `HashMap<(HandlerId, ValueHash), TaskId>` mapping `(handler, input)` to an existing task.
- When a new Invoke dispatch matches an existing entry, the new Invoke frame's `task_id` is set to the existing `TaskId`. `task_to_frame` becomes `task_to_frames: HashMap<TaskId, Vec<FrameId>>` (one task can complete multiple Invoke frames).
- On completion, the result is cloned to each frame in the vec.

This matters for Parallel where multiple branches invoke the same pure handler with the same input — e.g., `parallel(fetchUser(userId), fetchUser(userId))` dispatches once instead of twice.

### Annotation mechanism

Annotations live on `HandlerKind` (or a new `HandlerMetadata` struct). The TS surface DSL would specify them in `createHandler`:

```ts
createHandler({
  annotations: { pure: true },
  handle: async ({ value }) => { ... },
})
```

The annotations serialize into the handler metadata and are available to the engine at runtime. The flattener preserves them in the handler pool.

### Scope

This is purely an optimization — the engine produces correct results without annotations. Annotations are opt-in; unannotated handlers are treated as effectful (no deduplication, no automatic retry).

## Runtime Value Type Checking

The engine already does structural type checks in `advance` — ForEach panics if the input isn't an array, Branch panics if the input lacks a `kind` field. These are ad-hoc checks on specific combinators. A general mechanism would validate the input value against the handler's declared schema before every dispatch (or advance step).

### Where it belongs: advance, not dispatch

Type checking in advance is natural because advance already introspects values:
- ForEach: `match value { Value::Array(elements) => ..., other => panic!(...) }`
- Branch: `value["kind"].as_str().expect(...)`

Extending this to handler schemas: when advance hits an Invoke, it could validate the value against the handler's `value_schema` (if declared) before creating the Invoke frame and dispatch. A type mismatch would be caught immediately in the engine, with a full frame-tree stack trace available, rather than failing later in the handler subprocess with an opaque error.

The alternative — checking at dispatch time (in the scheduler/runtime) — catches the same errors but later, after the value has left the engine. The engine's frame tree context is gone. Error messages are worse.

### What it looks like

```rust
FlatAction::Invoke { handler } => {
    if let Some(schema) = self.flat_config.handler_value_schema(handler) {
        validate(&value, schema)
            .unwrap_or_else(|e| panic!("Type error at Invoke: {e}"));
    }
    // ... create frame, push dispatch ...
}
```

The `value_schema` on handlers is already a `Option<Value>` (JSON Schema). Validation would use a JSON Schema validator crate (e.g., `jsonschema`).

### Panics vs error propagation

Currently type mismatches panic. With the error propagation mechanism (COMPLETION.md), they should produce engine errors instead — propagating through the frame tree like handler failures, catchable by Attempt. This makes type errors recoverable and gives them the same stack-trace treatment as handler errors.

### Not now

This depends on having a JSON Schema validation crate in the dependency tree and wiring up handler schemas through the flattener. Not needed for the initial engine milestones. The existing panics (ForEach non-array, Branch missing kind) are sufficient for now.

## Schema Validation Elision

If we add runtime validation (checking values against handler schemas before dispatch), we can skip redundant checks when the engine knows a value already satisfies a schema.

The key insight: Parallel clones the same input to N children. If all N children expect the same input type (same handler, same schema), we validate once and skip N-1 checks. The value hasn't changed — it's a clone of a validated value.

More generally, a value that has been validated against schema S remains valid for S as long as no handler has transformed it. Pass-through operations (Branch case lookup, Step redirect, Chain forwarding to rest with the same value) preserve the validation. Only Invoke (which produces a new value from a handler) invalidates the "already checked" status.

### How it could work

Track a `SchemaId` (or `HandlerId` as proxy) on values as they flow through the engine. When a value is validated against a handler's input schema, tag it. When the same schema is expected again, skip validation.

Implementation options:

1. **Per-dispatch dedup**: During `advance()`, if two dispatches target the same handler with the same input value (by identity, not equality), validate once. This is a subset of the pure handler deduplication optimization (above) — schema validation elision is the type-checking counterpart of dispatch deduplication.

2. **Value tagging**: Attach a "validated for" set to values as engine-internal metadata. When a value passes through identity operations, the tag carries forward. When it's transformed (handler output), the tag is cleared. This is more general but requires wrapping `Value` or maintaining a side table.

3. **Compile-time analysis**: The flattener can determine statically which paths share input types. If Parallel's children all expect the same schema, emit a flag that tells the engine to validate once. This moves the analysis to compile time (no runtime bookkeeping) but is less general.

### When it matters

Irrelevant until runtime validation exists. Currently, handlers receive raw `Value` with no schema checking. When we add handler input validation (via `inputValidator` schemas), this optimization prevents O(N) redundant validations in Parallel/ForEach fan-outs.

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
  inputValidator: z.object({ ... }),
  errorType: z.object({ code: z.string(), message: z.string() }),
  handle: async ({ value }) => { ... },
})
```

The error type defaults to `unknown` in TypeScript. The `attempt` combinator would then produce `AttemptResult<TOutput, TError>` instead of `AttemptResult<TOutput>` with `error: unknown`.

## Workflow Stack Traces

When a handler panics, fails, or the engine hits an unexpected state, the error message should include a meaningful stack trace showing the workflow path that led to the failure — not a Rust call stack, but a Barnum frame trace.

### What a Barnum stack trace looks like

The frame tree already contains the information: every frame has a `parent`, forming a chain from the failure point to Root. Walk the parent chain and emit a trace:

```
Handler error in ./payment.ts:charge
  at Invoke (action 14)
  at Chain rest (action 12)
  at Parallel child 2 of 3 (action 8)
  at Chain rest (action 5)
  at Attempt (action 3)
  at Root
```

Each frame in the trace can include:
- **Frame kind**: Invoke, Chain, Parallel, ForEach, Loop, Attempt
- **ActionId**: position in the flat table (useful for developer debugging)
- **Structural context**: "child 2 of 3" for Parallel, "iteration N" for Loop
- **Handler identity**: for Invoke frames, the handler's module path + function name

### Implementation

Two approaches:

1. **On-demand trace**: When an error occurs, walk the frame tree's parent chain upward from the failing frame. No per-frame overhead — the trace is constructed only on error. This is the natural approach since the parent chain already exists.

2. **Precomputed path**: Each frame stores its full path (a `Vec<FrameId>` or similar). Updated during advance. Costs memory proportional to tree depth × number of frames. Not worth it for the common case.

On-demand is the right choice. The engine already has `parent` pointers — walking them is O(depth) which is bounded by workflow nesting.

### Named anchors

The trace above uses ActionIds, which are opaque to workflow authors. To make traces human-readable, actions could carry optional names:

- Step references already have names (`StepName`). Step frames in the trace show the step name.
- Handlers have module path + function name. Invoke frames show these.
- Combinators (`pipe`, `parallel`, `branch`) could accept an optional label parameter in the TS surface DSL: `pipe("checkout-flow", ...)`. The label would serialize into the AST and survive flattening as metadata on the FlatEntry.

Without labels, the trace falls back to ActionIds + handler identities, which is still more useful than nothing.

### Panic hook integration

In the Rust engine, panics (from `expect`, `panic!`, or unexpected states) produce a Rust stack trace that's useless to workflow authors. A custom panic hook could:

1. Catch the panic
2. Walk the frame tree to build the Barnum trace
3. Include both the Rust panic message and the Barnum trace in the error output

This requires the engine (or a thread-local) to be accessible from the panic hook. The engine is `!Sync` (single-threaded), so thread-local access is straightforward.

### Error propagation traces

When `error()` propagates up the frame tree, it could accumulate a trace: each frame the error passes through adds a line. By the time the error reaches Root (or is caught by Attempt), the trace shows the full propagation path including cancelled siblings. This is richer than a simple parent-chain walk — it shows the dynamic error path, not just the static frame ancestry.

## Value Interning

Values (`serde_json::Value`) flow through the engine by move/clone. Parallel clones the input for each child — `value.clone()` deep-copies the entire JSON tree. For a 10KB payload fanned out to 20 parallel branches, that's 200KB of redundant copies.

### Level 1: Rc<Value> (cheap clones)

Replace `Value` with `Rc<Value>` in the engine's internal data flow. Parallel's `value.clone()` becomes an Rc clone — O(1), just an increment of the reference count. No deep copy.

```rust
// Before: deep clone per child
for (i, child) in children.into_iter().enumerate() {
    self.advance(child, value.clone(), ...);
}

// After: Rc clone per child (O(1))
for (i, child) in children.into_iter().enumerate() {
    self.advance(child, Rc::clone(&value), ...);
}
```

The rest of the engine is unchanged — it just passes `Rc<Value>` instead of `Value`. `Rc` is appropriate because the engine is single-threaded (`!Sync`). `Arc` would work too but has unnecessary atomic overhead.

**When values diverge:** Handlers produce new values (not mutations of existing ones). When an Invoke frame completes, the result is a new `Rc<Value>` — the old shared input is dropped naturally when all Rc references go out of scope. No copy-on-write needed because values are never mutated in the engine.

**Dispatch boundary:** `Dispatch` carries a value to the runtime. If the runtime needs ownership (e.g., to send to a handler subprocess), it can `Rc::try_unwrap()` or clone at that point. The clone only happens once per dispatch, not once per Parallel child.

**Cost:** Rc adds a pointer indirection and 8 bytes of refcount overhead per value. Negligible compared to the deep-clone savings.

### Level 2: Value intern table (deduplication + identity equality)

A step beyond Rc: deduplicate structurally identical values via an intern table.

```rust
struct ValuePool {
    table: HashMap<Value, ValueId>,
    values: Vec<Value>,
}
```

When a value enters the engine (from `start()` or `on_task_completed()`), it's looked up in the pool. If it already exists, the existing `ValueId` is reused. Structurally identical values share a single allocation.

**Benefits:**
- **Identity equality:** `value_a == value_b` becomes `value_id_a == value_id_b` — O(1) instead of O(n) structural comparison. This enables cheap dispatch deduplication for pure handlers (same handler + same ValueId = skip redundant dispatch).
- **Memory deduplication:** If multiple handlers return the same value (e.g., `null`, `true`, common error objects), only one copy exists.

**Costs:**
- **Hashing:** `Value` hashing is recursive over the JSON tree. For large values, this is expensive. The hash cost may exceed the clone cost for values that are only used once.
- **Lifetime management:** When should entries be evicted? Reference counting per entry, or GC pass between engine steps? An Rc-based approach (Level 1) handles this automatically; an intern table needs explicit management.
- **Floating-point hashing:** JSON numbers include floats. `f64` is not `Hash` in Rust. Need a wrapper that hashes the bits (`f64::to_bits()`), which means `NaN != NaN` in the intern table. Edge case but real.

**Verdict:** Level 1 (Rc) is the clear first step — trivial to implement, no downsides, eliminates Parallel deep clones. Level 2 (intern table) is worth pursuing only when dispatch deduplication for pure handlers is implemented, since that's the main consumer of identity equality.

### Interaction with other features

- **Dispatch deduplication (Handler Annotations):** Requires comparing input values for equality. With interning, this is O(1) by ValueId. Without interning, it's O(n) structural comparison per dispatch pair.
- **Schema validation elision:** If values are interned, "this value was already validated" can be tracked per ValueId rather than per value instance.
- **Snapshot testing:** Interned values serialize identically to plain values. No impact on test output.

## Streams

Support for streaming data through the pipeline — actions that produce or consume async iterables rather than single values. Relevant for large datasets, real-time feeds, or incremental processing where buffering the full result is impractical.

Open question: is this a new primitive (e.g., `StreamForEach`) or a modifier on existing primitives? Could also be a handler-level concern (handlers that yield multiple values) rather than an AST-level feature.

## Fluent API (`.then()`, `.attempt()`, etc.)

Currently, composing actions uses nested function calls:

```ts
pipe(
  constant(42),
  fetchUser(),
  branch({
    Active: processActive(),
    Inactive: archiveUser(),
  })
)
```

A fluent API would use dot-chaining:

```ts
constant(42)
  .then(fetchUser())
  .then(branch({
    Active: processActive(),
    Inactive: archiveUser(),
  }))
```

`.then()` is the big win — it replaces `pipe()` for sequential composition. `pipe()` has 10 overloads to handle 1-10 arguments, each manually threading the type parameters. `.then()` is a single method that chains naturally with full type inference.

### Which operations work as dot methods

**Sequencing (the main one):**
- **`.then(action)`** → `chain(this, action)`. The action receives this action's output as input.

**Wrapping (modifiers on this action):**
- **`.attempt()`** → `attempt(this)`. Wrap this action in a try/catch boundary. Result is `AttemptResult<Out>`.
- **`.forEach()`** → `forEach(this)`. Apply this action to each element of the input array. Input becomes `In[]`, output becomes `Out[]`.
- **`.loop()`** → `loop(this)`. Repeat this action until it returns `{ kind: "Break", value }`. The action body must return a `LoopResult`.

**Don't make sense as dot methods:**
- `parallel(a, b, c)` — symmetric. No "this" that the others follow. Stays as a function call.
- `branch(cases)` — takes a cases object, not an action to chain after. Use `.then(branch({...}))`.
- `step("name")` — a reference, not a modification of an existing action.

### Implementation: non-enumerable properties

`TypedAction` is currently a branded type over plain `Action` objects (discriminated union with phantom type fields). Adding methods requires attaching them to the objects without breaking JSON serialization.

Non-enumerable properties solve this. `JSON.stringify` skips non-enumerable properties, so they don't appear in serialized output. But `action.then(...)` still works at runtime:

```ts
function withMethods<In, Out, Refs extends string>(
  action: Action,
): TypedAction<In, Out, Refs> {
  Object.defineProperties(action, {
    then: {
      value<Next, R extends string>(
        this: TypedAction<In, Out, Refs>,
        next: TypedAction<Out, Next, R>,
      ): TypedAction<In, Next, Refs | R> {
        return chain(this, next);
      },
      enumerable: false,
    },
    attempt: {
      value<In, Out, Refs extends string>(
        this: TypedAction<In, Out, Refs>,
      ): TypedAction<In, AttemptResult<Out>, Refs> {
        return attempt(this);
      },
      enumerable: false,
    },
    forEach: {
      value<In, Out, Refs extends string>(
        this: TypedAction<In, Out, Refs>,
      ): TypedAction<In[], Out[], Refs> {
        return forEach(this);
      },
      enumerable: false,
    },
    loop: {
      value<In, Out, Refs extends string>(
        this: TypedAction<In, Out, Refs>,
      ): TypedAction<In, Out, Refs> {
        return loop(this);
      },
      enumerable: false,
    },
  });
  return action as TypedAction<In, Out, Refs>;
}
```

Every combinator function (`chain`, `parallel`, `branch`, `invoke`, `constant`, etc.) would call `withMethods()` on the Action object before returning it. This is similar to the existing `Object.assign` pattern used by `createHandler` for `CallableHandler`.

### TypeScript declaration

The `TypedAction` type gains method signatures:

```ts
export type TypedAction<In, Out, Refs extends string = never> = Action & {
  __phantom_in?: (input: In) => void;
  __phantom_out?: () => Out;
  __in?: In;
  __refs?: { _brand: Refs };

  then<Next, R extends string>(
    next: TypedAction<Out, Next, R>,
  ): TypedAction<In, Next, Refs | R>;

  attempt(): TypedAction<In, AttemptResult<Out>, Refs>;
  forEach(): TypedAction<In[], Out[], Refs>;
  loop(): TypedAction<In, Out, Refs>; // Out must be LoopResult<In, T>
};
```

### Coexistence with functional API

The fluent API doesn't replace the functional API — both coexist. `pipe()` and `chain()` remain available. Users choose based on preference:

```ts
// Functional style
const workflow = pipe(constant(42), fetchUser(), processUser());

// Fluent style
const workflow = constant(42).then(fetchUser()).then(processUser());

// Mixed
const workflow = parallel(
  constant(42).then(fetchUser()),
  constant(99).then(fetchAdmin()),
);
```

`parallel` and `branch` stay as functions. `.then()` replaces `pipe` for the common case of linear sequencing.

### Ergonomic impact

The big win: `.then()` eliminates `pipe()` overloads. Currently `pipe` has 10 overloads (for 1-10 arguments) because TypeScript can't infer a variadic chain of type parameters. Each overload manually threads `T1 → T2 → T3 → ...`. Adding an 11th step means adding an 11th overload.

`.then()` has a single signature: `then<Next>(next: TypedAction<Out, Next>): TypedAction<In, Next>`. It chains indefinitely without overloads. The type system infers `Out` from the preceding `.then()` call automatically.

### Promise analogy

`.then()` mirrors `Promise.then()` — a familiar pattern for JS/TS developers. A `TypedAction` is like a "deferred computation" that you compose with `.then()`. The parallel to Promises is intentional:

| Promise | TypedAction |
|---|---|
| `.then(fn)` | `.then(action)` |
| `Promise.all([p1, p2])` | `parallel(a1, a2)` |
| `try { await p } catch (e)` | `.attempt()` |

This makes the API immediately intuitive to anyone who's used Promises.

### Open question: should TypedAction be a class?

Non-enumerable properties work but are somewhat unusual. An alternative: make `TypedAction` a proper class with a `.toJSON()` method that serializes only the `Action` data (excluding methods). This is more conventional OOP, but:
- Breaking change (all combinator functions must return class instances)
- Heavier (class overhead, prototype chain)
- `createHandler`'s `Object.assign` pattern would need adjustment

Non-enumerable properties are the lighter touch. Revisit if the pattern causes issues.

## ~~Constant and Range Builtins~~ (Implemented)

Implemented in `libs/barnum/src/builtins.ts`. `constant<T>(value)` and `range(start, end)` are available as TypeScript builtins using placeholder `__builtin__` Invoke nodes.

## ~~Handler as Callable~~ (Implemented)

Implemented in `libs/barnum/src/handler.ts`. `createHandler` returns a `CallableHandler` — a function that produces `TypedAction` when invoked, with Handler metadata (`__filePath`, `__definition`, brand symbol) attached via `Object.assign`. Direct invocation: `setup()` or `setup({ stepConfig: { timeout: 5000 } })`. `invoke()` still works for explicit invocation.

## Engine-level Pick (Schema-based Input Filtering)

With invariant types (INVARIANT_TYPES.md), the type system guarantees that only declared fields arrive at a handler boundary. The `pick` builtin constructs a new object with only the named fields at runtime.

A more advanced feature: the engine itself could enforce input filtering at handler boundaries based on the handler's JSON schema. When the engine dispatches to a handler, it strips any fields not declared in the handler's `inputValidator` schema before serializing.

This provides defense-in-depth: even if a type-level `pick` is accidentally omitted, the engine never sends undeclared fields to a handler. It also enables polyglot handlers — a Rust or Python handler that strict-deserializes its input would never see unexpected fields.

**Why deferred**: The type system should be the primary enforcement mechanism. Engine-level filtering is a safety net, not a substitute. It also adds per-dispatch overhead (schema introspection) and requires all handlers to have schemas (currently `inputValidator` is optional). Worth revisiting once the invariant type system is stable and handler schemas are mandatory.
