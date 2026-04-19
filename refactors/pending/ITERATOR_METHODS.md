# Iterator Methods — Full Catalog

Reference for all Iterator methods. Organized by category. Rust equivalents noted for each.

All barnum Iterators are **eager** (backed by `T[]`), not lazy. This means:
- No short-circuiting (`.find()` scans the whole array, then picks the first)
- `.take(n)` / `.skip(n)` are slice operations, not lazy truncation
- No infinite iterators (no `.cycle()`)

---

## Currently implemented (Phase 1)

These are implemented in `libs/barnum/src/iterator.ts`.

### `Iterator.fromArray<T>()` — `T[] → Iterator<T>`

**Implementation:** `tag<"Iterator", IteratorDef<T>, "Iterator">("Iterator", "Iterator")`

Wraps an array in the Iterator tagged union. Runtime: `{ kind: "Iterator.Iterator", value: T[] }`. Uses the existing `tag` builtin to construct the tagged union.

### `Iterator.fromOption<T>()` — `Option<T> → Iterator<T>`

**Implementation:** `branch({ Some: wrapInArray → fromArray, None: constant([]) → fromArray })`

Dispatches on the Option variant. Some wraps the value in a single-element array, None produces an empty array. Both paths wrap in Iterator via `fromArray`.

### `Iterator.fromResult<T, E>()` — `Result<T, E> → Iterator<T>`

**Implementation:** `branch({ Ok: wrapInArray → fromArray, Err: constant([]) → fromArray })`

Same pattern as fromOption. Ok values produce a single-element Iterator, Err values produce an empty Iterator (error is discarded).

### `.iterate()` — postfix on Option/Result/Array

**Implementation** (in `ast.ts` `iterateMethod`): `chain(this, branchFamily({ Option: fromOption, Result: fromResult, Array: fromArray }))`

Uses `branchFamily` for two-level dispatch via `ExtractPrefix`. The Rust engine strips the namespace prefix (`rsplit_once('.')`) to route `Option.Some` → `Option` family → `fromOption` handler. For bare arrays (no `kind` field), `ExtractPrefix` produces `{ kind: "Array", value: input }`.

### `Iterator.collect<T>()` — `Iterator<T> → T[]`

**Implementation:** `getField("value")`

Trivial — the Iterator tagged union stores its elements in the `value` field. `getField` extracts it.

### `.collect()` — postfix on Option[]/Iterator

**Implementation** (in `ast.ts` `collectMethod`): `chain(this, branchFamily({ Array: Option.collect(), Iterator: Iterator.collect() }))`

Two-level dispatch: `Option<T>[]` (an array) routes to `Option.collect()` (the `CollectSome` builtin). `Iterator<T>` routes to `Iterator.collect()` (`getField("value")`).

### `Iterator.map<T, U>(f)` — `Iterator<T> → Iterator<U>`

**Implementation:** `getField("value") → forEach(f) → fromArray<U>()`

Unwraps the Iterator to get the backing array, applies `f` to each element in parallel via `ForEach`, then re-wraps as Iterator.

- `getField("value")`: extracts `T[]` from the tagged union
- `forEach(f)`: Rust `ForEach` AST node — runs `f` on each element concurrently, returns `U[]`
- `fromArray<U>()`: re-wraps as `Iterator<U>`

### `.map(f)` — postfix on Option/Result/Iterator

**Implementation** (in `ast.ts` `mapMethod`): `chain(this, branchFamily({ Option: Option.map(f), Result: Result.map(f), Iterator: Iterator.map(f) }))`

Three-family dispatch. Each family applies `f` to the "success" value.

### `Iterator.flatMap<T, U>(f)` — `Iterator<T> → Iterator<U>`

**Implementation:** `getField("value") → forEach(f → intoIteratorNormalize) → flatten() → fromArray<U>()`

The key primitive. `f` can return any IntoIterator type (Iterator, Option, Result, or array). The result is normalized to a flat array before re-wrapping.

