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
| `forEach(action)` | `T[] → U[]` | Postfix. Map over elements |
| `getIndex(n)` | `Tuple → Option<Tuple[N]>` | Returns `Option`. Compose `.unwrap()` for known-present. |
| `flatten()` | `T[][] → T[]` | Postfix `.flatten()`. Array-only builtin. |
| `splitFirst()` | `T[] → Option<[T, T[]]>` | Postfix. Head/tail decomposition |
| `splitLast()` | `T[] → Option<[T[], T]>` | Postfix. Init/last decomposition |
| `first()` | `T[] → Option<T>` | Postfix. Safe first element |
| `last()` | `T[] → Option<T>` | Postfix. Safe last element |

### Proposed

| Name | Signature | Notes |
|------|-----------|-------|
| `Arr.length()` | `T[] → number` | |
| `Arr.isEmpty()` | `T[] → boolean` | |
| `Arr.join(sep)` | `string[] → string` | |
| `Arr.reverse()` | `T[] → T[]` | New Rust builtin (can't compose) |
| `Arr.take(n)` | `T[] → T[]` | |
| `Arr.skip(n)` | `T[] → T[]` | |
| `Arr.contains(v)` | `T[] → boolean` | |
| `Arr.enumerate()` | `T[] → {index, value}[]` | |
| `Arr.sortBy(field)` | `T[] → T[]` | |
| `Arr.unique()` | `T[] → T[]` | |
| `Arr.zip()` | `[T[], U[]] → [T, U][]` | Binary |
| `Arr.append()` | `[T[], T[]] → T[]` | Binary concat |
| `filter(pred)` | `T[] → T[]` | Composable: `forEach(pred).then(Option.collect())` |
| `flatMap(action)` | `T[] → U[]` | Composable: `forEach(action).then(flattenArray())` |

---

## Self: `Option<T>`

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Option.some()` | `T → Option<T>` | exists | Constructor |
| `Option.none()` | `any → Option<T>` | exists | Constructor, ignores input |
| `Option.map(action)` | `Option<T> → Option<U>` | exists, postfix | |
| `Option.andThen(action)` | `Option<T> → Option<U>` | exists, postfix | Monadic bind |
| `Option.unwrap()` | `Option<T> → T` | exists, postfix | Panics on None (fatal, not caught by tryCatch) |
| `Option.unwrapOr(action)` | `Option<T> → T` | exists, postfix | |
| `Option.filter(pred)` | `Option<T> → Option<T>` | exists, postfix | |
| `Option.isSome()` | `Option<T> → boolean` | exists, postfix | |
| `Option.isNone()` | `Option<T> → boolean` | exists, postfix | |
| `Option.collect()` | `Option<T>[] → T[]` | exists, postfix | Filter + extract Somes |

| `Option.transpose()` | `Option<Result<T,E>> → Result<Option<T>,E>` | exists, postfix | Swaps nesting, changes family to Result |

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
| `Result.ok()` | `T → Result<T, E>` | exists | Constructor |
| `Result.err()` | `E → Result<T, E>` | exists | Constructor |
| `Result.map(action)` | `Result<T, E> → Result<U, E>` | exists, postfix | |
| `Result.mapErr(action)` | `Result<T, E> → Result<T, F>` | exists, postfix | |
| `Result.andThen(action)` | `Result<T, E> → Result<U, E>` | exists, postfix | Monadic bind |
| `Result.or(action)` | `Result<T, E> → Result<T, F>` | exists, postfix | Fallback on Err |
| `Result.and(action)` | `Result<T, E> → Result<U, E>` | exists, postfix | Replace Ok |
| `Result.unwrap()` | `Result<T, E> → T` | exists, postfix | Panics on Err (fatal, not caught by tryCatch) |
| `Result.unwrapOr(action)` | `Result<T, E> → T` | exists, postfix | |
| `Result.toOption()` | `Result<T, E> → Option<T>` | exists, postfix | |
| `Result.toOptionErr()` | `Result<T, E> → Option<E>` | exists, postfix | |
| `Result.transpose()` | `Result<Option<T>, E> → Option<Result<T, E>>` | exists, postfix | |
| `Result.isOk()` | `Result<T, E> → boolean` | exists, postfix | |
| `Result.isErr()` | `Result<T, E> → boolean` | exists, postfix | |

### Proposed

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Result.flatten()` | `Result<Result<T,E>,E> → Result<T,E>` | composable | `Result.andThen(identity())` |

---

## Self: `TaggedUnion<T>`

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `tag(kind, enumName)` | `T → TaggedUnion<TEnumName, {K: T}>` | exists | Constructor — wrap value as namespaced variant |
| `branch(cases)` | `TaggedUnion<T> → Out` | exists, postfix | Dispatch on discriminant. Auto-unwraps `value`. |
| `matchPrefix(cases)` | `TaggedUnion<T> → Out` | exists | Two-level dispatch: extract enum prefix, then branch. Used by postfix methods (`.map()`, `.unwrapOr()`, etc.) to dispatch across Option/Result. |
| `extractPrefix()` | `{kind: "Prefix.Variant", ...} → {kind: "Prefix", value: original}` | exists | Rust builtin. Splits kind on `'.'`. Internal — used by `matchPrefix`. |

---

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
| `__union` runtime dispatch | Replaced by `matchPrefix` + `ExtractPrefix` AST nodes | **done** |
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
- [x] `__union` dispatch replaced by `matchPrefix` + `ExtractPrefix` (see UNION_DISPATCH_AST_NODES in past/)
- [x] `getIndex(n)` returns `Option<Tuple[N]>` instead of raw value

### Pending
- [ ] Remove `merge` from JS export, delete postfix `.merge()` (keep Rust builtin)
- [ ] `IntoIterator` / `.intoIter()` — see TRAIT_DISPATCH_AND_ITERATORS.md

### Postfix: future
- [ ] `.omit()` — Struct-only (when implemented)

### New: control flow
- [ ] `allObject` — `Record<K, Action> → { [K]: Out }` (composable)
- [ ] `withRetries(n)` — retry on error (composable: loop + tryCatch)
- [ ] Curry `withTimeout` — `(ms) → (body) → Result<Out, void>`

### New: struct
- [ ] `omit(...keys)` — complement of `pick`

### New: array
- [ ] `Arr.length()` — `T[] → number`
- [ ] `Arr.isEmpty()` — `T[] → boolean`
- [ ] `Arr.join(sep)` — `string[] → string`
- [ ] `filter(pred)` — composable: `forEach(pred).then(Option.collect())`

### New: Option
- [ ] `Option.okOr(action)` — `Option<T> → Result<T, E>` (composable)

### Resolve: merge → allObject
- [ ] Implement `allObject` as the canonical abstraction for `all() → merge()`
- [ ] Refactor `tag`, `pick`, `withResource` to use `allObject` internally

### Lower priority (tier 2)
- [ ] Arr: reverse, take, skip, contains, enumerate, sortBy, unique, zip, append
- [ ] Option: zip
- [ ] HashMap: first-class support as distinct type from struct
