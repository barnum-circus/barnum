# Lazy Iterators

## Current model: eager

Barnum's Iterator is backed by `T[]`. Every operation materializes the full array:

```
[1, 2, 3].iterate().map(f).filter(pred).collect()
```

This actually runs:
1. Wrap array → `Iterator` (tag)
2. Unwrap → `T[]`, `forEach(f)` on the full array → `U[]`, re-wrap → `Iterator` 
3. Unwrap → `U[]`, `forEach(pred + AsOption + ...)` on full array → filter → re-wrap → `Iterator`
4. Unwrap → `T[]` (getField)

Three intermediate arrays. Three unwrap/re-wrap cycles. Each `.map()` and `.filter()` is its own `ForEach` AST node — the Rust engine schedules each as a complete parallel dispatch before moving to the next.

For small arrays and handler-heavy steps (LLM calls), this is fine — the parallelism within each `.map()` is the bottleneck, not the intermediate arrays. But for large arrays or pure data transforms chained together, it's wasteful.

## Design: LazyIterator AST node + Realize terminal

Two new AST node kinds:

### `LazyIterator` — accumulate a pipeline

```typescript
type LazyStep =
  | { kind: "Map"; action: Action }
  | { kind: "Filter"; predicate: Action }
  | { kind: "FlatMap"; action: Action }
  | { kind: "Take"; n: number }
  | { kind: "Skip"; n: number }
  | { kind: "Enumerate" }
  | { kind: "Reverse" }
  // etc.

interface LazyIteratorNode {
  kind: "LazyIterator";
  source: Action;     // produces T[]
  steps: LazyStep[];  // pipeline of transformations
}
```

Calling `.map(f)` on a lazy iterator doesn't execute anything — it appends `{ kind: "Map", action: f }` to the steps array. The AST grows, the engine hasn't touched any data yet.

```typescript
// What the user writes:
array.iterate().lazy().map(f).filter(pred).map(g)

// AST produced:
{
  kind: "LazyIterator",
  source: /* action producing T[] */,
  steps: [
    { kind: "Map", action: f },
    { kind: "Filter", predicate: pred },
    { kind: "Map", action: g },
  ]
}
```

### `Realize` — execute the lazy pipeline

A terminal operation wraps the `LazyIterator` in a `Realize` node that tells the engine how to materialize:

```typescript
type RealizeMode =
  | { kind: "Collect" }                    // → T[]
  | { kind: "First" }                      // → Option<T>  (stop after first)
  | { kind: "Last" }                       // → Option<T>
  | { kind: "Count" }                      // → number
  | { kind: "Find"; predicate: Action }    // → Option<T>  (stop after first match)
  | { kind: "Any"; predicate: Action }     // → boolean    (stop after first match)
  | { kind: "All"; predicate: Action }     // → boolean    (stop after first non-match)
  | { kind: "Fold"; init: Action; body: Action }  // → U  (sequential accumulation)
  | { kind: "Nth"; n: number }             // → Option<T>  (stop after index)
  // etc.

interface RealizeNode {
  kind: "Realize";
  iterator: LazyIteratorNode;
  mode: RealizeMode;
}
```

When the user calls `.collect()` on a lazy iterator:

```typescript
// User:
array.iterate().lazy().map(f).filter(pred).collect()

// AST:
{
  kind: "Realize",
  iterator: {
    kind: "LazyIterator",
    source: /* ... */,
    steps: [
      { kind: "Map", action: f },
      { kind: "Filter", predicate: pred },
    ]
  },
  mode: { kind: "Collect" }
}
```

When the user calls `.first()`:

```typescript
// User:
array.iterate().lazy().map(f).filter(pred).first()

// AST:
{
  kind: "Realize",
  iterator: { /* same LazyIterator */ },
  mode: { kind: "First" }
}
```

## Rust engine execution

The engine processes a `Realize` node as a single pass over the source data:

```
1. Evaluate `source` action → get T[]
2. For each element in T[]:
   a. Run through steps sequentially (map → filter → map → ...)
   b. If element is filtered out, skip to next
   c. If element passes all steps, feed to the RealizeMode accumulator
   d. If RealizeMode is short-circuiting (First, Find, Any, Nth), check termination condition
3. Return accumulated result
```