- `getField("value")`: extracts `T[]`
- `forEach(chain(f, intoIteratorNormalize))`: applies `f` to each element, then normalizes the result. Produces `U[][]` (array of arrays).
- `flatten()`: Rust `Flatten` builtin — flattens `U[][]` → `U[]`
- `fromArray<U>()`: re-wraps as `Iterator<U>`

**`intoIteratorNormalize`** is a `branchFamily` that handles all four IntoIterator return types:
- `Iterator`: `branch({ Iterator: identity() })` — unwrap tagged union to array
- `Option`: `branch({ Some: wrapInArray(), None: constant([]) })` — Some → `[value]`, None → `[]`
- `Result`: `branch({ Ok: wrapInArray(), Err: constant([]) })` — Ok → `[value]`, Err → `[]`
- `Array`: `identity()` — already an array, pass through

### `.flatMap(f)` — postfix on Iterator

**Implementation** (in `ast.ts` `flatMapMethod`): `chain(this, Iterator.flatMap(f))`

Direct delegation. Unlike `.map()`, flatMap only exists on Iterator (not Option/Result — they have `andThen`).

### `Iterator.filter<T>(pred)` — `Iterator<T> → Iterator<T>`

**Implementation:** `Iterator.flatMap(bindInput(element → element.then(pred).asOption().branch({ Some: element.some(), None: Option.none() })))`

Filter is implemented as flatMap + AsOption. For each element:

1. `element.then(pred)` — apply the predicate, produces `boolean`
2. `.asOption()` — `AsOption` builtin: `true` → `Option.Some(null)`, `false` → `Option.None`
3. `.branch({ Some: element.some(), None: Option.none() })` — if Some, wrap original element as `Option.Some(element)`; if None, produce `Option.None`
4. flatMap's `intoIteratorNormalize` handles the Option: `Some(element)` → `[element]`, `None` → `[]`

`bindInput` gives the branch access to the original element value (before the predicate consumed it). Without `bindInput`, the element would be lost after the boolean predicate.

### `.filter(pred)` — postfix on Option/Iterator

**Implementation** (in `ast.ts` `filterMethod`): `chain(this, branchFamily({ Option: branch({Some: pred, None: Option.none()}), Iterator: Iterator.filter(pred) }))`

Two-family dispatch:
- **Option**: predicate returns `Option<T>` (Some to keep, None to discard)
- **Iterator**: predicate returns `boolean`, delegates to `Iterator.filter`

Note the different predicate types: Option.filter takes `T → Option<T>`, Iterator.filter takes `T → boolean`.

### `wrapInArray<T>()` — helper, `T → T[]`

**Implementation:** `all(identity())`

Uses the `All` AST node with a single `identity()` action. `All` runs all actions on the same input and collects results as an array. With one action (identity), this wraps the input in a single-element array.

---

## Not yet implemented

### Transforming

#### `.filterMap(f)` — `Iterator<T> → Iterator<U>`

**Rust equivalent:** `filter_map`

`f: T → Option<U>`. Keep Some values, drop None. Combines filter + map.

**Implementation:** `Iterator.flatMap(f)` — identical to flatMap since `intoIteratorNormalize` already handles Option returns. `filterMap` is just flatMap with the constraint that `f` returns `Option<U>`.

**Needs:** Nothing new — just a type-constrained alias for flatMap. TypeScript implementation:
```ts
filterMap<TIn, TOut>(
  action: Pipeable<TIn, OptionT<TOut>>,
): TypedAction<IteratorT<TIn>, IteratorT<TOut>> {
  return Iterator.flatMap<TIn, TOut>(action);
}
```

#### `.flatten()` — `Iterator<IntoIterator<T>> → Iterator<T>`

**Rust equivalent:** `flatten`

Flattens one level of nesting. Each element is normalized via IntoIterator.

**Implementation:** `Iterator.flatMap(identity())` — apply identity to each element, then normalize via intoIteratorNormalize. Each element (an IntoIterator) becomes an array, then all arrays are concatenated.

**Needs:** Nothing new. TypeScript:
```ts
flatten<TElement>(): TypedAction<IteratorT<...>, IteratorT<TElement>> {
  return Iterator.flatMap<..., TElement>(identity());
}
```

