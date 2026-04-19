# Lazy Iterators — Speculation

## Current model: eager

Barnum's Iterator is backed by `T[]`. Every operation materializes the full array:

```
[1, 2, 3].iterate().map(f).filter(pred).collect()
```

This actually runs:
1. Wrap array → `Iterator`
2. `forEach(f)` on the full array → new array
3. `CollectWhere` on the full array → new array
4. Unwrap → final array

Three intermediate arrays allocated. For large arrays or expensive transforms, this is wasteful.

## What lazy would look like

Instead of executing each step immediately, lazy Iterator builds up a chain of transformations and only materializes when a terminal operation (`.collect()`, `.first()`, etc.) is called.

### Option A: Fused AST nodes

The Rust engine fuses adjacent Iterator operations into a single pass at compile time (AST optimization). No runtime laziness — the scheduler recognizes patterns like `ForEach(f) → CollectWhere(pred)` and emits a single fused `FilterMap` pass.

**Pros:**
- No new runtime concepts — it's an optimization pass on the existing eager model
- The TypeScript runtime stays simple (eager)
- Rust engine gets fast paths without changing the scheduling model

**Cons:**
- Only handles patterns the optimizer recognizes — not general-purpose laziness
- New AST patterns need new fusion rules

### Option B: Lazy Iterator AST node

A new AST node kind: `LazyIterator` that accumulates a pipeline of operations and defers execution.

```
{ kind: "LazyIterator", source: Action, steps: LazyStep[] }
```

Where `LazyStep` is:
```
| { kind: "Map", action: Action }
| { kind: "Filter", predicate: Action }
| { kind: "FlatMap", action: Action }
| { kind: "Take", n: number }
| ...
```

The Rust engine processes `LazyIterator` in a single pass: iterate the source, apply each step to each element, emit results. No intermediate arrays.

**Pros:**
- General-purpose — any combination of operations fuses automatically
- Single-pass for chains like `.map(f).filter(pred).map(g)` — each element goes through all three steps before the next element starts
- Natural fit for short-circuiting (`.first()`, `.find()`) — stop iterating when you have the answer

**Cons:**
- New scheduling concept — the Rust engine needs a `LazyIterator` executor
- Step actions may themselves be async (handler calls) — the executor needs to handle that
- TypeScript runtime needs a parallel implementation
- More complex AST

### Option C: Compile to loop + splitFirst

The TypeScript layer compiles a chain like `.iterate().map(f).filter(pred).collect()` into a `loop` + `splitFirst` pattern under the hood:

```ts
// What the user writes:
array.iterate().map(f).filter(pred).collect()

// What the AST becomes:
loop((recur, done) =>
  arr.splitFirst().branch({
    Some: bindInput(([head, tail]) =>
      head.then(f).then(pred_check).branch({
        Keep: /* append to accumulator, recur with tail */,
        Skip: /* recur with tail */,
      }),
    ),
    None: done, // return accumulated results
  })
)
```

**Pros:**
- No new runtime concepts — uses existing `loop`, `splitFirst`, `branch`
- Automatically single-pass and short-circuit capable
- Works today (once scan/fold provides accumulator threading)

**Cons:**
- Each element is processed sequentially (no parallelism within the map step)
- The AST is much larger / harder to read for debugging
- Relies on scan/fold for accumulator threading (not yet designed)

### Option D: Hybrid — eager by default, lazy opt-in

Keep the eager model as default. Add `.lazy()` as an explicit opt-in:

```ts
// Eager (default) — three passes, parallel forEach:
array.iterate().map(f).filter(pred).collect()

// Lazy (opt-in) — single pass, sequential:
array.iterate().lazy().map(f).filter(pred).collect()
```

`.lazy()` switches the Iterator into a mode where subsequent operations are accumulated rather than executed. `.collect()` (or any terminal) triggers the single-pass execution.

**Pros:**
- No breaking changes — eager remains default
- User explicitly chooses the tradeoff (parallel multi-pass vs sequential single-pass)
- Clear mental model

**Cons:**
- Two code paths for every Iterator method
- Type system needs to distinguish `Iterator<T>` vs `LazyIterator<T>`

## Key tension: parallelism vs fusion

The fundamental tradeoff:

- **Eager `.map(f)`** runs `forEach(f)` — the Rust engine dispatches all elements in parallel (each element's `f` can be a handler call that runs concurrently)
- **Lazy `.map(f)`** processes elements one at a time — no parallelism within a single map step, but multiple map/filter steps fuse into one pass

For handler-heavy workflows (LLM calls, API calls), eager parallelism is a massive win. For pure data transformation, fusion is a win. The right default depends on the use case.

## Open questions

1. **Is this even a problem yet?** Our arrays are typically small (a handful of PRs, a few files). The extra intermediate arrays are negligible. This might be premature.

2. **Can the Rust engine detect and fuse automatically (Option A) without user opt-in?** If so, lazy is an optimization, not a user-facing concept.

3. **How does laziness interact with `forEach`'s parallelism?** A lazy `.map(handler)` can't dispatch handler calls in parallel — it processes one element at a time. This is the same tradeoff as `forEachSync` vs `forEach`.

4. **Is `.lazy()` just `.forEachSync()` with extra steps?** If the main benefit is single-pass sequential processing, maybe `forEachSync` (built on `scan`) is sufficient and a full lazy Iterator model is overkill.
