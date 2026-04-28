# Deferred Features

Features removed from the initial implementation to keep the surface area minimal. To be added incrementally as needed.

## Language-Aware Coalescing and Builtin Placement

When consecutive actions in a pipe share the same execution language, they can be coalesced into a single dispatch — eliminating per-step overhead of crossing the Rust/TypeScript boundary. Builtins (identity, merge, getField, etc.) can execute in any language; the compiler should place them to minimize boundary crossings.

See `past/COMPILATION.md` for full details.

## Trivial Combinator Elimination

Compile-time simplifications during flattening (or a validation/normalization pass):

- **`All([A])`**: NOT a trivial elimination. `All([A])` produces `[A(x)]` while `A` produces `A(x)` — different output shapes (array-wrapped vs unwrapped). Eliminating the All requires also wrapping the child's output in an array, which means a builtin. Not worth pursuing until builtins exist.

- **`All([])`**: Produces `[]` (empty tuple). The TS `all()` already compiles this to `constant([])` at build time. The Rust flattener should also handle `All { actions: [] }` by rewriting to a constant empty array, as a defensive measure. Important for constant folding and dead code elimination.

Other potential simplifications to investigate as the AST matures.

## Handler Annotations and Dispatch Deduplication

Handlers could carry metadata annotations that enable the engine to skip redundant work:

- **Pure** (deterministic, no side effects): Given the same input, always produces the same output. The engine can **deduplicate dispatches** — if two Invoke nodes have the same `HandlerId` and the same input `Value`, the engine dispatches once and delivers the result to both Invoke frames. This is common subexpression elimination (CSE) at the dispatch level.

- **Idempotent** (safe to retry, but may have side effects): Re-executing with the same input produces the same observable effect. Useful for retry policies — the engine can safely re-dispatch on timeout without worrying about double-charging, double-emailing, etc. Doesn't enable deduplication (side effects may differ between calls), but enables automatic retry.