#### `.enumerate()` — `Iterator<T> → Iterator<[number, T]>`

**Rust equivalent:** `enumerate`

Pairs each element with its index.

**Implementation:** Needs a new `Enumerate` Rust builtin. Input: array. Output: array of `[index, element]` pairs. The builtin itself is trivial: `input.iter().enumerate().map(|(i, v)| [i, v]).collect()`.

**Needs:** New `Enumerate` builtin in Rust.

#### `.scan(init, f)` — `Iterator<T> → Iterator<U>`

**Rust equivalent:** `scan`

**Primitive.** Stateful map: `f: (acc, element) → [newAcc, output]`, emits each output. The accumulator threads through sequentially.

**Implementation:** Needs a new `Scan` AST node (not just a builtin). This is fundamentally sequential — each step depends on the previous accumulator. Cannot be expressed as `ForEach` (which is parallel). Needs a new AST variant in the Rust engine that processes elements one-at-a-time, threading state.

**Needs:**
- New `Scan` AST variant in `Action` enum (not a builtin — it composes an inner action)
- Rust scheduler support for sequential element-by-element execution with accumulator state
- TypeScript AST constructor

This is the most complex new primitive. fold, reduce, and forEachSync all derive from scan.

---

### Limiting & Slicing

#### `.take(n)` — `Iterator<T> → Iterator<T>`

**Rust equivalent:** `take`

First n elements.

**Implementation:** New `Take` builtin. `input[..n]`. Trivial array slice.

**Needs:** New `Take` builtin (`{ kind: "Take", n: number }`).

#### `.skip(n)` — `Iterator<T> → Iterator<T>`

**Rust equivalent:** `skip`

Drop first n elements.

**Implementation:** New `Skip` builtin. `input[n..]`. Trivial array slice.

**Needs:** New `Skip` builtin (`{ kind: "Skip", n: number }`).

#### `.takeWhile(pred)` — `Iterator<T> → Iterator<T>`

**Rust equivalent:** `take_while`

Elements from start while predicate is true.

**Implementation:** Needs a new `TakeWhile` AST node (not a builtin — it composes the predicate action). Processes elements sequentially, stopping at first false. Cannot be a simple builtin because the predicate is an arbitrary action.

**Needs:** New `TakeWhile` AST variant, or implement via `scan` once scan exists.

#### `.skipWhile(pred)` — `Iterator<T> → Iterator<T>`

**Rust equivalent:** `skip_while`

Drop elements from start while predicate is true.

**Implementation:** Same story as takeWhile — needs sequential processing with a predicate.

**Needs:** New `SkipWhile` AST variant, or implement via `scan`.

#### `.stepBy(n)` — `Iterator<T> → Iterator<T>`

**Rust equivalent:** `step_by`

Every nth element.

**Implementation:** New `StepBy` builtin. `input.iter().step_by(n).collect()`.

**Needs:** New `StepBy` builtin (`{ kind: "StepBy", n: number }`).

#### `.chunks(n)` — `Iterator<T> → Iterator<T[]>`

**Rust equivalent:** slice `chunks`

Groups into fixed-size sub-arrays. Last chunk may be smaller.

**Implementation:** New `Chunks` builtin. `input.chunks(n).map(|c| c.to_vec()).collect()`.

**Needs:** New `Chunks` builtin (`{ kind: "Chunks", n: number }`).

#### `.windows(n)` — `Iterator<T> → Iterator<T[]>`

**Rust equivalent:** slice `windows`

Overlapping windows of size n.

**Implementation:** New `Windows` builtin. `input.windows(n).map(|w| w.to_vec()).collect()`.

**Needs:** New `Windows` builtin (`{ kind: "Windows", n: number }`).

---

### Searching & Exiting to Option

All of these exit Iterator and produce `Option<T>`.

#### `.first()` — `Iterator<T> → Option<T>`

**Rust equivalent:** `next`

First element.

**Implementation:** `collect() → splitFirst() → Option.map(getIndex(0).unwrap())`

Uses existing `SplitFirst` builtin. Already exists as the standalone `first()` function in `option.ts` — just needs to be exposed as an Iterator method that chains `collect()` first.

