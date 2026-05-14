# Concurrency-Limited Map

## Motivation

`.iterate().map(handler).collect()` dispatches all elements to the engine simultaneously via `ForEach`. For large iterators hitting external services (LLM calls, API requests, file system operations), this creates unbounded concurrency — potentially overwhelming rate limits, exhausting file descriptors, or causing OOM from too many in-flight responses.

Currently the only way to process sequentially is `.fold()`, which gives parallelism=1. There's no way to say "process at most N items concurrently."

## Current state

The `ForEach` AST node (`crates/barnum_engine/src/advance.rs`) iterates all elements and dispatches each one immediately:

```rust
FlatAction::ForEach { body } => {
    let elements = match value { Value::Array(elements) => elements, ... };
    let frame_id = workflow_state.insert_frame(Frame {
        parent,
        kind: FrameKind::ForEach { results: vec![None; elements.len()] },
    });
    for (i, element) in elements.into_iter().enumerate() {
        advance(workflow_state, body, element, ...)
    }
}
```

No concurrency control exists at this layer.

## Proposed design

### Option A: Engine-level `ForEachLimited` node

Add a new AST variant:

```rust
ForEachLimited { body: Box<FlatAction>, concurrency: usize }
```

The engine maintains a window of at most `concurrency` in-flight elements. When one completes, the next is dispatched. Results are collected in order (same as ForEach).

**TS surface:**

```typescript
// Postfix on Iterator:
listFiles.iterate().map(processFile, { concurrency: 5 }).collect()

// Or as a separate method:
listFiles.iterate().mapLimited(5, processFile).collect()
```

### Option B: Chunked composition (no engine changes)

Add `.chunked(n)` to Iterator, then compose:

```typescript
// Process in batches of 5:
listFiles.iterate().chunked(5).map(all(...items)).flatten().collect()
```

This requires `.chunked(n): Iterator<T> -> Iterator<Iterator<T>>` (or `Iterator<T[]>`) and a way to `all()` within each chunk. Less ergonomic but avoids engine changes.

### Option C: Semaphore-style postfix modifier

A method that wraps the subsequent `.map()` with a concurrency gate:

```typescript
listFiles.iterate().limitConcurrency(5).map(processFile).collect()
```

This would modify how the following `ForEach` is dispatched. Requires either a new AST node or a transform that rewrites the ForEach into a chunked loop.

## Recommendation

Option A is the cleanest UX. The engine already tracks in-flight work per ForEach frame — extending it with a concurrency cap is natural. The postfix form `.mapLimited(n, action)` avoids ambiguity about what `.limitConcurrency()` modifies.

## Open questions

1. Should the concurrency parameter be a constant or a `Pipeable<void, number>` (dynamic)?
2. Should backpressure be FIFO (dispatch in order) or allow out-of-order dispatch with ordered collection?
3. Does this interact with `withTimeout` — if a slot times out, does it free the concurrency slot?
4. Naming: `mapLimited`, `mapWithConcurrency`, `mapN`, `mapBounded`?
