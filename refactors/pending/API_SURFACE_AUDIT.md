# API Surface Audit

Complete inventory of everything exposed from the JS library, organized by **self type** (what's in the pipeline when the operation runs). Covers what exists, what's missing, and what to add.

**Goal:** Rationalize the API surface before the next release.

---

## Legend

| Status | Meaning |
|--------|---------|
| **exists** | Shipped and working |
| **remove** | Exists but should be removed |
| **rename** | Exists but needs a new name |
| **proposed** | Not yet implemented |
| **composable** | Can be built from existing primitives (no new engine work) |

---

## Control Flow (self: determined by context)

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `sleep(ms)` | `number → void` | exists | Rust builtin, timing primitive |
| `pipe` | Variadic sequential (1–11 steps) | exists | |
| `chain` | `(A→B, B→C) → A→C` | exists | Binary sequential |
| `all` | Variadic concurrent (0–10 branches) | exists | |
| `loop` | `(body) → TBreak` | exists | `TBreak=void`, `TRecur=void` defaults |
| `recur` | `TIn → never` | exists | Loop continue |
| `earlyReturn` | Scope with early exit token | exists | `TEarlyReturn=void` default |
| `tryCatch` | `(body, handler) → Out` | exists | Error recovery |
| `race` | `(...actions) → first-to-complete` | exists | |
| `withTimeout` | `(ms, body) → Result<Out, void>` | exists | Race body against timer |
| `bind` | `(bindings, body) → Out` | exists, postfix | Concurrent let-bindings |
| `bindInput` | `(body) → Out` | exists, postfix | Capture input as VarRef |
| `defineRecursiveFunctions` | Mutual recursion | exists | |
| `withResource` | `(create, body, dispose) → Out` | exists | RAII pattern |

### Proposed

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `allObject` | `Record<K, Action> → { [K]: Out }` | composable | wrapInField each key, All, merge internally |
| `withRetries(n)` | `(action) → action` | composable | Loop + tryCatch + counter |
| `withTimeout` (curried) | `(ms) → (body) → Result<Out, void>` | exists (refactor) | Curry existing two-arg form |

---

## Self: `T` (any value)

Operations that work regardless of what's in the pipeline.

| Name | Signature | Notes |
|------|-----------|-------|
| `constant(v)` | `any → T` | Fixed value, ignores input |
| `identity` | `T → T` | Pass through |
| `drop` | `T → void` | Postfix `.drop()` |
| `panic(msg)` | `any → never` | Fatal error, not caught by tryCatch. Rust builtin. |
| `wrapInField(key)` | `T → { K: T }` | Wrap under a key |

### Removed

| Name | Reason | Status |
|------|--------|--------|
| `tap(action)` | Subsumed by `bind`/`bindInput` | **done** |
| `merge()` | Internal plumbing for `pick`, `allObject`, `withResource`. Not user-facing. Keep Rust builtin, remove JS export. | pending |

---

## Self: Struct (typed object with known fields)

Objects in barnum are **structs** — fields are known at compile time. This is distinct from hashmaps (dynamic string-keyed bags). Struct operations take literal keys as type parameters.

| Name | Signature | Notes |
|------|-----------|-------|
| `getField(key)` | `Obj → Obj[K]` | Postfix `.getField()`. Struct fields are known at compile time — returning raw value is correct. `Option` semantics belong on HashMap.get, not struct field access. |
| `pick(...keys)` | `Obj → Pick<Obj, Keys>` | Postfix `.pick()` |

### Proposed

| Name | Signature | Notes |
|------|-----------|-------|
| `omit(...keys)` | `T → Omit<T, Keys>` | Complement of pick |

## Self: HashMap (`Record<string, T>`)

Not yet supported. Hashmaps are dynamic string-keyed bags — fundamentally different from structs. When we add them, they get their own self type following Rust's `HashMap` API:

| Name | Signature | Notes |
|------|-----------|-------|
| `HashMap.new()` | `any → Record<string, T>` | Constructor (empty map) |
| `HashMap.fromEntries()` | `{key: string, value: T}[] → Record<string, T>` | Constructor |
| `get(key)` | `Record<string, T> → Option<T>` | Lookup by key |
| `insert(key, value)` | `Record<string, T> → Record<string, T>` | Add/overwrite entry |
| `remove(key)` | `Record<string, T> → Record<string, T>` | Remove entry |
| `containsKey(key)` | `Record<string, T> → boolean` | |
| `keys()` | `Record<string, T> → string[]` | |
| `values()` | `Record<string, T> → T[]` | |
| `entries()` | `Record<string, T> → {key: string, value: T}[]` | Rust: `iter()` |
| `len()` | `Record<string, T> → number` | |
| `isEmpty()` | `Record<string, T> → boolean` | |

Not proposed for the current release. Belongs to a future where barnum has first-class hashmap support with a distinct type (not conflated with structs).

---

---

## Self: `T[]` (array)

| Name | Signature | Notes |
|------|-----------|-------|
| `range(start, end)` | `any → number[]` | Constant integer array, ignores input |
| `forEach(action)` | `T[] → U[]` | Postfix. Low-level parallel map over elements. **Prefer `.iterate().map(action).collect()`**. |
| `getIndex(n)` | `Tuple → Option<Tuple[N]>` | Returns `Option`. Compose `.unwrap()` for known-present. |
| `flatten()` | `T[][] → T[]` | Postfix `.flatten()`. Array-only builtin. |
| `splitFirst()` | `T[] → Option<[T, T[]]>` | Postfix. Head/tail decomposition |
| `splitLast()` | `T[] → Option<[T[], T]>` | Postfix. Init/last decomposition |
| `first()` | `T[] → Option<T>` | Standalone function (not postfix). Safe first element |
| `last()` | `T[] → Option<T>` | Standalone function (not postfix). Safe last element |
| `.iterate()` | `T[] → Iterator<T>` | Postfix. Enter Iterator for `.map()`, `.flatMap()`, `.filter()`, `.collect()`. |

### Proposed

| Name | Signature | Notes |
|------|-----------|-------|
| `Arr.length()` | `T[] → number` | New `ArrayLength` builtin |
| `Arr.isEmpty()` | `T[] → boolean` | Composable: `Arr.length() → constant(0) → eq` or new builtin |
| `Arr.join(sep)` | `string[] → string` | New builtin |
| `Arr.reverse()` | `T[] → T[]` | New `Reverse` builtin |
| `Arr.take(n)` | `T[] → T[]` | New `Take` builtin |
| `Arr.skip(n)` | `T[] → T[]` | New `Skip` builtin |
| `Arr.contains(v)` | `T[] → boolean` | |
| `Arr.enumerate()` | `T[] → [number, T][]` | New `Enumerate` builtin |
| `Arr.sortBy(field)` | `T[] → T[]` | New `SortBy` AST node (action arg) |
| `Arr.unique()` | `T[] → T[]` | |
| `Arr.zip()` | `[T[], U[]] → [T, U][]` | Binary |
| `Arr.append()` | `[T[], T[]] → T[]` | Binary concat |

---

## Self: `Iterator<T>`

`Iterator<T>` is `TaggedUnion<"Iterator", { Iterator: T[] }>`. Runtime representation: `{ kind: "Iterator.Iterator", value: T[] }`.

Iterators are **eager** (backed by arrays). `.map()` dispatches via `ForEach` (parallel). See ITERATOR_METHODS.md for the full method catalog including implementation details.

### Constructors

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Iterator.fromArray()` | `T[] → Iterator<T>` | exists | `tag("Iterator", "Iterator")` |
| `Iterator.fromOption()` | `Option<T> → Iterator<T>` | exists | Some → 1-element, None → empty |
| `Iterator.fromResult()` | `Result<T, E> → Iterator<T>` | exists | Ok → 1-element, Err → empty |
| `.iterate()` | `T[] / Option<T> / Result<T,E> → Iterator<T>` | exists, postfix | `branchFamily` dispatch across all three |

### Transforming

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Iterator.map(action)` | `Iterator<T> → Iterator<U>` | exists, postfix | Parallel via `ForEach` |
| `Iterator.flatMap(action)` | `Iterator<T> → Iterator<U>` | exists, postfix | `action` returns any IntoIterator (Iterator, Option, Result, array). Normalized via `branchFamily`. |
| `Iterator.filter(pred)` | `Iterator<T> → Iterator<T>` | exists, postfix | `pred: T → boolean`. Implemented as flatMap + AsOption + bindInput. |
| `Iterator.collect()` | `Iterator<T> → T[]` | exists, postfix | `getField("value")` |

### Postfix dispatch

`.map()`, `.flatMap()`, `.filter()`, and `.collect()` are available as postfix methods. `.map()` additionally dispatches across Option and Result via `branchFamily`. `.collect()` dispatches between `Iterator<T>` (→ `getField("value")`) and `Option<T>[]` (→ `CollectSome` builtin).

### Proposed (see ITERATOR_METHODS.md for details)

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `.filterMap(f)` | `Iterator<T> → Iterator<U>` | composable | `flatMap(f)` where `f: T → Option<U>`. Type-constrained alias. |
| `.flatten()` | `Iterator<IntoIter<T>> → Iterator<T>` | composable | `flatMap(identity())` |
| `.enumerate()` | `Iterator<T> → Iterator<[number, T]>` | proposed | New `Enumerate` builtin |
| `.first()` / `.last()` | `Iterator<T> → Option<T>` | composable | `collect → splitFirst/splitLast → Option.map(getIndex)` |
| `.find(pred)` | `Iterator<T> → Option<T>` | composable | `filter(pred).first()` |
| `.nth(n)` | `Iterator<T> → Option<T>` | composable | `collect → getIndex(n)` |
| `.count()` | `Iterator<T> → number` | proposed | New `ArrayLength` builtin |
| `.any(pred)` | `Iterator<T> → boolean` | composable | `find(pred).isSome()` |
| `.take(n)` / `.skip(n)` | `Iterator<T> → Iterator<T>` | proposed | New builtins |
| `.reverse()` | `Iterator<T> → Iterator<T>` | proposed | New `Reverse` builtin |
| `.chain(other)` | `Iterator<T>, Iterator<T> → Iterator<T>` | composable | Concatenate via `all + flatten + fromArray` |
| `.collectResult()` | `Iterator<Result<T,E>> → Result<T[],E>` | proposed | New `CollectResult` builtin |
| `.scan(init, f)` | `Iterator<T> → Iterator<U>` | proposed | **New `Scan` AST node.** Sequential primitive. Unlocks fold, reduce, forEachSync. |
| `.fold(init, f)` | `Iterator<T> → U` | composable (needs scan) | `scan(init, f).last().unwrap()` |
| `.partition(pred)` | `Iterator<T> → [T[], T[]]` | proposed (needs scan) | |
| `.zip(other)` | `Iterator<T>, Iterator<U> → Iterator<[T,U]>` | proposed | New `Zip` builtin |
| `.sortBy(f)` | `Iterator<T> → Iterator<T>` | proposed | New `SortBy` AST node |

---

## Self: `boolean`

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `asOption()` | `boolean → Option<void>` | exists, postfix | `AsOption` Rust builtin. `true` → Some, `false` → None. Used internally by `Iterator.filter`. |

---

## Self: `Option<T>`

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Option.some()` | `T → Option<T>` | exists | Constructor. Postfix `.some()`. |
| `Option.none()` | `any → Option<T>` | exists | Constructor, ignores input |
| `Option.map(action)` | `Option<T> → Option<U>` | exists, postfix | Postfix `.map()` dispatches across Option/Result/Iterator |
| `Option.andThen(action)` | `Option<T> → Option<U>` | exists, postfix | Monadic bind |
| `Option.unwrap()` | `Option<T> → T` | exists, postfix | Panics on None (fatal, not caught by tryCatch) |
| `Option.unwrapOr(action)` | `Option<T> → T` | exists, postfix | Postfix `.unwrapOr()` dispatches across Option/Result |
| `Option.filter(pred)` | `Option<T> → Option<T>` | exists, postfix | `pred: T → Option<T>` (not boolean — returns Some to keep, None to drop) |
| `Option.isSome()` | `Option<T> → boolean` | exists, postfix | |
| `Option.isNone()` | `Option<T> → boolean` | exists, postfix | |
| `Option.collect()` | `Option<T>[] → T[]` | exists, postfix | `CollectSome` Rust builtin. Postfix `.collect()` dispatches between `Option<T>[]` and `Iterator<T>`. |
| `Option.transpose()` | `Option<Result<T,E>> → Result<Option<T>,E>` | exists, postfix | Swaps nesting, changes family to Result |
| `.iterate()` | `Option<T> → Iterator<T>` | exists, postfix | Some → 1-element Iterator, None → empty |

### Proposed

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Option.flatten()` | `Option<Option<T>> → Option<T>` | composable | `Option.andThen(identity())` |
| `Option.okOr(action)` | `Option<T> → Result<T, E>` | composable | Branch → tag |
| `Option.zip` | `(Option<T>, Option<U>) → Option<[T, U]>` | composable | Low priority |

---

## Self: `Result<T, E>`

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Result.ok()` | `T → Result<T, E>` | exists | Constructor. Postfix `.ok()`. |
| `Result.err()` | `E → Result<T, E>` | exists | Constructor. Postfix `.err()`. |
| `Result.map(action)` | `Result<T, E> → Result<U, E>` | exists, postfix | Postfix `.map()` dispatches across Option/Result/Iterator |
| `Result.mapErr(action)` | `Result<T, E> → Result<T, F>` | exists, postfix | |
| `Result.andThen(action)` | `Result<T, E> → Result<U, E>` | exists, postfix | Monadic bind |
| `Result.or(action)` | `Result<T, E> → Result<T, F>` | exists, postfix | Fallback on Err |
| `Result.unwrap()` | `Result<T, E> → T` | exists, postfix | Panics on Err (fatal, not caught by tryCatch) |
| `Result.unwrapOr(action)` | `Result<T, E> → T` | exists, postfix | Postfix `.unwrapOr()` dispatches across Option/Result |
| `Result.asOkOption()` | `Result<T, E> → Option<T>` | exists, postfix | |
| `Result.asErrOption()` | `Result<T, E> → Option<E>` | exists, postfix | |
| `Result.transpose()` | `Result<Option<T>, E> → Option<Result<T, E>>` | exists, postfix | |
| `Result.isOk()` | `Result<T, E> → boolean` | exists, postfix | |
| `Result.isErr()` | `Result<T, E> → boolean` | exists, postfix | |
| `.iterate()` | `Result<T, E> → Iterator<T>` | exists, postfix | Ok → 1-element Iterator, Err → empty |

### Proposed

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Result.flatten()` | `Result<Result<T,E>,E> → Result<T,E>` | composable | `Result.andThen(identity())` |
| `Result.and(action)` | `Result<T, E> → Result<U, E>` | composable | Replace Ok value regardless. `andThen` where body ignores input. |

---

## Self: `TaggedUnion<T>` (generic dispatch)

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `tag(kind, enumName)` | `T → TaggedUnion<TEnumName, {K: T}>` | exists | Constructor — wrap value as namespaced variant |
| `branch(cases)` | `TaggedUnion<T> → Out` | exists, postfix | Dispatch on discriminant. Auto-unwraps `value`. |
| `branchFamily(cases)` | `TaggedUnion<T> → Out` | exists | Two-level dispatch: extractPrefix → branch. Used by postfix methods (`.map()`, `.unwrapOr()`, `.iterate()`, etc.) to dispatch across Option/Result/Iterator/Array. |
| `extractPrefix()` | `{kind: "Prefix.Variant", ...} → {kind: "Prefix", value: original}` | exists | Rust builtin. Splits kind on `'.'`. For bare arrays (no `kind` field), produces `{kind: "Array", value: input}`. Internal — used by `branchFamily`. |
| `asOption()` | `boolean → Option<void>` | exists, postfix | Rust `AsOption` builtin. `true` → Some, `false` → None. |

---

## `flatten` — array-only

`flatten` is the array builtin `T[][] → T[]`. The postfix `.flatten()` calls the array Flatten builtin directly.

For Option/Result flattening, use `andThen(identity())`:
- `Option<Option<T>> → Option<T>`: `Option.andThen(identity())`
- `Result<Result<T,E>,E> → Result<T,E>`: `Result.andThen(identity())`

These are composable from existing primitives — no dedicated flatten combinator needed.

---

## Removals

| Name | Reason | Status |
|------|--------|--------|
| `tap` | Subsumed by `bind`/`bindInput` | **done** — removed from exports and postfix |
| `__union` runtime dispatch | Replaced by `branchFamily` + `ExtractPrefix` AST nodes | **done** |
| `merge` | See below | pending |

### `merge` → `allObject`

`merge` is internal plumbing used by `tag`, `pick`, `withResource` — all follow `all(...) → merge()`. `allObject` is the canonical abstraction for this pattern. Internal uses of `merge` become implementation details of `allObject`, `tag`, `pick`, `withResource`.

---

## Design Decisions

### Error handling

For field/index access, the primitive returns `Option` (safe by default). Compose `.unwrap()` for known-present access. No separate `tryGetField` — `getField` IS the safe version.

Convention: `try` prefix always means `Result<T, E>`, never `Option<T>`.

### Namespace naming

| Namespace | Self type |
|-----------|-----------|
| `Arr` | `T[]` |
| `Option` | `Option<T>` |
| `Result` | `Result<T, E>` |
| `Iterator` | `Iterator<T>` |

### Trait dispatch via branchFamily

Postfix methods like `.map()`, `.unwrapOr()`, `.collect()`, `.iterate()` dispatch across multiple self types using `branchFamily` (= `extractPrefix()` → `branch()`). This gives Rust trait-like dispatch: `.map()` on Option calls `Option.map`, on Result calls `Result.map`, on Iterator calls `Iterator.map`.

### Iterator vs forEach

`forEach` is the low-level `ForEach` AST node — parallel map over array elements. `Iterator.map()` wraps this in a typed API with `Iterator<T>` as the self type. User-facing code should use `.iterate().map(f).collect()` instead of `forEach(f)`. `forEach` remains exported for backward compatibility and internal use.

### Thunk builtins

Ergonomic improvement where zero-arg builtins can be passed as bare references. Orthogonal to this audit. See THUNK_BUILTINS.md.

---

## TODOs

### Done
- [x] Remove `tap` from public exports
- [x] `mapOption` → `map` — renamed, converted to dispatch
- [x] `mapErr` → converted to dispatch
- [x] `unwrapOr` — widened to Option + Result, converted to dispatch
- [x] `Option.transpose` — implemented, dispatched
- [x] `.flatten()` — array-only builtin
- [x] `unwrap` — panicking unwrap for Option and Result
- [x] `panic(msg)` — Panic builtin (TS + Rust)
- [x] `__union` dispatch replaced by `branchFamily` + `ExtractPrefix` (see UNION_DISPATCH_AST_NODES in past/)
- [x] `getIndex(n)` returns `Option<Tuple[N]>` instead of raw value
- [x] Iterator Phase 1 — `Iterator<T>` type, fromArray/fromOption/fromResult, map, flatMap, filter, collect
- [x] `branchFamily` — two-level dispatch via ExtractPrefix + Branch
- [x] `AsOption` builtin — `boolean → Option<void>`, used by Iterator.filter
- [x] `.iterate()` postfix — dispatches across Option/Result/Array via branchFamily
- [x] `.map()` postfix dispatches Iterator (in addition to Option/Result)
- [x] `.collect()` postfix dispatches Iterator (in addition to Option[])

### Pending
- [ ] Remove `merge` from JS export, delete postfix `.merge()` (keep Rust builtin)

### Postfix: future
- [ ] `.omit()` — Struct-only (when implemented)

### New: control flow
- [ ] `allObject` — `Record<K, Action> → { [K]: Out }` (composable)
- [ ] `withRetries(n)` — retry on error (composable: loop + tryCatch)
- [ ] Curry `withTimeout` — `(ms) → (body) → Result<Out, void>`

### New: struct
- [ ] `omit(...keys)` — complement of `pick`

### New: array
- [ ] `Arr.length()` — `T[] → number` (new `ArrayLength` builtin)
- [ ] `Arr.isEmpty()` — `T[] → boolean`
- [ ] `Arr.join(sep)` — `string[] → string`

### New: Iterator (Phase 2 — see ITERATOR_METHODS.md)
- [ ] `.filterMap(f)` — composable: `flatMap(f)` type alias
- [ ] `.flatten()` — composable: `flatMap(identity())`
- [ ] `.first()` / `.last()` — composable from existing builtins
- [ ] `.find(pred)` — composable: `filter(pred).first()`
- [ ] `.enumerate()` — new `Enumerate` builtin
- [ ] `.count()` — new `ArrayLength` builtin
- [ ] `.collectResult()` — new `CollectResult` builtin
- [ ] `.scan(init, f)` — **new `Scan` AST node** (complex, unlocks fold/reduce/forEachSync)
- [ ] `.fold(init, f)` / `.reduce(f)` — composable from scan

### New: Option
- [ ] `Option.okOr(action)` — `Option<T> → Result<T, E>` (composable)

### Resolve: merge → allObject
- [ ] Implement `allObject` as the canonical abstraction for `all() → merge()`
- [ ] Refactor `tag`, `pick`, `withResource` to use `allObject` internally

### Lower priority (tier 2)
- [ ] Arr: reverse, take, skip, contains, enumerate, sortBy, unique, zip, append
- [ ] Iterator: take, skip, reverse, chain, zip, sortBy, partition, takeWhile, skipWhile, chunks, windows
- [ ] Option: zip
- [ ] HashMap: first-class support as distinct type from struct