### Short-circuiting

This is where lazy shines. Eager `.find(pred)` runs `filter(pred)` on the **entire** array, then takes the first element. Lazy `.find(pred)` processes elements one at a time and stops as soon as it finds a match.

Short-circuiting terminals: `First`, `Last` (needs full scan but no intermediate arrays), `Find`, `Any`, `All`, `Nth`, `Fold` (sequential but one-pass).

Non-short-circuiting terminals: `Collect` (needs all elements), `Count` (needs full scan).

### Step execution detail

Each step in the pipeline transforms or filters one element at a time:

- **Map**: Run the inner action on the element. Replace element with result.
- **Filter**: Run predicate on element. If falsy, skip element (don't pass to subsequent steps).
- **FlatMap**: Run inner action on element. Result may be 0, 1, or many elements. Each feeds independently into subsequent steps.
- **Take(n)**: Pass first n elements, then stop (short-circuit the source iteration).
- **Skip(n)**: Drop first n elements, pass the rest.
- **Enumerate**: Attach index, pass `[index, element]`.
- **Reverse**: **Cannot be lazy** — needs all elements first. Buffers, then reverses. (Or: engine detects this and falls back to eager for the prefix up to this step, then continues lazily.)

### FlatMap complication

FlatMap is the tricky one. A single input element can produce multiple output elements, each of which must independently flow through the remaining steps. The engine needs to handle 1-to-many expansion mid-pipeline.

Approach: when a FlatMap step produces N elements, the engine iterates over those N elements and feeds each through the remaining steps. This is a recursive/nested loop but still single-pass over the source.

### Steps containing async actions

Map, Filter, and FlatMap steps contain inner actions that may be handler invocations (async, spawns a subprocess). The engine must await each step's result before proceeding to the next step for that element. This is inherently sequential per-element — the pipeline processes one element at a time.

This is the fundamental tradeoff vs eager: eager `.map(handler)` dispatches ALL elements to the handler in parallel via `ForEach`. Lazy `.map(handler)` processes elements one at a time.

## When lazy vs eager matters

| Scenario | Eager wins | Lazy wins |
|----------|-----------|-----------|
| `.map(llmCall)` on 10 items | Yes — 10 concurrent LLM calls | No — 10 sequential calls |
| `.map(transform).filter(pred).map(transform2)` on 10K items | No — 3 intermediate arrays | Yes — single pass, no intermediate arrays |
| `.filter(pred).first()` on 10K items | No — filters entire array, then takes first | Yes — stops at first match |
| `.take(5)` on 10K items | No — materializes then slices | Yes — stops after 5 |
| `.map(handler).filter(pred)` on 10 items | Maybe — parallel map is fast, extra array is cheap | Maybe — single pass but sequential |

## API design

### Entry point: `.lazy()`

```typescript
array.iterate().lazy()  // → LazyIterator<T>
```

`.lazy()` is a postfix method on `Iterator<T>` that switches to lazy mode. All subsequent method calls (`.map()`, `.filter()`, etc.) accumulate steps instead of executing.

### Terminal operations (trigger Realize)

```typescript
.collect()       // → T[]
.first()         // → Option<T>
.last()          // → Option<T>
.find(pred)      // → Option<T>
.nth(n)          // → Option<T>
.count()         // → number
.any(pred)       // → boolean
.all(pred)       // → boolean
.fold(init, f)   // → U
```

Each of these wraps the accumulated `LazyIterator` in a `Realize` node and returns a non-lazy `Action`.

### Returning to eager

`.collect()` returns `T[]`. You can `.iterate()` again to enter eager mode for a parallel `.map()`:

```typescript
// Lazy filter + take, then parallel map
array.iterate().lazy()
  .filter(pred)
  .take(10)
  .collect()           // realize: T[]
  .iterate()           // back to eager Iterator
  .map(expensiveHandler)  // parallel ForEach
  .collect()
```

### Or: `.realize()` as explicit materialization

Instead of overloading `.collect()`, a dedicated `.realize()` terminal that returns `Iterator<T>` (eager):

```typescript
array.iterate().lazy()
  .filter(pred)
  .take(10)
  .realize()           // → Iterator<T> (eager, backed by materialized array)
  .map(expensiveHandler)  // parallel ForEach
  .collect()
```

This keeps `.collect()` meaning "exit Iterator, give me T[]" in both lazy and eager contexts, while `.realize()` means "execute the lazy pipeline, give me an eager Iterator."

## Type system implications

`LazyIterator<T>` is a distinct type from `Iterator<T>`. They share method names but:

- `Iterator<T>.map(f)` returns `Iterator<U>` (eagerly evaluated)
- `LazyIterator<T>.map(f)` returns `LazyIterator<U>` (step appended)
- `LazyIterator<T>.collect()` returns `T[]` (triggers Realize)
- `LazyIterator<T>.realize()` returns `Iterator<T>` (triggers Realize, stays in Iterator)

The TypeScript postfix methods need to distinguish the two. Either:
1. **Separate method sets** — `LazyIterator` has its own `TypedAction` with lazy-specific methods
2. **Dispatch** — `.map()` checks if `this` is lazy and either appends a step or executes eagerly

Option 1 is cleaner for types. Option 2 is what we do for Option/Result/Iterator dispatch already.

## Interaction with scan

`scan(init, f)` is already sequential. In the eager model, `scan` processes elements one-at-a-time with an accumulator. In the lazy model, `scan` is just another step — but it's inherently sequential anyway.

Key question: does scan belong as a lazy step, or as a `RealizeMode`? 

- **As a step**: `.lazy().map(f).scan(init, g).filter(pred).collect()` — scan is mid-pipeline, subsequent steps process scan's outputs
- **As a terminal**: `.lazy().map(f).fold(init, g)` — fold is `scan.last()`, a terminal

Both are useful. `scan` as a step allows chaining after it. `fold` as a terminal is the common case. Scan-as-step is strictly more general.

## Interaction with parallel dispatch

The key architectural question: can a lazy pipeline contain a "parallel" step?

```typescript
array.iterate().lazy()
  .filter(pred)       // sequential
  .parallelMap(f)     // parallel dispatch within this step?
  .filter(pred2)      // sequential
  .collect()
```

This would mean the engine:
1. Sequentially filters source elements
2. Batches elements that pass the filter
3. Dispatches the batch to `f` in parallel (like eager `ForEach`)
4. Sequentially filters the results

This is a hybrid model — mostly lazy/sequential, with explicit parallel steps. It would need:
- A `ParallelMap` lazy step kind
- The engine to buffer elements, dispatch in parallel, then continue sequentially

This is significantly more complex but would give the best of both worlds. Probably not worth it for v1 — start with pure sequential lazy, let users `.realize()` back to eager for parallel steps.

## Open questions

1. **Is `.lazy()` the right entry point, or should lazy be the default?** If most real workloads are handler-heavy (where eager parallelism wins), lazy should be opt-in. If most are data transforms, lazy should be default.

2. **How does `.realize()` interact with type-level Iterator dispatch?** The `branchFamily` dispatch routes based on `kind` prefix. If `LazyIterator` has a different kind tag, postfix methods like `.map()` would dispatch differently. Do we want that, or should lazy/eager be invisible at the dispatch level?

3. **Should `Realize` be a separate AST node, or should LazyIterator carry its terminal?** Two options:
   - `Realize { iterator: LazyIterator, mode: ... }` — the terminal is a wrapper
   - `LazyIterator { source, steps, terminal: ... }` — the terminal is part of the node
   
   The wrapper approach (Realize) is more compositional — a LazyIterator without Realize is a "pending" pipeline that can be further extended.

4. **Can we optimize the eager model instead?** The Rust engine could detect chains like `ForEach(f) → ForEach(filter_expand) → GetField("value")` and fuse them into a single pass, without any changes to the TypeScript API. This is Option A from the old doc — engine-level optimization of the existing eager AST. Less general but zero API change.

5. **Naming**: `realize()`, `materialize()`, `evaluate()`, `run()`, `execute()`? The Rust ecosystem uses `collect()` as the terminal, but we already use `collect()` for "unwrap Iterator to array." If lazy `.collect()` means "realize + unwrap," that's consistent. Then maybe we don't need a separate `realize()` — `.collect()` is the terminal, and `.lazy().filter(pred).collect().iterate()` is the way back to eager.
