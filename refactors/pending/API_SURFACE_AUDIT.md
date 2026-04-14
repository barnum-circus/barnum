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
| `set(key, value)` | `T → T & { K: V }` | Add/overwrite a field with a constant value |

## Self: HashMap (`Record<string, T>`)

Not yet supported. Hashmaps are dynamic string-keyed bags — fundamentally different from structs. If/when we need them, they get their own self type with their own operations:

| Name | Signature | Notes |
|------|-----------|-------|
| `keys()` | `Record<string, T> → string[]` | |
| `values()` | `Record<string, T> → T[]` | |
| `entries()` | `Record<string, T> → {key: string, value: T}[]` | |
| `fromEntries()` | `{key: string, value: T}[] → Record<string, T>` | Constructor |
| `has(key)` | `Record<string, T> → boolean` | Dynamic key lookup |
| `size()` | `Record<string, T> → number` | |

None of these are proposed for the current release. They belong to a future where barnum has first-class hashmap support with a distinct type (not conflated with structs).

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
| `Option.collect()` | `Option<T>[] → T[]` | Filter + extract Somes |

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
| `Option.filter(pred)` | `Option<T> → Option<T>` | exists | |
| `Option.isSome()` | `Option<T> → boolean` | exists | |
| `Option.isNone()` | `Option<T> → boolean` | exists | |

### Proposed

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Option.okOr(action)` | `Option<T> → Result<T, E>` | composable | Branch → tag |
| `Option.zip` | `(Option<T>, Option<U>) → Option<[T, U]>` | composable | Low priority |
| `Option.transpose` | `Option<Result<T, E>> → Result<Option<T>, E>` | composable | Low priority |

---

## Self: `Result<T, E>`

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Result.ok()` | `T → Result<T, E>` | exists | Constructor |
| `Result.err()` | `E → Result<T, E>` | exists | Constructor |
| `Result.map(action)` | `Result<T, E> → Result<U, E>` | exists, postfix | |
| `Result.mapErr(action)` | `Result<T, E> → Result<T, F>` | exists, postfix | |
| `Result.andThen(action)` | `Result<T, E> → Result<U, E>` | exists, postfix | Monadic bind |
| `Result.or(action)` | `Result<T, E> → Result<T, F>` | exists | Fallback on Err |
| `Result.and(action)` | `Result<T, E> → Result<U, E>` | exists | Replace Ok |
| `Result.unwrapOr(action)` | `Result<T, E> → T` | exists, postfix | |
| `flattenResult()` | `Result<Result<T,E>,E> → Result<T,E>` | rename | Currently `Result.flatten()` — add top-level alias |
| `Result.toOption()` | `Result<T, E> → Option<T>` | exists | |
| `Result.toOptionErr()` | `Result<T, E> → Option<E>` | exists | |
| `Result.transpose()` | `Result<Option<T>, E> → Option<Result<T, E>>` | exists | |
| `Result.isOk()` | `Result<T, E> → boolean` | exists | |
| `Result.isErr()` | `Result<T, E> → boolean` | exists | |

---

## Self: `TaggedUnion<T>`

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `tag(kind)` | `T → TaggedUnion<{K: T}>` | exists | Constructor — wrap value as variant |
| `branch(cases)` | `TaggedUnion<T> → Out` | exists, postfix | Dispatch on discriminant |

---

## Handler & Execution

| Name | Status | Notes |
|------|--------|-------|
| `createHandler` | exists | Define TS handler with Zod validators |
| `createHandlerWithConfig` | exists | Handler with step config |
| `runPipeline` | exists | Run pipeline to completion |
| `config` | exists | Config factory |
| `zodToCheckedJsonSchema` | exists | Zod → JSON Schema |
| `taggedUnionSchema` | exists | Zod schema for TaggedUnion |
| `Option.schema` | exists | Zod schema for Option |
| `Result.schema` | exists | Zod schema for Result |

---

## Types

| Name | Status | Notes |
|------|--------|-------|
| `TypedAction<In, Out>` | exists | Core pipeline-typed action |
| `Pipeable<In, Out>` | exists | Parameter type for combinators |
| `Action` | exists | Untyped AST union |
| `Config` | exists | Top-level workflow config |
| `TaggedUnion<TDef>` | exists | `{ kind, value }` discriminated union |
| `Option<T>` | exists | Some/None |
| `Result<TValue, TError>` | exists | Ok/Err |
| `LoopResult<TC, TB>` | exists | Continue/Break |
| `VarRef<TValue>` | exists | Bound variable reference |
| `ExtractInput<T>` | exists | |
| `ExtractOutput<T>` | exists | |
| `PipeIn<T>` | exists | Maps never/void → any |
| `Handler<V, O>` | exists | Opaque handler reference |

