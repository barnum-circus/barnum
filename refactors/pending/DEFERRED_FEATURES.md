# Deferred Features

Features removed from the initial implementation to keep the surface area minimal. To be added incrementally as needed.

## Language-Aware Coalescing and Builtin Placement

When consecutive actions in a pipe share the same execution language, they can be coalesced into a single dispatch — eliminating per-step overhead of crossing the Rust/TypeScript boundary. Builtins (identity, merge, getField, etc.) can execute in any language; the compiler should place them to minimize boundary crossings.

See `past/COMPILATION.md` for full details.

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

## Lazy Step Flattening

Currently, flattening eagerly processes all steps in `Config::steps`, even if some are never referenced by the workflow. This is wasted work and inflates the flat table with dead entries.

Lazy flattening: only flatten a step when the flattener first encounters a `Step` reference to it. Steps that are never referenced are never flattened. This is a natural fit for the two-pass model — pass 1 reserves ActionIds for steps when they're first referenced, pass 2 resolves them. The change is to skip pre-allocating entries for unreferenced steps entirely.

Benefits:
- Smaller flat tables when configs contain library-style step registries (many steps defined, few used per workflow).
- Faster flattening for large configs.
- Dead step detection for free — any step that wasn't flattened after the walk is unreferenced.

This could go further: flatten steps on-demand during execution, not just during the flattening pass. The engine flattens the workflow root eagerly (down to the first Invoke leaves), dispatches those handlers, and while waiting for results, lazily flattens any Step targets that haven't been flattened yet. Step bodies behind a Chain's `rest` or inside a Branch case that hasn't been taken yet don't need to exist in the flat table until the engine actually reaches them. This turns flattening into an incremental process interleaved with execution — only the reachable frontier is materialized at any given time.

The current eager approach is simpler and correct. Lazy/incremental flattening is an optimization for when config sizes grow.

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

Processing a sequence of events one at a time as they arrive: poll for the next item, process it, repeat until the source is exhausted or a condition is met.

### Core pattern: loop + invoke

This doesn't require new engine primitives. A stream consumer is a `loop` that invokes a "next item" handler on each iteration:

```ts
// waitForPrEvent: TypedAction<PrUrl, PrEvent>
// PrEvent = { kind: "CiCompleted"; value: ... } | { kind: "ReviewSubmitted"; value: ... } | ...

const babysitPr = loop<PrResult, PrUrl>((recur, done) =>
  pipe(
    waitForPrEvent,
    branch({
      CiCompleted: pipe(handleCiResult, recur),
      ReviewSubmitted: pipe(handleReview, recur),
      Closed: done,
    }),
  ),
);
```

The engine suspends at `waitForPrEvent` (an Invoke). The runtime resolves it when an event arrives (webhook, polling, whatever). The engine processes the event, recurs, and suspends again. No new AST nodes, no new engine code.

### `forEachStream` combinator

Sugar for the common case where every item is processed the same way and the stream has an explicit end signal. The source handler returns `Option<TElement>` — `Some` with the next item, or `None` when exhausted:

```ts
function forEachStream<TIn, TElement, TOut>(
  source: Pipeable<TIn, Option<TElement>>,
  body: Pipeable<TElement, unknown>,
): TypedAction<TIn, TOut[]>
```

Desugars to:

```ts
function forEachStream(source, body) {
  return recur<{ items: unknown[]; input: TIn }>((restart) =>
    pipe(
      bindInput(({ items, input }) =>
        pipe(
          input,
          source,
          O.match({
            Some: pipe(body, /* append to items, */ restart),
            None: pipe(drop, items, done),
          }),
        ),
      ),
    ),
  );
}
```

Actually, this is awkward because we need to accumulate results across iterations. `loop` handles this naturally — `recur` carries state:

```ts
function forEachStream(source, body) {
  return loop<TOut[], { items: TOut[]; input: TIn }>((recur, done) =>
    bindInput<{ items: TOut[]; input: TIn }>((state) =>
      pipe(
        state.getField("input"),
        source,
        branch({
          Some: pipe(body, /* build new state with appended item, */ recur),
          None: pipe(drop, state.getField("items"), done),
        }),
      ),
    ),
  );
}
```

This is a derived combinator — no engine changes. The engine sees loop/branch/invoke, same as any other workflow.

### Postfix `.forEachStream(source, body)`

Following the existing pattern of postfix methods (`.then()`, `.forEach()`, `.branch()`):

