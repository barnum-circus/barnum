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

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `constant(v)` | `any → T` | exists | Fixed value, ignores input |
| `identity` | `T → T` | exists | Pass through |
| `drop` | `T → void` | exists, postfix | Discard value |
| `wrapInField(key)` | `T → { K: T }` | exists | Wrap under a key |
| `Cmp.eq(v)` | `T → boolean` | proposed | Deep JSON equality |
| `Cmp.neq(v)` | `T → boolean` | proposed | |
| `Convert.toString()` | `T → string` | proposed | |
| `Convert.toBool()` | `T → boolean` | proposed | JS truthiness |
| `Convert.toJson()` | `T → string` | proposed | JSON.stringify |

### Removed

| Name | Reason |
|------|--------|
| `tap(action)` | Subsumed by `bind`/`bindInput`. Remove from public API. |
| `merge()` | Internal plumbing for `pick`, `allObject`, `withResource`. Not user-facing. Keep Rust builtin, remove JS export. |

---

## Self: `Record` / Object

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `getField(key)` | `Obj → Obj[K]` | exists, postfix | |
| `getIndex(n)` | `Tuple → Tuple[N]` | exists | |
| `pick(...keys)` | `Obj → Pick<Obj, Keys>` | exists, postfix | |
| `Obj.omit(...keys)` | `T → Omit<T, Keys>` | proposed | Complement of pick |
| `Obj.has(key)` | `Record → boolean` | proposed | |
| `Obj.set(key, value)` | `T → T & { K: V }` | proposed | Add/overwrite constant field |
| `Obj.keys()` | `Record → string[]` | proposed | |
| `Obj.values()` | `Record<K, T> → T[]` | proposed | |
| `Obj.entries()` | `Record<K, T> → {key, value}[]` | proposed | |
| `Obj.fromEntries()` | `{key, value}[] → Record` | proposed | Self: `{key, value}[]` not Record |
| `Obj.size()` | `Record → number` | proposed | |
| `Str.template(tpl)` | `{...} → string` | proposed | `"${field}"` interpolation from object fields |
| `tryGetField(key)` | `Obj → Option<Obj[K]>` | proposed | Fallible field access |
| `tryGetIndex(n)` | `Tuple → Option<Tuple[N]>` | proposed | Fallible index access |

---

## Self: `number`

