# Iterator Methods — Full Catalog

Reference for all Iterator methods we want, beyond the Phase 1 core (`.map`, `.flatMap`, `.filter`, `.collect`). Organized by category. Rust equivalents noted for each.

All barnum Iterators are **eager** (backed by `T[]`), not lazy. This means:
- No short-circuiting (`.find()` scans the whole array, then picks the first)
- `.take(n)` / `.skip(n)` are slice operations, not lazy truncation
- No infinite iterators (no `.cycle()`)

---

## Transforming

| Method | Rust | Signature | Notes |
|--------|------|-----------|-------|
| `.filterMap(f)` | `filter_map` | `Iterator<T> → Iterator<U>` | `f: T → Option<U>`. Keep Some values, drop None. Combines filter + map. |
| `.flatten()` | `flatten` | `Iterator<IntoIterator<T>> → Iterator<T>` | Flattens one level of nesting. Each element is normalized via IntoIterator (same as `.flatMap`'s inner normalization). |
| `.enumerate()` | `enumerate` | `Iterator<T> → Iterator<[number, T]>` | Pairs each element with its index. |
| `.scan(init, f)` | `scan` | `Iterator<T> → Iterator<U>` | Stateful map. `f: (state, T) → Option<U>`. State threads through. None stops emission for that element. Needs design — state threading in AST. |
| `.inspect(f)` | `inspect` | `Iterator<T> → Iterator<T>` | Side-effect on each element, passes through unchanged. Equivalent to `.map(tap(f))`. |
| `.intersperse(sep)` | `intersperse` | `Iterator<T> → Iterator<T>` | Inserts separator between elements. |

---

## Limiting & Slicing

| Method | Rust | Signature | Notes |
|--------|------|-----------|-------|
| `.take(n)` | `take` | `Iterator<T> → Iterator<T>` | First n elements. New builtin. |
| `.skip(n)` | `skip` | `Iterator<T> → Iterator<T>` | Drop first n elements. New builtin. |
| `.takeWhile(pred)` | `take_while` | `Iterator<T> → Iterator<T>` | Elements from start while pred is true. New builtin. |
| `.skipWhile(pred)` | `skip_while` | `Iterator<T> → Iterator<T>` | Drop elements from start while pred is true. New builtin. |
| `.stepBy(n)` | `step_by` | `Iterator<T> → Iterator<T>` | Every nth element. New builtin. |
| `.chunks(n)` | slice `chunks` | `Iterator<T> → Iterator<T[]>` | Groups into fixed-size sub-arrays. Last chunk may be smaller. New builtin. |
| `.windows(n)` | slice `windows` | `Iterator<T> → Iterator<T[]>` | Overlapping windows of size n. New builtin. |

---

## Searching & Exiting to Option

All of these exit Iterator and produce `Option<T>`.

| Method | Rust | Signature | Notes |
|--------|------|-----------|-------|
| `.first()` | `next` | `Iterator<T> → Option<T>` | First element. Equivalent to array `.splitFirst()` — see note below. |
| `.last()` | `last` | `Iterator<T> → Option<T>` | Last element. |
| `.find(pred)` | `find` | `Iterator<T> → Option<T>` | First element matching predicate. Not short-circuiting. |
| `.findMap(f)` | `find_map` | `Iterator<T> → Option<U>` | `f: T → Option<U>`. First Some result. Same as `.filterMap(f).first()`. |
| `.nth(n)` | `nth` | `Iterator<T> → Option<T>` | Element at index n. |
| `.position(pred)` | `position` | `Iterator<T> → Option<number>` | Index of first match. |

### `.first()` and `.splitFirst()`

`.splitFirst()` is the existing array builtin: `T[] → Option<[T, T[]]>`. It returns both the first element and the remainder.

Iterator's `.first()` only returns the first element (discards the rest). It's the simpler form — equivalent to Rust's `.next()`.

In `babysit-prs`, the current loop uses `splitFirst` to peel off one PR at a time. With Iterator, this becomes `.iterate().map(process).collect()` — no manual peeling needed. `.splitFirst()` remains useful for recursive array processing outside of Iterator (e.g., in `loop` + `branch` patterns).

---

## Aggregation

These exit Iterator and produce a scalar.

| Method | Rust | Signature | Notes |
|--------|------|-----------|-------|
| `.count()` | `count` | `Iterator<T> → number` | Number of elements. Needs `ArrayLength` builtin. |
| `.any(pred)` | `any` | `Iterator<T> → boolean` | True if any element matches. `.find(pred).isSome()`. |
| `.all(pred)` | `all` | `Iterator<T> → boolean` | True if all elements match. Name collision with `all()` combinator — needs resolution. |
| `.sum()` | `sum` | `Iterator<number> → number` | Sum of elements. Needs `Sum` builtin. |
| `.product()` | `product` | `Iterator<number> → number` | Product of elements. Needs `Product` builtin. |
| `.min()` | `min` | `Iterator<number> → Option<number>` | Minimum element. Needs builtin. |
| `.max()` | `max` | `Iterator<number> → Option<number>` | Maximum element. Needs builtin. |
| `.minBy(f)` | `min_by_key` | `Iterator<T> → Option<T>` | Min by key function. Needs builtin. |
| `.maxBy(f)` | `max_by_key` | `Iterator<T> → Option<T>` | Max by key function. Needs builtin. |

---

## Collecting — typed destinations

These exit Iterator into a specific type.

| Method | Rust | Signature | Notes |
|--------|------|-----------|-------|
| `.collect()` | `collect::<Vec>` | `Iterator<T> → T[]` | **Phase 1.** Default collect to array. |
| `.collectResult()` | `collect::<Result<Vec,E>>` | `Iterator<Result<T,E>> → Result<T[],E>` | All-or-nothing. First Err short-circuits. Needs `CollectResult` builtin. |
| `.collectOption()` | `collect::<Option<Vec>>` | `Iterator<Option<T>> → Option<T[]>` | All-or-nothing. First None short-circuits. Needs `CollectOption` builtin. |
| `.partition(pred)` | `partition` | `Iterator<T> → [T[], T[]]` | Split into two arrays by predicate. Needs builtin. |
| `.unzip()` | `unzip` | `Iterator<[A, B]> → [A[], B[]]` | Unzip pairs. Needs builtin. |

---

## Combining Iterators

| Method | Rust | Signature | Notes |
|--------|------|-----------|-------|
| `.chain(other)` | `chain` | `Iterator<T> → Iterator<T>` | Concatenate two iterators. No naming collision — barnum's `chain()` is internal, users see `.then()`. |
| `.zip(other)` | `zip` | `Iterator<T>, Iterator<U> → Iterator<[T, U]>` | Pair elements from two iterators. Truncates to shorter. Needs design for how `other` is provided. |

---

## Reordering

| Method | Rust | Signature | Notes |
|--------|------|-----------|-------|
| `.reverse()` | `rev` | `Iterator<T> → Iterator<T>` | Reverse element order. Trivial on eager arrays. Needs `Reverse` builtin. |
| `.sortBy(f)` | — | `Iterator<T> → Iterator<T>` | Sort by key function. Not on Rust Iterator (Rust iterators are lazy). Useful on our eager arrays. Needs `SortBy` builtin. |

---

## Folding & Sequential Execution

| Method | Rust | Signature | Notes |
|--------|------|-----------|-------|
| `.fold(init, f)` | `fold` | `Iterator<T> → U` | Accumulate with initial value. `f: (acc, T) → U`. Needs design — how to express accumulator threading in AST. |
| `.reduce(f)` | `reduce` | `Iterator<T> → Option<T>` | Fold without initial value. First element is initial accumulator. Returns None on empty. |
| `.forEachSync(f)` | `for_each` | `Iterator<T> → Iterator<U>` | Sequential (non-parallel) element processing. Wrapper around reduce that ensures each item is fully processed before the next starts. `forEach` dispatches all elements in parallel — `forEachSync` is the serial alternative. |

Fold/reduce need significant design work — accumulator state threading doesn't have an obvious AST representation yet. `.forEachSync` is a natural first consumer of that mechanism: it's reduce where the accumulator is the output array being built up one element at a time.

---

## Array postfix methods (not Iterator)

These stay on arrays directly, not on Iterator. They're structural operations on the array itself.

| Method | Signature | Notes |
|--------|-----------|-------|
| `.splitFirst()` | `T[] → Option<[T, T[]]>` | **Exists today.** First element + remainder. Used in `loop` + `branch` patterns for recursive array processing. |
| `.splitLast()` | `T[] → Option<[T[], T]>` | **Exists today.** Remainder + last element. |
| `.splitFirstN(n)` | `T[] → [T[], T[]]` | First n elements + remainder. Needs `SplitFirstN` builtin. |
| `.splitLastN(n)` | `T[] → [T[], T[]]` | Remainder + last n elements. Needs `SplitLastN` builtin. |

---

## Not applicable to barnum

These Rust Iterator methods don't translate to barnum's eager model:

| Method | Why not |
|--------|---------|
| `cycle` | Infinite iterator — no eager equivalent |
| `fuse` | Already eager — no "after None" state |
| `peekable` | No lazy consumption model |
| `by_ref` | Rust borrowing concept |
| `cloned` / `copied` | Rust ownership concept |
| `size_hint` | No lazy iteration |
| `try_fold` / `try_reduce` / `try_find` | Use `.collectResult()` or `.find()` + Result methods instead |
| Comparison methods (`eq`, `lt`, `cmp`, etc.) | Array comparison is a different problem — not iterator-shaped in barnum |
| `is_sorted` / `is_partitioned` | Niche predicates — implement when needed |

---

## Priority

**High — needed for demos:**
- `.filterMap(f)` — very common pattern (filter + transform in one step)
- `.first()` / `.last()` — exit Iterator to Option
- `.find(pred)` — searching
- `.collectResult()` / `.collectOption()` — typed collect for fallible pipelines
- `.enumerate()` — index tracking
- `.fold(init, f)` / `.reduce(f)` — accumulation (needs AST design for state threading)
- `.forEachSync(f)` — serial element processing (built on reduce)

**Medium — useful but not blocking:**
- `.flatten()` — nested IntoIterator flattening
- `.count()` — length
- `.any(pred)` / `.all(pred)` — boolean predicates
- `.take(n)` / `.skip(n)` — slicing
- `.chain(other)` — concatenation
- `.reverse()` — reordering
- `.nth(n)` — indexed access
- `.partition(pred)` — splitting

**Low — add when a demo or user needs them:**
- `.zip(other)` — pairing
- `.unzip()` — unpairing
- `.sum()` / `.product()` — numeric aggregation
- `.min()` / `.max()` / `.minBy()` / `.maxBy()` — extrema
- `.sortBy(f)` — sorting
- `.scan(init, f)` — stateful transform
- `.inspect(f)` — debugging (`.map(tap(f))` works today)
- `.intersperse(sep)` — separator insertion
- `.takeWhile(pred)` / `.skipWhile(pred)` — conditional slicing
- `.stepBy(n)` — strided access
- `.chunks(n)` / `.windows(n)` — grouping
- `.position(pred)` — index searching
- `.findMap(f)` — `.filterMap(f).first()`
