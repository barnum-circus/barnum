# API Surface Audit

Complete inventory of everything exposed from the JS library, organized by **self type** (what's in the pipeline when the operation runs). Covers what exists, what's missing, and what to add.

**Goal:** Rationalize the API surface before the next release.

---

## Legend

| Status | Meaning |
|--------|---------|
| **exists** | Shipped and working |
| **proposed** | Not yet implemented |
| **composable** | Can be built from existing primitives (no new engine work) |

---

## Signature conventions

Signatures describe the pipeline type transformation: `InputType → OutputType`. When the input is genuinely ignored (the combinator works regardless of what's in the pipeline), `any` is used. Config parameters (passed at AST construction time, not at runtime) are shown in the name: `sleep(ms)`, `constant(v)`, `take(n)`.

---

## Control Flow

These are combinators — they compose actions into larger actions. They don't operate on a specific self type.

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `pipe(a, b, ...)` | `A → ... → Z` | exists | Variadic sequential (1–11 steps) |
| `chain(a, b)` | `A → B → C` | exists | Binary sequential |
| `all(a, b, ...)` | `T → [A, B, ...]` | exists | Variadic concurrent fan-out (0–10 branches) |
| `allObject({k: action, ...})` | `T → {k: TOut, ...}` | exists | Named concurrent fan-out. `wrapInField` each key → `all` → `merge`. |
| `loop(body)` | `TRecur → TBreak` | exists | HOAS. `TBreak=void`, `TRecur=void` defaults. `VoidToNull` applied to output. |
| `recur(body)` | `TIn → TOut` | exists | Simple restart-based recursion. Body receives `restart` token. |
| `earlyReturn(body)` | `TIn → TOut \| TEarlyReturn` | exists | Scope with early exit token |
| `tryCatch(body, handler)` | `TIn → TOut` | exists | Error recovery. Type-level errors only (not JS exceptions). |
| `race(...actions)` | `T → TOut` | exists | First branch to complete wins, losers cancelled |
| `withTimeout(ms, body)` | `TIn → Result<TOut, void>` | exists | Race body against timer |
| `bind(bindings, body)` | `T → TOut` | exists, postfix | Concurrent let-bindings |
| `bindInput(body)` | `T → TOut` | exists, postfix | Capture input as VarRef |
| `defineRecursiveFunctions(bodies)(entry)` | `any → TOut` | exists | Mutual recursion via ResumeHandle |
| `withResource({create, action, dispose})` | `TIn → TOut` | exists | RAII pattern |
| `sleep(ms)` | `any → void` | exists | Rust builtin. `ms` is config, input is ignored. |

### Proposed

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `withRetries(n, action)` | `T → TOut` | composable | `loop` + `tryCatch` + counter |

---

## Self: `any` (works on any value)

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `constant(v)` | `any → TValue` | exists | Fixed value, input ignored |
| `identity()` | `T → T` | exists | Pass-through |
| `drop` | `any → void` | exists | Postfix `.drop()`. Discard value. |
| `panic(msg)` | `any → never` | exists | Fatal, not caught by tryCatch |
| `wrapInField(key)` | `T → {K: T}` | exists | Postfix `.wrapInField(key)` |

---

## Self: `Struct` (typed object with known fields)

Struct operations take literal keys as type parameters. Distinct from hashmaps (dynamic string-keyed bags).

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `getField(key)` | `TObj → TObj[K]` | exists, postfix | |
| `pick(...keys)` | `TObj → Pick<TObj, Keys>` | exists, postfix | |

### Proposed

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `omit(...keys)` | `TObj → Omit<TObj, Keys>` | proposed | Complement of pick |

---

## Self: `HashMap` (`Record<string, T>`)

Not yet supported. Future work — distinct type from structs.

| Name | Signature | Notes |
|------|-----------|-------|
| `HashMap.new()` | `void → Record<string, T>` | Constructor (empty map) |
| `HashMap.fromEntries()` | `{key: string, value: T}[] → Record<string, T>` | Constructor |
| `HashMap.get(key)` | `Record<string, T> → Option<T>` | Lookup |
| `HashMap.insert(key, value)` | `Record<string, T> → Record<string, T>` | Add/overwrite |
| `HashMap.remove(key)` | `Record<string, T> → Record<string, T>` | Remove |
| `HashMap.containsKey(key)` | `Record<string, T> → boolean` | |
| `HashMap.keys()` | `Record<string, T> → string[]` | |
| `HashMap.values()` | `Record<string, T> → T[]` | |
| `HashMap.entries()` | `Record<string, T> → {key: string, value: T}[]` | |
| `HashMap.len()` | `Record<string, T> → number` | |
| `HashMap.isEmpty()` | `Record<string, T> → boolean` | |

---

## Self: `T[]` (array)

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `range(start, end)` | `any → number[]` | exists | Constant array, input ignored |
| `forEach(action)` | `T[] → U[]` | exists, postfix | Low-level parallel map. **Prefer `.iterate().map(action).collect()`.** |
| `getIndex(n)` | `T[] → Option<T[N]>` | exists, postfix | Returns `Option`. Compose `.unwrap()` for known-present. |
| `flatten()` | `T[][] → T[]` | exists, postfix | One level of flattening |
| `splitFirst()` | `T[] → Option<[T, T[]]>` | exists, postfix | Head/tail decomposition |
| `splitLast()` | `T[] → Option<[T[], T]>` | exists, postfix | Init/last decomposition |
| `first()` | `T[] → Option<T>` | exists | Standalone. Composes `splitFirst` + `Option.map(getIndex(0).unwrap())`. |
| `last()` | `T[] → Option<T>` | exists | Standalone. Composes `splitLast` + `Option.map(getIndex(1).unwrap())`. |
| `.iterate()` | `T[] → Iterator<T>` | exists, postfix | Enter Iterator |

### Proposed

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Arr.length()` | `T[] → number` | proposed | New `ArrayLength` builtin. Also powers `Iterator.count()`. |
| `Arr.isEmpty()` | `T[] → boolean` | proposed | |
| `Arr.join(sep)` | `string[] → string` | proposed | New builtin |

Other array operations (reverse, take, skip, enumerate, sortBy, unique, zip, append, contains) belong on Iterator. Use `.iterate()` to enter the Iterator API.

---

## Self: `Iterator<T>`

`Iterator<T>` is `TaggedUnion<"Iterator", { Iterator: T[] }>`. Runtime: `{ kind: "Iterator.Iterator", value: T[] }`.

Iterators are **eager** (backed by arrays). `.map()` dispatches via `ForEach` (parallel). See ITERATOR_METHODS.md for the full method catalog with implementation details.

### Constructors

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Iterator.fromArray()` | `T[] → Iterator<T>` | exists | `tag("Iterator", "Iterator")` |
| `Iterator.fromOption()` | `Option<T> → Iterator<T>` | exists | Some → 1-element, None → empty |
| `Iterator.fromResult()` | `Result<T, E> → Iterator<T>` | exists | Ok → 1-element, Err → empty |
| `.iterate()` | `T[] / Option<T> / Result<T, E> → Iterator<T>` | exists, postfix | `branchFamily` dispatch |

### Transforming

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Iterator.map(action)` | `Iterator<T> → Iterator<U>` | exists, postfix | Parallel via `ForEach` |
| `Iterator.flatMap(action)` | `Iterator<T> → Iterator<U>` | exists, postfix | `action` returns any IntoIterator type. Normalized via `branchFamily`. |
| `Iterator.filter(pred)` | `Iterator<T> → Iterator<T>` | exists, postfix | `pred: T → boolean`. Implemented as flatMap + AsOption + bindInput. |
| `Iterator.collect()` | `Iterator<T> → T[]` | exists, postfix | `getField("value")` |

### Postfix dispatch

`.map()` dispatches across Option, Result, and Iterator via `branchFamily`. `.collect()` dispatches between `Iterator<T>` (→ `getField("value")`) and `Option<T>[]` (→ `CollectSome` builtin). `.filter()` dispatches between `Iterator<T>` (pred returns `boolean`) and `Option<T>` (pred returns `Option<T>`).

### Proposed (see ITERATOR_METHODS.md)

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `.filterMap(f)` | `Iterator<T> → Iterator<U>` | composable | `flatMap(f)` where `f: T → Option<U>` |
| `.flatten()` | `Iterator<IntoIter<T>> → Iterator<T>` | composable | `flatMap(identity())` |
| `.enumerate()` | `Iterator<T> → Iterator<[number, T]>` | proposed | New `Enumerate` builtin |
| `.first()` / `.last()` | `Iterator<T> → Option<T>` | composable | `collect` → `splitFirst`/`splitLast` → `Option.map(getIndex)` |
| `.find(pred)` | `Iterator<T> → Option<T>` | composable | `filter(pred).first()` |
| `.nth(n)` | `Iterator<T> → Option<T>` | composable | `collect` → `getIndex(n)` |
| `.count()` | `Iterator<T> → number` | proposed | New `ArrayLength` builtin |
| `.any(pred)` | `Iterator<T> → boolean` | composable | `find(pred).isSome()` |
| `.take(n)` / `.skip(n)` | `Iterator<T> → Iterator<T>` | proposed | New builtins |
| `.reverse()` | `Iterator<T> → Iterator<T>` | proposed | New `Reverse` builtin |
| `.chain(other)` | `(Iterator<T>, Iterator<T>) → Iterator<T>` | composable | `all` + `flatten` + `fromArray` |
| `.collectResult()` | `Iterator<Result<T, E>> → Result<T[], E>` | proposed | New `CollectResult` builtin |
| `.scan(init, f)` | `Iterator<T> → Iterator<U>` | proposed | **New `Scan` AST node.** Sequential primitive. Unlocks fold/reduce/forEachSync. |
| `.fold(init, f)` | `Iterator<T> → U` | composable (needs scan) | `scan(init, f).last().unwrap()` |
| `.partition(pred)` | `Iterator<T> → [T[], T[]]` | proposed (needs scan) | |
| `.zip(other)` | `(Iterator<T>, Iterator<U>) → Iterator<[T, U]>` | proposed | New `Zip` builtin |
| `.sortBy(f)` | `Iterator<T> → Iterator<T>` | proposed | New `SortBy` AST node |
| `.unique()` | `Iterator<T> → Iterator<T>` | proposed | New `Unique` builtin |

---

## Self: `boolean`

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `asOption()` | `boolean → Option<void>` | exists, postfix | `AsOption` Rust builtin. `true` → Some, `false` → None. |

---

## Self: `Option<T>`

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Option.some()` | `T → Option<T>` | exists | Constructor. Postfix `.some()`. |
| `Option.none()` | `void → Option<T>` | exists | Constructor |
| `Option.map(action)` | `Option<T> → Option<U>` | exists, postfix | `.map()` dispatches across Option/Result/Iterator |
| `Option.andThen(action)` | `Option<T> → Option<U>` | exists, postfix | Monadic bind. `action: T → Option<U>`. |
| `Option.unwrap()` | `Option<T> → T` | exists, postfix | Panics on None |
| `Option.unwrapOr(default)` | `Option<T> → T` | exists, postfix | `default: void → T`. `.unwrapOr()` dispatches across Option/Result. |
| `Option.filter(pred)` | `Option<T> → Option<T>` | exists, postfix | `pred: T → Option<T>`. Returns Some to keep, None to drop. `.filter()` dispatches: Option takes `T → Option<T>`, Iterator takes `T → boolean`. |
| `Option.isSome()` | `Option<T> → boolean` | exists, postfix | |
| `Option.isNone()` | `Option<T> → boolean` | exists, postfix | |
| `Option.collect()` | `Option<T>[] → T[]` | exists, postfix | `CollectSome` Rust builtin. `.collect()` dispatches between `Option<T>[]` and `Iterator<T>`. |
| `Option.transpose()` | `Option<Result<T, E>> → Result<Option<T>, E>` | exists, postfix | |
| `.iterate()` | `Option<T> → Iterator<T>` | exists, postfix | Some → 1-element, None → empty |

### Proposed

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Option.flatten()` | `Option<Option<T>> → Option<T>` | composable | `Option.andThen(identity())` |
| `Option.okOr(err)` | `Option<T> → Result<T, E>` | composable | `err: void → E`. Branch → tag. |

---

## Self: `Result<T, E>`

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Result.ok()` | `T → Result<T, E>` | exists | Constructor. Postfix `.ok()`. |
| `Result.err()` | `E → Result<T, E>` | exists | Constructor. Postfix `.err()`. |
| `Result.map(action)` | `Result<T, E> → Result<U, E>` | exists, postfix | `action: T → U`. `.map()` dispatches across Option/Result/Iterator. |
| `Result.mapErr(action)` | `Result<T, E> → Result<T, F>` | exists, postfix | `action: E → F` |
| `Result.andThen(action)` | `Result<T, E> → Result<U, E>` | exists, postfix | `action: T → Result<U, E>` |
| `Result.or(fallback)` | `Result<T, E> → Result<T, F>` | exists, postfix | `fallback: E → Result<T, F>` |
| `Result.unwrap()` | `Result<T, E> → T` | exists, postfix | Panics on Err |
| `Result.unwrapOr(default)` | `Result<T, E> → T` | exists, postfix | `default: E → T`. `.unwrapOr()` dispatches across Option/Result. |
| `Result.asOkOption()` | `Result<T, E> → Option<T>` | exists, postfix | Ok → Some, Err → None |
| `Result.asErrOption()` | `Result<T, E> → Option<E>` | exists, postfix | Err → Some, Ok → None |
| `Result.transpose()` | `Result<Option<T>, E> → Option<Result<T, E>>` | exists, postfix | |
| `Result.isOk()` | `Result<T, E> → boolean` | exists, postfix | |
| `Result.isErr()` | `Result<T, E> → boolean` | exists, postfix | |
| `.iterate()` | `Result<T, E> → Iterator<T>` | exists, postfix | Ok → 1-element, Err → empty |

### Proposed

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Result.flatten()` | `Result<Result<T, E>, E> → Result<T, E>` | composable | `Result.andThen(identity())` |

---

## Self: `TaggedUnion` (generic dispatch infrastructure)

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `tag(kind, enumName)` | `T → TaggedUnion<TEnumName, {K: T}>` | exists | Constructor. Postfix `.tag(kind)` (infers enumName from context). |
| `branch(cases)` | `TaggedUnion → TOut` | exists, postfix | Dispatch on discriminant. Auto-unwraps `value`. |
| `branchFamily(cases)` | `TaggedUnion → TOut` | exists | Two-level dispatch: `extractPrefix` → `branch`. Powers `.map()`, `.unwrapOr()`, `.iterate()`, etc. |
| `extractPrefix()` | `{kind, value} → {kind: prefix, value: original}` | exists | Rust builtin. Splits kind on `'.'`. Bare arrays → `{kind: "Array", value: input}`. Internal. |

---

## Standalone utilities

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `taggedUnionSchema(enumName, cases)` | Zod schema constructor | exists | Builds `z.discriminatedUnion` for `TaggedUnion` |
| `asOption()` | `boolean → Option<void>` | exists | Standalone form of `.asOption()` postfix |
| `first()` | `T[] → Option<T>` | exists | See array section |
| `last()` | `T[] → Option<T>` | exists | See array section |
| `runPipeline(pipeline, input?)` | `Action → Promise<TOut>` | exists | Execute a pipeline via the Rust runtime |
| `zodToCheckedJsonSchema(schema)` | Zod schema → JSON Schema | exists | Validates and converts Zod schemas for handler definitions |
| `config(workflow)` | `Action → Config` | exists | Wraps a pipeline for `runPipeline` |

---

## `flatten` — array-only

`flatten` is the array builtin `T[][] → T[]`. The postfix `.flatten()` calls the array Flatten builtin directly.

For Option/Result flattening, use `andThen(identity())`:
- `Option<Option<T>> → Option<T>`: `Option.andThen(identity())`
- `Result<Result<T, E>, E> → Result<T, E>`: `Result.andThen(identity())`

These are composable — no dedicated flatten combinator needed.

---

## Internal (not user-facing)

These exist in the codebase but are not part of the public API. Kept for reference.

| Name | Notes |
|------|-------|
| `merge()` | Rust builtin. Merges a tuple of objects. Used internally by `pick`, `allObject`, `tag`, `withResource`. |
| `toAction()` | Strips phantom types from Pipeable → Action. |
| `typedAction()` | Attaches postfix methods to a plain Action. |
| `resetEffectIdCounter()` | Testing utility. Resets gensym counters. |
| `buildRestartBranchAction()` | Infrastructure for restart-based combinators (loop, earlyReturn, race, tryCatch). |

---

## Design Decisions

### Error handling

Field/index access returns `Option` (safe by default). Compose `.unwrap()` for known-present access. No separate `tryGetField` — `getField` IS the safe version.

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

`forEach` is the low-level `ForEach` AST node — parallel map over array elements. `Iterator.map()` wraps this in a typed API. User-facing code should use `.iterate().map(f).collect()`. `forEach` remains exported for internal use.

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
- [x] `__union` dispatch replaced by `branchFamily` + `ExtractPrefix`
- [x] `getIndex(n)` returns `Option<T[N]>`
- [x] Iterator Phase 1 — `Iterator<T>` type, fromArray/fromOption/fromResult, map, flatMap, filter, collect
- [x] `branchFamily` — two-level dispatch via ExtractPrefix + Branch
- [x] `AsOption` builtin — `boolean → Option<void>`, used by Iterator.filter
- [x] `.iterate()` postfix — dispatches across Option/Result/Array via branchFamily
- [x] `.map()` postfix dispatches Iterator (in addition to Option/Result)
- [x] `.collect()` postfix dispatches Iterator (in addition to Option[])
- [x] `allObject` — composable from existing primitives
- [x] Remove `merge` from JS export, delete postfix `.merge()` (keep Rust builtin)
- [x] Remove `tap`, `__union` dispatch

### Proposed
- [ ] `withRetries(n)` — composable: loop + tryCatch
- [ ] `omit(...keys)` — struct operation
- [ ] `Arr.length()` — new `ArrayLength` builtin
- [ ] `Arr.isEmpty()` — new builtin
- [ ] `Arr.join(sep)` — new builtin
- [ ] `Option.okOr(err)` — composable

### Proposed: Iterator Phase 2 (see ITERATOR_METHODS.md)
- [ ] `.filterMap(f)` — composable: type-constrained flatMap
- [ ] `.flatten()` — composable: `flatMap(identity())`
- [ ] `.first()` / `.last()` — composable
- [ ] `.find(pred)` — composable: `filter(pred).first()`
- [ ] `.enumerate()` — new `Enumerate` builtin
- [ ] `.count()` — new `ArrayLength` builtin
- [ ] `.collectResult()` — new `CollectResult` builtin
- [ ] `.scan(init, f)` — **new `Scan` AST node** (unlocks fold/reduce/forEachSync)
- [ ] `.fold(init, f)` / `.reduce(f)` — composable from scan

### Lower priority
- [ ] Iterator: take, skip, reverse, chain, zip, sortBy, unique, partition, takeWhile, skipWhile, chunks, windows, contains/any, append/concat
- [ ] Arr: length, isEmpty, join
- [ ] Option: flatten, okOr
- [ ] HashMap: first-class support