Nothing exists today.

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Num.add(n)` | `number → number` | proposed | |
| `Num.sub(n)` | `number → number` | proposed | |
| `Num.mul(n)` | `number → number` | proposed | |
| `Num.div(n)` | `number → number` | proposed | Panics on div by zero |
| `Num.mod(n)` | `number → number` | proposed | |
| `Num.pow(n)` | `number → number` | proposed | |
| `Num.min(n)` | `number → number` | proposed | |
| `Num.max(n)` | `number → number` | proposed | |
| `Num.negate()` | `number → number` | proposed | |
| `Num.abs()` | `number → number` | proposed | |
| `Num.floor()` | `number → number` | proposed | |
| `Num.ceil()` | `number → number` | proposed | |
| `Num.round()` | `number → number` | proposed | |
| `Num.clamp(min, max)` | `number → number` | proposed | |
| `Num.tryDiv(n)` | `number → Option<number>` | proposed | Safe div by zero |
| `Cmp.lt(n)` | `number → boolean` | proposed | |
| `Cmp.lte(n)` | `number → boolean` | proposed | |
| `Cmp.gt(n)` | `number → boolean` | proposed | |
| `Cmp.gte(n)` | `number → boolean` | proposed | |
| `Convert.toNumber()` | `string → number` | proposed | (self: string, listed here for discoverability) |

### Binary forms (self: `[number, number]`)

When both operands come from the pipeline via `all`:

| Name | Signature | Notes |
|------|-----------|-------|
| `Num.add()` | `[number, number] → number` | No-arg = binary overload |
| `Num.sub()` | `[number, number] → number` | |
| `Num.mul()` | `[number, number] → number` | |
| `Num.div()` | `[number, number] → number` | |
| `Num.min()` | `[number, number] → number` | |
| `Num.max()` | `[number, number] → number` | |
| `Cmp.eq()` | `[T, T] → boolean` | |
| `Cmp.lt()` | `[number, number] → boolean` | |
| etc. | | |

---

## Self: `string`

Nothing exists today.

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Str.length()` | `string → number` | proposed | |
| `Str.isEmpty()` | `string → boolean` | proposed | |
| `Str.concat(s)` | `string → string` | proposed | Also binary |
| `Str.includes(s)` | `string → boolean` | proposed | |
| `Str.trim()` | `string → string` | proposed | |
| `Str.toUpperCase()` | `string → string` | proposed | |
| `Str.toLowerCase()` | `string → string` | proposed | |
| `Str.startsWith(s)` | `string → boolean` | proposed | |
| `Str.endsWith(s)` | `string → boolean` | proposed | |
| `Str.split(sep)` | `string → string[]` | proposed | |
| `Str.replace(pat, rep)` | `string → string` | proposed | |
| `Str.slice(start, end?)` | `string → string` | proposed | |
| `Str.padStart(len, fill?)` | `string → string` | proposed | |
| `Str.padEnd(len, fill?)` | `string → string` | proposed | |
| `Str.parseNumber()` | `string → number` | proposed | Panics on non-numeric |
| `Str.parseJson()` | `string → unknown` | proposed | Panics on malformed |
| `Str.tryParseNumber()` | `string → Option<number>` | proposed | Safe variant |
| `Convert.fromJson()` | `string → unknown` | proposed | JSON.parse |
| `Convert.tryFromJson()` | `string → Option<unknown>` | proposed | Safe JSON parse |

---

## Self: `boolean`