**Needs:** Nothing new — compose existing builtins.

#### `.last()` — `Iterator<T> → Option<T>`

**Rust equivalent:** `last`

Last element.

**Implementation:** `collect() → splitLast() → Option.map(getIndex(1).unwrap())`

Uses existing `SplitLast` builtin. Already exists as `last()` in `option.ts`.

**Needs:** Nothing new.

#### `.find(pred)` — `Iterator<T> → Option<T>`

**Rust equivalent:** `find`

First element matching predicate. **Not short-circuiting** (scans whole array).

**Implementation:** `.filter(pred).first()`

**Needs:** Nothing new — compose filter + first.

#### `.findMap(f)` — `Iterator<T> → Option<U>`

**Rust equivalent:** `find_map`

`f: T → Option<U>`. First Some result.

**Implementation:** `.filterMap(f).first()` — which is `.flatMap(f).first()`.

**Needs:** Nothing new.

#### `.nth(n)` — `Iterator<T> → Option<T>`

**Rust equivalent:** `nth`

Element at index n.

**Implementation:** `collect() → getIndex(n)`. Uses existing `GetIndex` builtin which already returns `Option`.

**Needs:** Nothing new.

#### `.position(pred)` — `Iterator<T> → Option<number>`

**Rust equivalent:** `position`

Index of first match.

**Implementation:** `.enumerate().find(([_, elem]) => pred(elem)).map(([idx, _]) => idx)`. Depends on `enumerate` builtin existing.

**Needs:** `Enumerate` builtin.

### `.first()` and `.splitFirst()`

`.splitFirst()` is the existing array builtin: `T[] → Option<[T, T[]]>`. It returns both the first element and the remainder.

Iterator's `.first()` only returns the first element (discards the rest). It's the simpler form — equivalent to Rust's `.next()`.

`.splitFirst()` is essential for sequential processing patterns — `loop` + `splitFirst` + `branch` processes one element at a time serially, while `.iterate().map(f)` dispatches all elements in parallel via `forEach`. Use `splitFirst` when ordering matters. Use `.iterate().map()` when parallel dispatch is fine.

---

## Aggregation

These exit Iterator and produce a scalar.

#### `.count()` — `Iterator<T> → number`

**Rust equivalent:** `count`

Number of elements.

**Implementation:** New `ArrayLength` builtin. `input.len()`.

**Needs:** New `ArrayLength` builtin.

#### `.any(pred)` — `Iterator<T> → boolean`

**Rust equivalent:** `any`

True if any element matches.

**Implementation:** `.find(pred).isSome()`. Compose existing methods.

**Needs:** Nothing new (once `.find()` exists).

#### `.all(pred)` — `Iterator<T> → boolean`

**Rust equivalent:** `all`

True if all elements match.

**Implementation:** Negate predicate + `.any()` + negate result. Or: `.filter(notPred).count() == 0`. Or use a dedicated approach. Name collision with `all()` combinator — needs resolution (maybe `.every(pred)`).

**Needs:** Name resolution. Implementation depends on available primitives.

---

## Collecting — typed destinations

#### `.collect()` — `Iterator<T> → T[]`

**Rust equivalent:** `collect::<Vec>`

**Phase 1. Implemented.** Default collect to array. `getField("value")`.

#### `.collectResult()` — `Iterator<Result<T,E>> → Result<T[],E>`

**Rust equivalent:** `collect::<Result<Vec,E>>`

All-or-nothing. First Err short-circuits.

**Implementation:** New `CollectResult` builtin. Iterates over array of Result values: if all Ok, returns `Result.Ok(values)`. If any Err, returns the first `Result.Err(error)`.

**Needs:** New `CollectResult` builtin.

#### `.partition(pred)` — `Iterator<T> → [T[], T[]]`

**Rust equivalent:** `partition`

Split into two arrays by predicate.

**Implementation:** New `Partition` AST node (predicate is an action, not just data). Or implement via scan.

**Needs:** New AST node or scan primitive.

#### `.unzip()` — `Iterator<[A, B]> → [A[], B[]]`

**Rust equivalent:** `unzip`

