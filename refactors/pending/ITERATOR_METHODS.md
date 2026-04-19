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
| `.scan(init, f)` | `scan` | `Iterator<T> → Iterator<U>` | **Primitive.** Stateful map: `f: (acc, T) → U`, emits each intermediate accumulator. See Folding section. |

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

`.splitFirst()` is essential for sequential processing patterns — `loop` + `splitFirst` + `branch` processes one element at a time serially, while `.iterate().map(f)` dispatches all elements in parallel via `forEach`. Use `splitFirst` when ordering matters (e.g., `identify-and-address-refactors` implements one refactor at a time). Use `.iterate().map()` when parallel dispatch is fine.

**Example: sequential processing with `splitFirst` + `loop`**

```ts
// Process PRs one at a time, recurring with the remainder
loop<void, void>((recur, done) =>
  prs
    .splitFirst()                              // Option<[number, number[]]>
    .branch({
      Some: bindInput<[number, number[]]>(([pr, rest]) =>
        pr
          .then(checkPR)
          .branch({
            ChecksFailed: fixIssues.drop().then(rest).then(recur),
            ChecksPassed: landPR.drop().then(rest).then(recur),
            Landed: drop.then(rest).then(recur),
          }),
      ),
      None: done,                              // all PRs processed
    }),
)
```

This processes one PR at a time. Each iteration peels off the first PR, processes it, then recurs with the remainder. Compare with `.iterate().map(process).collect()` which dispatches all PRs in parallel.

---

## Aggregation

These exit Iterator and produce a scalar.

| Method | Rust | Signature | Notes |
|--------|------|-----------|-------|
| `.count()` | `count` | `Iterator<T> → number` | Number of elements. Needs `ArrayLength` builtin. |
| `.any(pred)` | `any` | `Iterator<T> → boolean` | True if any element matches. `.find(pred).isSome()`. |
| `.all(pred)` | `all` | `Iterator<T> → boolean` | True if all elements match. Name collision with `all()` combinator — needs resolution. |

---

## Collecting — typed destinations

These exit Iterator into a specific type.

| Method | Rust | Signature | Notes |
|--------|------|-----------|-------|
| `.collect()` | `collect::<Vec>` | `Iterator<T> → T[]` | **Phase 1.** Default collect to array. |
| `.collectResult()` | `collect::<Result<Vec,E>>` | `Iterator<Result<T,E>> → Result<T[],E>` | All-or-nothing. First Err short-circuits. Needs `CollectResult` builtin. |
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
| `.scan(init, f)` | `scan` | `Iterator<T> → Iterator<U>` | **Primitive.** Stateful map: `f: (acc, T) → U`, emits each intermediate accumulator. Needs AST design for state threading. |
| `.fold(init, f)` | `fold` | `Iterator<T> → U` | `.scan(init, f).last()`. Not a primitive. |
| `.reduce(f)` | `reduce` | `Iterator<T> → Option<T>` | Fold without initial value. First element is initial accumulator. `.splitFirst()` + `.scan()` + `.last()`. |
| `.forEachSync(f)` | `for_each` | `Iterator<T> → Iterator<U>` | Sequential (non-parallel) element processing. Scan where the accumulator is the growing output array. `forEach` dispatches in parallel — `forEachSync` is the serial alternative. |

Scan is the primitive — fold, reduce, and forEachSync all compose from it. The core design work is accumulator state threading in the AST.

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
- `.collectResult()` — typed collect for fallible pipelines
- `.enumerate()` — index tracking
- `.scan(init, f)` — **primitive** for accumulator state threading (fold, reduce, forEachSync all derive from this)
- `.fold(init, f)` / `.reduce(f)` / `.forEachSync(f)` — derived from scan

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
- `.sortBy(f)` — sorting
- `.takeWhile(pred)` / `.skipWhile(pred)` — conditional slicing
- `.stepBy(n)` — strided access
- `.chunks(n)` / `.windows(n)` — grouping
- `.position(pred)` — index searching
- `.findMap(f)` — `.filterMap(f).first()`