Nothing exists today.

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Bool.not()` | `boolean → boolean` | proposed | |
| `Bool.branch(t, f)` | `boolean → Out` | proposed | Dispatch on bool; desugars to BoolToTagged + branch |

### Binary forms (self: `[boolean, boolean]`)

| Name | Signature | Notes |
|------|-----------|-------|
| `Bool.and()` | `[boolean, boolean] → boolean` | |
| `Bool.or()` | `[boolean, boolean] → boolean` | |

---

## Self: `T[]` (array)

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `range(start, end)` | `any → number[]` | exists | Constant integer array, ignores input |
| `forEach(action)` | `T[] → U[]` | exists, postfix | Map over elements |
| `flattenArray()` | `T[][] → T[]` | rename | Currently `flatten()` — rename to disambiguate from Option/Result |
| `splitFirst()` | `T[] → Option<[T, T[]]>` | exists, postfix | Head/tail decomposition |
| `splitLast()` | `T[] → Option<[T[], T]>` | exists, postfix | Init/last decomposition |
| `first()` | `T[] → Option<T>` | exists, postfix | Safe first element |
| `last()` | `T[] → Option<T>` | exists, postfix | Safe last element |
| `Option.collect()` | `Option<T>[] → T[]` | exists | Filter + extract Somes |
| `Arr.length()` | `T[] → number` | proposed | |
| `Arr.isEmpty()` | `T[] → boolean` | proposed | |
| `Arr.join(sep)` | `string[] → string` | proposed | |
| `Arr.reverse()` | `T[] → T[]` | proposed | New Rust builtin (can't compose) |
| `Arr.take(n)` | `T[] → T[]` | proposed | |
| `Arr.skip(n)` | `T[] → T[]` | proposed | |
| `Arr.contains(v)` | `T[] → boolean` | proposed | |
| `Arr.enumerate()` | `T[] → {index, value}[]` | proposed | |
| `Arr.sortBy(field)` | `T[] → T[]` | proposed | |
| `Arr.unique()` | `T[] → T[]` | proposed | |

### Binary forms (self: `[T[], U[]]`)

| Name | Signature | Notes |
|------|-----------|-------|
| `Arr.zip()` | `[T[], U[]] → [T, U][]` | |
| `Arr.append()` | `[T[], T[]] → T[]` | Concat |

### Composable (no new Rust builtins)

| Name | Composition | Notes |
|------|-------------|-------|
| `filter(pred)` | `forEach(pred).then(Option.collect())` | `pred: T → Option<T>` |
| `flatMap(action)` | `forEach(action).then(flattenArray())` | |

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
| `isEmpty` | string, array | Would collide if both are standalone — use `Str.isEmpty()`, `Arr.isEmpty()` |

---

## Removals

| Name | Reason | Action |
|------|--------|--------|
| `tap` | Subsumed by `bind`/`bindInput` | Remove from public exports, delete postfix `.tap()` |
| `merge` | Internal plumbing for `pick`/`allObject`/`withResource` | Keep Rust builtin, remove JS export, delete postfix `.merge()` |

---

## Design Decisions

### Parameterized vs binary builtins

All numeric, comparison, and string builtins support two forms:
- **Parameterized** `Num.add(5)`: `number → number` (pipeline value + constant)
- **Binary** `Num.add()`: `[number, number] → number` (both from pipeline via `all`)

Overloaded in TypeScript: presence of arg determines which form.

### Error handling in builtins

Builtins that fail at runtime (div by zero, parse non-numeric, index OOB) are Byzantine faults. Scheduler panics the workflow.

For fallible operations, provide `Option`-returning `try*` variants.

### Namespace naming

Use `Num` not `Math` (avoids shadowing JS global).

| Namespace | Self type |
|-----------|-----------|
| `Num` | `number` |
| `Bool` | `boolean` |
| `Cmp` | `T` or `number` |
| `Str` | `string` |
| `Arr` | `T[]` |
| `Obj` | `Record` |
| `Convert` | varies |
| `Option` | `Option<T>` |
| `Result` | `Result<T, E>` |

### Postfix methods for primitive namespaces

Defer. Namespace form is clear. Postfix on TypedAction reserved for structural operations.

### Thunk builtins

Ergonomic improvement where zero-arg builtins can be passed as bare references. Orthogonal to this audit. See THUNK_BUILTINS.md.

---

## Priority Tiers

### Tier 1 — basic pipeline logic
- Comparison: `Cmp.eq`, `Cmp.neq`, `Cmp.gt`, `Cmp.lt`, `Cmp.gte`, `Cmp.lte`
- Boolean: `Bool.not`, `Bool.branch`
- Numeric: `Num.add`, `Num.sub`, `Num.mul`
- Array: `Arr.length`, `Arr.isEmpty`, `Arr.join`
- String: `Str.length`, `Str.isEmpty`, `Str.concat`, `Str.includes`, `Str.template`
- Object: `Obj.omit`, `Obj.set`, `Obj.has`
- Control flow: `allObject`, `withRetries`, curried `withTimeout`
- Renames: `flatten` → `flattenArray`, add `flattenOption`, `flattenResult`
- Removals: `tap`, `merge` (from public API)

### Tier 2 — data shaping
- Remaining Num (div, mod, pow, min, max, negate, abs, floor, ceil, round, clamp)
- Remaining Str (trim, case, startsWith, endsWith, split, replace, slice, pad, parse)
- Remaining Arr (reverse, take, skip, contains, enumerate, sortBy, unique, zip, append)
- Remaining Obj (keys, values, entries, fromEntries, size)
- Bool.and, Bool.or
- All Convert operations

### Tier 3 — safe `try*` variants
- `Num.tryDiv`, `Str.tryParseNumber`, `Convert.tryFromJson`
- `tryGetField`, `tryGetIndex`

### Tier 4 — binary overloads
- Binary forms of all Num and Cmp builtins

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