- **Read-only** (no side effects, but may be nondeterministic): Depends on external state that might change between calls (e.g., "get current inventory"). Safe to deduplicate within a single `advance()` expansion (the state won't change between dispatches produced in the same batch), but not across completions.

### Dispatch deduplication for pure handlers

During `advance()`, the engine accumulates effects in `pending_effects`. Before yielding them to the runtime, it scans for duplicates: pairs where `(handler_id, value)` are equal and the handler is annotated pure. Duplicates share a single dispatch; when the result arrives, it's delivered to all waiting Invoke frames.

Implementation sketch:
- `pending_effects` gains a dedup index: `HashMap<(HandlerId, ValueHash), TaskId>` mapping `(handler, input)` to an existing task.
- When a new Invoke dispatch matches an existing entry, the new Invoke frame's `task_id` is set to the existing `TaskId`. `task_to_frame` becomes `task_to_frames: HashMap<TaskId, Vec<FrameId>>` (one task can complete multiple Invoke frames).
- On completion, the result is cloned to each frame in the vec.

This matters for All where multiple branches invoke the same pure handler with the same input — e.g., `all(fetchUser(userId), fetchUser(userId))` dispatches once instead of twice.

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

## Workflow Stack Traces

When a handler panics, fails, or the engine hits an unexpected state, the error message should include a meaningful stack trace showing the workflow path that led to the failure — not a Rust call stack, but a Barnum frame trace.

### What a Barnum stack trace looks like

The frame tree already contains the information: every frame has a `parent`, forming a chain from the failure point to Root. Walk the parent chain and emit a trace:

```
Handler error in ./payment.ts:charge
  at Invoke (action 14)
  at Chain rest (action 12)
  at All child 2 of 3 (action 8)
  at Chain rest (action 5)
  at RestartHandle (action 3)
  at Root
```

Each frame in the trace can include:
- **Frame kind**: Invoke, Chain, All, ForEach, Branch, ResumeHandle, RestartHandle
- **ActionId**: position in the flat table (useful for developer debugging)
- **Structural context**: "child 2 of 3" for All, "iteration N" for RestartHandle (loop)
- **Handler identity**: for Invoke frames, the handler's module path + function name

### Implementation

Two approaches:

1. **On-demand trace**: When an error occurs, walk the frame tree's parent chain upward from the failing frame. No per-frame overhead — the trace is constructed only on error. This is the natural approach since the parent chain already exists.

2. **Precomputed path**: Each frame stores its full path (a `Vec<FrameId>` or similar). Updated during advance. Costs memory proportional to tree depth × number of frames. Not worth it for the common case.

On-demand is the right choice. The engine already has `parent` pointers — walking them is O(depth) which is bounded by workflow nesting.

### Named anchors

The trace above uses ActionIds, which are opaque to workflow authors. To make traces human-readable, actions could carry optional names:

- Handlers have module path + function name. Invoke frames show these.
- Combinators (`pipe`, `all`, `branch`) could accept an optional label parameter in the TS surface DSL: `pipe("checkout-flow", ...)`. The label would serialize into the AST and survive flattening as metadata on the FlatEntry.
- Recursive functions defined via `defineRecursiveFunctions` could carry their function names as labels.

Without labels, the trace falls back to ActionIds + handler identities, which is still more useful than nothing.

### Panic hook integration

In the Rust engine, panics (from `expect`, `panic!`, or unexpected states) produce a Rust stack trace that's useless to workflow authors. A custom panic hook could:

1. Catch the panic
2. Walk the frame tree to build the Barnum trace
3. Include both the Rust panic message and the Barnum trace in the error output

This requires the engine (or a thread-local) to be accessible from the panic hook. The engine is `!Sync` (single-threaded), so thread-local access is straightforward.

### Error propagation traces

When an error propagates up the frame tree, it could accumulate a trace: each frame the error passes through adds a line. By the time the error reaches Root (or is caught by a RestartHandle implementing tryCatch), the trace shows the full propagation path including cancelled siblings. This is richer than a simple parent-chain walk — it shows the dynamic error path, not just the static frame ancestry.

## Value Interning

Values (`serde_json::Value`) flow through the engine by move/clone. All clones the input for each child — `value.clone()` deep-copies the entire JSON tree. For a 10KB payload fanned out to 20 All branches, that's 200KB of redundant copies.

### Level 1: Rc<Value> (cheap clones)

Replace `Value` with `Rc<Value>` in the engine's internal data flow. All's `value.clone()` becomes an Rc clone — O(1), just an increment of the reference count. No deep copy.

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

When a value enters the engine (from `WorkflowState::new()` or `complete()`), it's looked up in the pool. If it already exists, the existing `ValueId` is reused. Structurally identical values share a single allocation.

**Benefits:**
- **Identity equality:** `value_a == value_b` becomes `value_id_a == value_id_b` — O(1) instead of O(n) structural comparison. This enables cheap dispatch deduplication for pure handlers (same handler + same ValueId = skip redundant dispatch).
- **Memory deduplication:** If multiple handlers return the same value (e.g., `null`, `true`, common error objects), only one copy exists.

**Costs:**
- **Hashing:** `Value` hashing is recursive over the JSON tree. For large values, this is expensive. The hash cost may exceed the clone cost for values that are only used once.
- **Lifetime management:** When should entries be evicted? Reference counting per entry, or GC pass between engine steps? An Rc-based approach (Level 1) handles this automatically; an intern table needs explicit management.
- **Floating-point hashing:** JSON numbers include floats. `f64` is not `Hash` in Rust. Need a wrapper that hashes the bits (`f64::to_bits()`), which means `NaN != NaN` in the intern table. Edge case but real.

**Verdict:** Level 1 (Rc) is the clear first step — trivial to implement, no downsides, eliminates All deep clones. Level 2 (intern table) is worth pursuing only when dispatch deduplication for pure handlers is implemented, since that's the main consumer of identity equality.

### Interaction with other features

- **Dispatch deduplication (Handler Annotations):** Requires comparing input values for equality. With interning, this is O(1) by ValueId. Without interning, it's O(n) structural comparison per dispatch pair.
- **Schema validation elision:** If values are interned, "this value was already validated" can be tracked per ValueId rather than per value instance.
- **Snapshot testing:** Interned values serialize identically to plain values. No impact on test output.

Note: The engine currently uses `pending_effects: VecDeque<PendingEffect>` to queue dispatches, and `task_to_frame: BTreeMap<TaskId, FrameId>` to track which frame owns which task. Dispatch deduplication would add an index on top of this structure.

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

The engine doesn't know or care about the delivery mechanism. It dispatches the handler via `advance()`, waits for the runtime to call `complete()`, processes the result.

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

## Free-standing Iterator Combinators

Currently `splitFirst`, `splitLast`, `fold`, `isEmpty`, `slice`, `take`, `skip` only exist as postfix methods on `Iterator<T>`. When the pipeline input is already an `Iterator<T>` (e.g., inside a `loop` body), you're forced to write `identity<Iterator<T>>().splitFirst()` to access the postfix method — ugly and non-obvious.

Add free-standing versions under the `Iterator` namespace:

```ts
Iterator.splitFirst<T>()   // TypedAction<Iterator<T>, Option<[T, Iterator<T>]>>
Iterator.splitLast<T>()    // TypedAction<Iterator<T>, Option<[Iterator<T>, T]>>
Iterator.fold<T, TAcc>(initial, reducer)
Iterator.isEmpty<T>()      // TypedAction<Iterator<T>, boolean>
Iterator.slice<T>()        // + take, skip
```

These would be thin wrappers (same AST output as the postfix versions) but make pipeline-start usage clean: `Iterator.splitFirst<string>()` instead of `identity<Iterator<string>>().splitFirst()`.

## Generalize ExtractPrefix

`ExtractPrefix` (from UNION_DISPATCH_AST_NODES.md) is a bespoke builtin that splits a `kind` string on `'.'` and restructures the value. It could be replaced by a more general string-processing primitive — e.g., a regex-based builtin that extracts capture groups, or a general "split string field" operation. This would make `ExtractPrefix` a derived combinator built from the general primitive rather than a special-cased builtin.

Not urgent — `ExtractPrefix` handles the concrete need (Option/Result dispatch). Generalize when a second use case for in-engine string processing appears.

## Boolean-to-Enum Builtin

**Partially implemented.** The `AsOption` builtin (`crates/barnum_builtins/src/lib.rs`) converts `boolean → Option<void>`: `true` → `{ kind: "Option.Some", value: null }`, `false` → `{ kind: "Option.None", value: null }`. This enables branching on booleans via `asOption()` + `branch({ Some: ..., None: ... })`.

The original proposal was a more general `boolToEnum` producing `{ kind: "True", value: void } | { kind: "False", value: void }`. `AsOption` covers the same use case with the existing Option type. A dedicated `ifElse` combinator could be built on top:

```ts
function ifElse<TIn, TOut>(
  condition: Pipeable<TIn, boolean>,
  thenAction: Pipeable<void, TOut>,
  elseAction: Pipeable<void, TOut>,
): TypedAction<TIn, TOut> {
  return pipe(
    condition,
    asOption(),
    branch({ Some: thenAction, None: elseAction }),
  );
}
```

Whether a dedicated `Bool` tagged union (True/False) is still worth adding is an open question — `AsOption` works but the Some/None naming is semantically awkward for if/else branching.

## Void in JSON Schema

`z.void()` cannot be represented in JSON Schema, so handlers that return `Result<void, E>` (or any type containing void) fail at runtime when `createHandler` attempts to convert the Zod validator. The workaround is to use `null` instead of `void` in Result types at the handler boundary (e.g., `Result<null, string>`).

The proper fix would be for the schema conversion layer (`zodToCheckedJsonSchema`) to transparently map `z.void()` to `z.null()` — void and null are semantically equivalent in the JSON wire format (both serialize to `null`). This would let handler authors write `Result<void, string>` naturally without thinking about JSON Schema limitations.