---

## Naming Collisions & Renames

Operations that exist for multiple self types need explicit names to avoid ambiguity:

| Current name | Self type | Proposed name | Notes |
|--------------|-----------|---------------|-------|
| `flatten()` | `T[][]` | `flattenArray()` | Currently standalone + postfix `.flatten()` |
| `Option.flatten()` | `Option<Option<T>>` | `flattenOption()` | Currently namespace-only |
| `Result.flatten()` | `Result<Result<T,E>,E>` | `flattenResult()` | Currently namespace-only |

The namespace forms (`Option.flatten()`, `Result.flatten()`) can remain as aliases, but the canonical name should be self-type-explicit. The standalone `flatten()` must be renamed to `flattenArray()` since there's no namespace to disambiguate.

Postfix `.flatten()` could dispatch based on self type (see UNION_POSTFIX_DISPATCH.md) or be split into `.flattenArray()`, `.flattenOption()`, `.flattenResult()`.

### Other potential collisions

| Operation | Self types | Currently disambiguated? |
|-----------|-----------|--------------------------|
| `map` | Option, Result | Yes — `Option.map()`, `Result.map()` (array uses `forEach`) |
| `andThen` | Option, Result | Yes — namespaced |
| `unwrapOr` | Option, Result | Yes — namespaced |
| `collect` | Option (on `Option<T>[]`) | Yes — `Option.collect()` only |
| `first`/`last` | Array | No collision — only array |
| `isEmpty` | array | No collision now — only array |

---

## Removals

| Name | Reason | Action |
|------|--------|--------|
| `tap` | Subsumed by `bind`/`bindInput` | Remove from public exports, delete postfix `.tap()` |
| `merge` | See below | Remove JS export, delete postfix `.merge()` |

### `merge` is a code smell

`merge` (self: `[...objects]` → flat object) is used by four unrelated functions: `tag`, `pick`, `withResource`, `tap`. All four follow the same `all(...) → merge()` pattern. This is suspicious — three unrelated self types (`T`, `Obj`, `TIn`) share a dependency on a tuple-flattening operation.

Root cause: `tag` and `pick` were recently moved from Rust builtins to JS compositions via `all + wrapInField + merge`. This turned single Rust operations (`json!({"kind": k, "value": input})` for tag, field subset for pick) into multi-node AST trees. The `merge` dependency is an artifact of that decomposition.

Options:
1. **Restore `Tag` and `Pick` as Rust builtins.** Then `merge` is only needed by `withResource` and the proposed `allObject`. Simpler ASTs, fewer nodes to traverse at runtime.
2. **Keep JS composition, accept the pattern.** `allObject` becomes the canonical abstraction for `all(...) → merge()`. Internal uses in `tag`/`pick`/`withResource` are implementation details.
3. **Hybrid.** Restore `Tag` as a Rust builtin (it's a fundamental operation), keep `pick` as JS composition (it's just `allObject` with `getField` per key).

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

## Priority Tiers

### Tier 1
- Renames: `flatten` → `flattenArray`, add `flattenOption`, `flattenResult`
- Removals: `tap`, `merge` (from public API)
- `getField`/`getIndex` return `Option` by default
- Control flow: `allObject`, `withRetries`, curried `withTimeout`
- Array: `Arr.length`, `Arr.isEmpty`, `Arr.join`
- Struct: `omit`, `set`

### Tier 2
- Remaining Arr (reverse, take, skip, contains, enumerate, sortBy, unique, zip, append)

---

## What this doc consolidates

Content from these docs was folded in here:
- `PRIMITIVE_BUILTINS.md` — All content (deleted)
- `BARNUM_NEXT.md` sections 1–4 — curried withTimeout, withRetries, allObject, array ops (removed; structural/architectural retained)
- `OPTION_RETURNING_EXTRACTORS.md` — All content (deleted)

Related docs kept separate (different concerns):
- `THUNK_BUILTINS.md` — `ActionLike` ergonomics for zero-arg builtins
- `INLINE_BUILTINS.md` — Execution model (resolve builtins in advance phase)
- `UNION_POSTFIX_DISPATCH.md` — Runtime dispatch for postfix `.map()` etc. across union families
- `VOID_INPUTS.md` — Type convention for pipeline-ignoring actions