```ts
// On TypedAction:
forEachStream<TIn, TElement, TOut>(
  this: TypedAction<TIn, Option<TElement>>,
  body: Pipeable<TElement, TOut>,
): TypedAction<TIn, TOut[]>;
```

Usage:

```ts
const processAllEvents = getEventSource
  .forEachStream(processEvent);
```

This reads as: "get the event source, then for each stream element, process it."

Alternative: the source and body are separate arguments to a standalone function, not a postfix. The postfix only makes sense if the receiver IS the source.

### Finite vs infinite streams

The `Option<TElement>` convention gives finite streams: `None` terminates. For infinite streams (event loops that run until an external condition), use `loop` directly with `done` as the exit signal:

```ts
// Infinite: runs until Closed event
const babysitPr = loop<PrResult, PrUrl>((recur, done) =>
  pipe(waitForPrEvent, branch({
    CiCompleted: pipe(handleCi, recur),
    Closed: done,
  })),
);

// Finite: processes items until source returns None
const processAll = forEachStream(fetchNextItem, processItem);
```

### What the runtime needs

The "next item" handler (`waitForPrEvent`, `fetchNextItem`) is an ordinary Invoke handler. The runtime resolves it however it wants:

- **Polling**: handler calls an API, returns the result or `None` if empty
- **Webhook/push**: handler blocks (async) until an event arrives, returns it
- **Buffered**: handler pulls from an internal queue, returns `None` when drained

The engine doesn't know or care about the delivery mechanism. It dispatches the handler, waits for `complete()`, processes the result.

### Backpressure

Backpressure is automatic. The engine won't dispatch the next `waitForPrEvent` until the current iteration's body completes (all its Invokes resolve). If processing is slow, the source handler simply isn't called — no events pile up in the engine. Events may queue in the runtime/external system, but that's outside the engine's scope.

### No new engine primitives needed

Streams are a pattern, not a primitive. The engine already has everything: `loop` for iteration, `branch` for dispatch, `Invoke` for blocking on external data, `recur`/`done` for continue/break. The `forEachStream` combinator is pure sugar that compiles to these existing building blocks.

See also: `refactors/pending/EVENT_LOOP_PATTERN.md` for the concrete PR babysitter use case.

## Engine-level Pick (Schema-based Input Filtering)

With invariant types (INVARIANT_TYPES.md), the type system guarantees that only declared fields arrive at a handler boundary. The `pick` builtin constructs a new object with only the named fields at runtime.

A more advanced feature: the engine itself could enforce input filtering at handler boundaries based on the handler's JSON schema. When the engine dispatches to a handler, it strips any fields not declared in the handler's `inputValidator` schema before serializing.

This provides defense-in-depth: even if a type-level `pick` is accidentally omitted, the engine never sends undeclared fields to a handler. It also enables polyglot handlers — a Rust or Python handler that strict-deserializes its input would never see unexpected fields.

**Why deferred**: The type system should be the primary enforcement mechanism. Engine-level filtering is a safety net, not a substitute. It also adds per-dispatch overhead (schema introspection) and requires all handlers to have schemas (currently `inputValidator` is optional). Worth revisiting once the invariant type system is stable and handler schemas are mandatory.

## Boolean-to-Enum Builtin

Branch dispatches on tagged unions (`{ kind, value }`). Booleans can't be branched on directly — you need to convert `true`/`false` to `{ kind: "True", value: void }` / `{ kind: "False", value: void }` first.

A `boolToEnum` builtin would do this conversion inline in Rust:

```ts
type BoolDef = { True: void; False: void };
type Bool = TaggedUnion<BoolDef>;

function boolToEnum(): TypedAction<boolean, Bool>
```

Desugars to a Builtin handler that reads the boolean and produces the tagged union:

```rust
BuiltinKind::BoolToEnum => {
    let kind = if value.as_bool().unwrap() { "True" } else { "False" };
    json!({ "kind": kind, "value": null, "__def": null })
}
```

This enables `ifElse` as a surface combinator:

```ts
function ifElse<TIn, TOut>(
  condition: Pipeable<TIn, boolean>,
  thenAction: Pipeable<void, TOut>,
  elseAction: Pipeable<void, TOut>,
): TypedAction<TIn, TOut> {
  return pipe(
    condition,
    boolToEnum(),
    branch({ True: thenAction, False: elseAction }),
  );
}
```

The `Bool` type would be a proper TaggedUnion with `__def`, so `.branch()` works on it with exhaustiveness checking. The `True`/`False` cases receive `void` (the boolean carries no data beyond the discriminant).

This is a small addition — one Builtin variant, one combinator, one type alias. No engine changes.
