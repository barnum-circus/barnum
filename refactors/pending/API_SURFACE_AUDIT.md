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
| `wrapInField(key)` | `T → { K: T }` | Wrap under a key |

### Removed

| Name | Reason |
|------|--------|
| `tap(action)` | Subsumed by `bind`/`bindInput`. Remove from public API. |
| `merge()` | Internal plumbing for `pick`, `allObject`, `withResource`. Not user-facing. Keep Rust builtin, remove JS export. |

---

## Self: Struct (typed object with known fields)

Objects in barnum are **structs** — fields are known at compile time. This is distinct from hashmaps (dynamic string-keyed bags). Struct operations take literal keys as type parameters.

| Name | Signature | Notes |
|------|-----------|-------|
| `getField(key)` | `Obj → Option<Obj[K]>` | Postfix `.getField()`. Currently returns raw value; should return `Option`. Compose `.unwrap()` for known-present fields. |
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
| `getIndex(n)` | `Tuple → Option<Tuple[N]>` | Currently returns raw value; should return `Option`. Compose `.unwrap()` for known-present. |
| `flattenArray()` | `T[][] → T[]` | Currently `flatten()` — rename to disambiguate from Option/Result |
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
| `Option.unwrapOr(action)` | `Option<T> → T` | exists, postfix | |
| `flattenOption()` | `Option<Option<T>> → Option<T>` | rename | Currently `Option.flatten()` — add top-level alias |
| `Option.filter(pred)` | `Option<T> → Option<T>` | exists, postfix | |
| `Option.isSome()` | `Option<T> → boolean` | exists, postfix | |
| `Option.isNone()` | `Option<T> → boolean` | exists, postfix | |
| `Option.collect()` | `Option<T>[] → T[]` | exists, postfix | Filter + extract Somes |

### Proposed

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Option.okOr(action)` | `Option<T> → Result<T, E>` | composable | Branch → tag |
| `Option.zip` | `(Option<T>, Option<U>) → Option<[T, U]>` | composable | Low priority |
| `Option.transpose` | `Option<Result<T, E>> → Result<Option<T>, E>` | composable | Deferred from union dispatch — needs optionMethods.transpose |

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
| `Result.unwrapOr(action)` | `Result<T, E> → T` | exists, postfix | |
| `flattenResult()` | `Result<Result<T,E>,E> → Result<T,E>` | rename | Currently `Result.flatten()` — add top-level alias |
| `Result.toOption()` | `Result<T, E> → Option<T>` | exists, postfix | |
| `Result.toOptionErr()` | `Result<T, E> → Option<E>` | exists, postfix | |
| `Result.transpose()` | `Result<Option<T>, E> → Option<Result<T, E>>` | exists, postfix | |
| `Result.isOk()` | `Result<T, E> → boolean` | exists, postfix | |
| `Result.isErr()` | `Result<T, E> → boolean` | exists, postfix | |

---

## Self: `TaggedUnion<T>`

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `tag(kind)` | `T → TaggedUnion<{K: T}>` | exists | Constructor — wrap value as variant |
| `branch(cases)` | `TaggedUnion<T> → Out` | exists, postfix | Dispatch on discriminant |

---

---

## Naming Collisions

Standalone functions that share a name across self types need explicit disambiguation:

| Current name | Self type | Standalone name | Postfix |
|--------------|-----------|-----------------|---------|
| `flatten()` | `T[][]` | `flattenArray()` | `.flatten()` via dispatch |
| `Option.flatten()` | `Option<Option<T>>` | `flattenOption()` | `.flatten()` via dispatch |
| `Result.flatten()` | `Result<Result<T,E>,E>` | `flattenResult()` | `.flatten()` via dispatch |

Standalone functions: use self-type-explicit names (`flattenArray`, `flattenOption`, `flattenResult`).
Postfix methods: dispatch on concrete type — `.flatten()` just works.

---

## Removals

| Name | Reason | Action |
|------|--------|--------|
| `tap` | Subsumed by `bind`/`bindInput` | Remove from public exports, delete postfix `.tap()` |
| `merge` | See below | Remove JS export, delete postfix `.merge()` |

### `merge` → `allObject`

`merge` is internal plumbing used by `tag`, `pick`, `withResource`, `tap` — all follow `all(...) → merge()`. `allObject` is the canonical abstraction for this pattern. Internal uses of `merge` become implementation details of `allObject`, `tag`, `pick`, `withResource`.

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

### Removals
- [ ] Remove `tap` from public exports, delete postfix `.tap()`
- [ ] Remove `merge` from JS export, delete postfix `.merge()` (keep Rust builtin)

### Breaking changes
- [ ] `getField(key)` returns `Option<Obj[K]>` instead of raw value
- [ ] `getIndex(n)` returns `Option<Tuple[N]>` instead of raw value

### Union postfix dispatch (done — see past/UNION_POSTFIX_DISPATCH.md)
- [x] Implement `__union` runtime tag on TypedAction
- [x] Option/Result constructors attach tag
- [x] Union-aware combinators propagate tag
- [x] `chain()` propagates `__union` from rest action
- [x] `.andThen()` — dispatch to Option.andThen / Result.andThen
- [x] `.filter()` — Option.filter
- [x] `.isSome()`, `.isNone()` — Option-only
- [x] `.collect()` — Option.collect
- [x] `.mapErr()` — Result-only
- [x] `.or()`, `.and()` — Result-only
- [x] `.toOption()`, `.toOptionErr()` — Result-only
- [x] `.transpose()` — Result.transpose (Option.transpose deferred)
- [x] `.isOk()`, `.isErr()` — Result-only

### Phase 0 — obvious mechanical wiring (dispatch infra + namespace methods exist, just connect them)
- [ ] `mapOption` → `map` — rename postfix, convert hardcoded → dispatch (Option.map + Result.map both exist)
- [ ] `mapErr` → convert hardcoded → dispatch for consistency (Result-only, no new functionality)
- [ ] `unwrapOr` — widen to Option + Result, convert hardcoded → dispatch (both implementations exist)
- [ ] `Option.transpose` — implement combinator, add to optionMethods dispatch table

### Phase 2 — needs design
- [ ] `.flatten()` widening — three-way dispatch: array (no `__union`), Option, Result. Naming: `flattenArray`/`flattenOption`/`flattenResult` as standalone vs dispatched `.flatten()`
- [ ] `IntoIterator` / `.intoIter()` — convert Option/Result/Array to common iterable form so array methods (forEach, filter, etc.) work uniformly. See design notes below.

### Postfix: future
- [ ] `.omit()` — Struct-only (when implemented)
- [ ] `.flatMap()` — Array-only (when implemented)

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
- [ ] `flatMap(action)` — composable: `forEach(action).then(flattenArray())`

### New: Option
- [ ] `Option.okOr(action)` — `Option<T> → Result<T, E>` (composable)

### Resolve: merge → allObject
- [ ] Implement `allObject` as the canonical abstraction for `all() → merge()`
- [ ] Refactor `tag`, `pick`, `withResource` to use `allObject` internally

### Lower priority (tier 2)
- [ ] Arr: reverse, take, skip, contains, enumerate, sortBy, unique, zip, append
- [ ] Option: zip
- [ ] HashMap: first-class support as distinct type from struct