Unzip pairs.

**Implementation:** New `Unzip` builtin. Pure data transformation.

**Needs:** New `Unzip` builtin.

---

## Combining Iterators

#### `.chain(other)` — `Iterator<T>, Iterator<T> → Iterator<T>`

**Rust equivalent:** `chain`

Concatenate two iterators.

**Implementation:** `all(this.collect(), other.collect()) → flatten() → fromArray()`. Or: new `Concat` builtin. No naming collision — barnum's `chain()` is internal, users see `.then()`.

**Needs:** Composable from existing builtins, or new `Concat` builtin for clarity.

#### `.zip(other)` — `Iterator<T>, Iterator<U> → Iterator<[T, U]>`

**Rust equivalent:** `zip`

Pair elements from two iterators. Truncates to shorter.

**Implementation:** New `Zip` builtin. Needs design for how `other` is provided (second argument to the method, or via `all` + `zip` standalone).

**Needs:** New `Zip` builtin, API design for multi-input.

---

## Reordering

#### `.reverse()` — `Iterator<T> → Iterator<T>`

**Rust equivalent:** `rev`

Reverse element order.

**Implementation:** New `Reverse` builtin. Trivial on eager arrays.

**Needs:** New `Reverse` builtin.

#### `.sortBy(f)` — `Iterator<T> → Iterator<T>`

Not in Rust (Rust iterators are lazy). Useful on our eager arrays.

**Implementation:** New `SortBy` AST node (sort key function is an action). Applies `f` to each element to get sort keys, then sorts by keys.

**Needs:** New `SortBy` AST node.

---

## Folding & Sequential Execution

#### `.scan(init, f)` — `Iterator<T> → Iterator<U>`

**Rust equivalent:** `scan`

**Primitive.** Stateful map: `f: (acc, element) → [newAcc, output]`, emits each output. Sequential — each step depends on previous accumulator.

**Implementation:** This is the fundamental sequential primitive. Needs a new `Scan` AST variant:
```rust
Scan(ScanAction),

pub struct ScanAction {
    /// Initial accumulator value (an action that produces the initial state)
    pub init: Box<Action>,
    /// Body action: receives [acc, element], returns [newAcc, output]
    pub body: Box<Action>,
}
```

The Rust scheduler processes elements one at a time: run body with `[acc, element]`, extract `[newAcc, output]`, thread `newAcc` to next iteration, collect all `output`s.

**Needs:** New `Scan` AST variant, scheduler support for sequential state threading, TypeScript constructor.

#### `.fold(init, f)` — `Iterator<T> → U`

**Rust equivalent:** `fold`

**Not a primitive.** `.scan(init, f).last().unwrap()` — runs scan, takes the last emitted value.

**Needs:** `scan` + `last`.

#### `.reduce(f)` — `Iterator<T> → Option<T>`

**Rust equivalent:** `reduce`

Fold without initial value. First element is initial accumulator.

**Not a primitive.** `.collect().splitFirst().andThen(([first, rest]) => rest.iterate().scan(first, f).last())`

**Needs:** `scan` + `splitFirst` + composition.

#### `.forEachSync(f)` — `Iterator<T> → Iterator<U>`

Sequential (non-parallel) element processing. Each element processed after the previous completes.

**Not a primitive.** Scan where the accumulator is the growing output array:
```
scan([], (acc, elem) => {
  output = f(elem)
  return [acc.concat([output]), output]
})
```

Or more directly: a scan that threads an output array.

**Needs:** `scan`.

---

## Array postfix methods (not Iterator)

These stay on arrays directly, not on Iterator. They're structural operations on the array itself.

| Method | Signature | Status | Notes |
|--------|-----------|--------|-------|
| `.splitFirst()` | `T[] → Option<[T, T[]]>` | **Exists** | `SplitFirst` builtin |
| `.splitLast()` | `T[] → Option<[T[], T]>` | **Exists** | `SplitLast` builtin |
| `.splitFirstN(n)` | `T[] → [T[], T[]]` | Needs builtin | First n elements + remainder |
| `.splitLastN(n)` | `T[] → [T[], T[]]` | Needs builtin | Remainder + last n elements |

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

## Implementation summary

### What exists today

| Builtin/AST node | Used by |
|-------------------|---------|
| `tag` | `fromArray` |
| `Branch` | `fromOption`, `fromResult`, `filter`, `intoIteratorNormalize` |
| `BranchFamily` (ExtractPrefix + Branch) | `.iterate()`, `.map()`, `.filter()`, `.collect()` postfix methods |
| `ForEach` | `map`, `flatMap` |
| `Flatten` | `flatMap` |
| `GetField` | `collect` |
| `All` + `Identity` | `wrapInArray` helper |
| `AsOption` | `filter` (bool → Option conversion) |
| `CollectSome` | `.collect()` on `Option<T>[]` |
| `SplitFirst` / `SplitLast` | `first()` / `last()` standalone functions |
| `GetIndex` | `first()` / `last()` |
| `Constant` | `fromOption`/`fromResult` empty array case, `intoIteratorNormalize` |
| `Drop` | `filter` None case |
| `Identity` | `intoIteratorNormalize` Array/Iterator passthrough |

### What needs new builtins (data-only, no inner action)

| Builtin | Methods it enables |
|---------|-------------------|
| `Enumerate` | `.enumerate()`, `.position()` |
| `ArrayLength` | `.count()` |
| `Take` | `.take(n)` |
| `Skip` | `.skip(n)` |
| `StepBy` | `.stepBy(n)` |
| `Chunks` | `.chunks(n)` |
| `Windows` | `.windows(n)` |
| `Reverse` | `.reverse()` |
| `CollectResult` | `.collectResult()` |
| `Unzip` | `.unzip()` |

### What needs new AST nodes (compose inner actions)

| AST node | Methods it enables |
|----------|-------------------|
| `Scan` | `.scan()`, `.fold()`, `.reduce()`, `.forEachSync()`, `.takeWhile()`, `.skipWhile()`, `.partition()` |
| `SortBy` | `.sortBy(f)` |
| `Zip` | `.zip(other)` |

### What composes from existing primitives (no new Rust code)

| Method | Composition |
|--------|-------------|
| `.filterMap(f)` | `flatMap(f)` (type alias) |
| `.flatten()` | `flatMap(identity())` |
| `.first()` | `collect → splitFirst → Option.map(getIndex(0).unwrap())` |
| `.last()` | `collect → splitLast → Option.map(getIndex(1).unwrap())` |
| `.find(pred)` | `filter(pred).first()` |
| `.findMap(f)` | `filterMap(f).first()` |
| `.nth(n)` | `collect → getIndex(n)` |
| `.any(pred)` | `find(pred).isSome()` |
| `.chain(other)` | `all(this.collect(), other.collect()) → flatten → fromArray` |

---

## Priority

**High — needed for demos:**
- `.filterMap(f)` — compose only, no new Rust code
- `.first()` / `.last()` — compose only
- `.find(pred)` — compose only
- `.enumerate()` — new `Enumerate` builtin (trivial)
- `.scan(init, f)` — **new AST node** (complex, but unlocks fold/reduce/forEachSync)
- `.fold(init, f)` / `.reduce(f)` / `.forEachSync(f)` — compose from scan
- `.collectResult()` — new `CollectResult` builtin

**Medium — useful but not blocking:**
- `.flatten()` — compose only
- `.count()` — new `ArrayLength` builtin (trivial)
- `.any(pred)` — compose only (once find exists)
- `.take(n)` / `.skip(n)` — new trivial builtins
- `.chain(other)` — composable
- `.reverse()` — new trivial builtin
- `.nth(n)` — compose only
- `.partition(pred)` — needs scan

**Low — add when needed:**
- `.zip(other)` — needs API design + builtin
- `.unzip()` — new builtin
- `.sortBy(f)` — new AST node
- `.takeWhile(pred)` / `.skipWhile(pred)` — need scan or dedicated AST
- `.stepBy(n)` — new builtin
- `.chunks(n)` / `.windows(n)` — new builtins
- `.position(pred)` — needs enumerate
- `.findMap(f)` — compose only
