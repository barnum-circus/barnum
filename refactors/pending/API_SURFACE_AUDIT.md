# API Surface Audit

Complete inventory of everything exposed from the JS library, what's missing, and what to do about it.

**Goal:** Rationalize the API surface before the next release.

---

## Legend

| Status | Meaning |
|--------|---------|
| **exists** | Shipped and working |
| **proposed** | Not yet implemented; needs new Rust builtin or TS combinator |
| **composable** | Can be built from existing primitives (no new engine work) |
| **postfix** | Available as a `.method()` on TypedAction |
| **namespace** | Available as `Namespace.method()` (e.g. `Option.map`) |

---

## 1. Core Combinators & Control Flow

Everything here exists today.

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `pipe` | Variadic sequential composition (1–11 steps) | exists | |
| `chain` | `(A→B, B→C) → A→C` | exists | Binary sequential composition |
| `all` | Variadic concurrent composition (0–10 branches) | exists | |
| `forEach` | `(T→U) → T[]→U[]` | exists, postfix | Array map |
| `branch` | `Record<K, CaseHandler> → TaggedUnion→Out` | exists, postfix | Dispatch on tagged union discriminant |
| `loop` | `(body) → TBreak` | exists | `TBreak=void`, `TRecur=void` defaults |
| `recur` | `TIn → never` | exists | Loop continue signal |
| `earlyReturn` | Scope with early exit token | exists | `TEarlyReturn=void` default |
| `tryCatch` | `(body, handler) → Out` | exists | Error recovery via restart+branch |
| `race` | `(...actions) → first-to-complete` | exists | |
| `sleep` | `number → void` | exists | Rust builtin |
| `withTimeout` | `(ms, body) → Result<Out, void>` | exists | Race body against timer |
| `bind` | `(bindings, body) → Out` | exists, postfix | Concurrent let-bindings |
| `bindInput` | `(body) → Out` | exists, postfix | Capture pipeline input as VarRef |
| `defineRecursiveFunctions` | Mutually recursive function definitions | exists | |

### Proposed additions

| Name | Signature | Status | Source |
|------|-----------|--------|--------|
| `allObject` | `Record<K, Action> → { [K]: Out }` | composable | Same pattern as `pick` — wrapInField each key, All, merge |
| `withRetries(n)` | `(action) → action` (retry on error) | composable | Loop + tryCatch + counter state |
| `withTimeout` (curried) | `(ms) → (body) → Result<Out, void>` | exists (refactor) | Curry existing two-arg form |
| `filter` | `(T→Option<T>) → T[]→T[]` | composable | `forEach(predicate).then(Option.collect())` |
| `flatMap` | `(T→U[]) → T[]→U[]` | composable | `forEach(action).then(flatten())` |

---

## 2. Data Transformations (Builtins)

Structural builtins that exist today.

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `constant` | `any → T` (fixed value) | exists | |
| `identity` | `T → T` | exists | |
| `drop` | `T → void` | exists, postfix | Discard value |
| `tag` | `T → TaggedUnion<{K: T}>` | exists | Wrap as union variant |
| `merge` | `[...objects] → merged` | exists | Merge tuple of objects |
| `flatten` | `T[][] → T[]` | exists, postfix | Array flatten one level |
| `getField` | `Obj → Obj[K]` | exists, postfix | Extract field |
| `getIndex` | `Tuple → Tuple[N]` | exists | Extract by index |
| `pick` | `Obj → Pick<Obj, Keys>` | exists, postfix | Select named fields |
| `wrapInField` | `T → { K: T }` | exists | Wrap value under a key |
| `withResource` | `(create, body, dispose) → Out` | exists | RAII pattern |
| `tap` | `(action) → T→T` | exists, postfix | Side effects, preserve input |
| `range` | `(start, end) → number[]` | exists | Constant integer array |
| `splitFirst` | `T[] → Option<[T, T[]]>` | exists, postfix | Head/tail decomposition |
| `splitLast` | `T[] → Option<[T[], T]>` | exists, postfix | Init/last decomposition |
| `first` | `T[] → Option<T>` | exists, postfix | First element |
| `last` | `T[] → Option<T>` | exists, postfix | Last element |
| `taggedUnionSchema` | Zod schema for TaggedUnion | exists | |

---

## 3. Option Namespace

Everything here exists today.

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Option.some` | `T → Option<T>` | exists, namespace | |
| `Option.none` | `any → Option<T>` | exists, namespace | |
| `Option.map` | `Option<T> → Option<U>` | exists, namespace + postfix | |
| `Option.andThen` | `Option<T> → Option<U>` | exists, namespace + postfix | Monadic bind |
| `Option.unwrapOr` | `Option<T> → T` | exists, namespace + postfix | |
| `Option.flatten` | `Option<Option<T>> → Option<T>` | exists, namespace | |
| `Option.filter` | `Option<T> → Option<T>` | exists, namespace | |
| `Option.collect` | `Option<T>[] → T[]` | exists, namespace | Filter+extract Somes |
| `Option.isSome` | `Option<T> → boolean` | exists, namespace | |
| `Option.isNone` | `Option<T> → boolean` | exists, namespace | |
| `Option.schema` | Zod schema for Option | exists, namespace | |

### Missing from Rust/TS conventions

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Option.zip` | `(Option<T>, Option<U>) → Option<[T, U]>` | composable | Branch + all |
| `Option.unzip` | `Option<[T, U]> → [Option<T>, Option<U>]` | composable | Branch + construct |
| `Option.xor` | `(Option<T>, Option<T>) → Option<T>` | composable | Branch both |
| `Option.okOr` | `Option<T> → Result<T, E>` | composable | Branch → tag |
| `Option.transpose` | `Option<Result<T, E>> → Result<Option<T>, E>` | composable | Nested branch |

These are all composable from branch + tag. Low priority — add when a demo or real workflow needs them.

---

## 4. Result Namespace

Everything here exists today.

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Result.ok` | `T → Result<T, E>` | exists, namespace | |
| `Result.err` | `E → Result<T, E>` | exists, namespace | |
| `Result.map` | `Result<T, E> → Result<U, E>` | exists, namespace + postfix | |
| `Result.mapErr` | `Result<T, E> → Result<T, F>` | exists, namespace + postfix | |
| `Result.andThen` | `Result<T, E> → Result<U, E>` | exists, namespace + postfix | |
| `Result.or` | `Result<T, E> → Result<T, F>` | exists, namespace | |
| `Result.and` | `Result<T, E> → Result<U, E>` | exists, namespace | |
| `Result.unwrapOr` | `Result<T, E> → T` | exists, namespace + postfix | |
| `Result.flatten` | `Result<Result<T, E>, E> → Result<T, E>` | exists, namespace | |
| `Result.toOption` | `Result<T, E> → Option<T>` | exists, namespace | |
| `Result.toOptionErr` | `Result<T, E> → Option<E>` | exists, namespace | |
| `Result.transpose` | `Result<Option<T>, E> → Option<Result<T, E>>` | exists, namespace | |
| `Result.isOk` | `Result<T, E> → boolean` | exists, namespace | |
| `Result.isErr` | `Result<T, E> → boolean` | exists, namespace | |
| `Result.schema` | Zod schema for Result | exists, namespace | |

### Missing from Rust conventions

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `Result.inspect` | `Result<T, E> → Result<T, E>` (side effect on Ok) | composable | `tap`-like branch |
| `Result.inspectErr` | `Result<T, E> → Result<T, E>` (side effect on Err) | composable | |

Low priority. `tap` + `branch` covers this pattern.

---

## 5. Numeric / Math

Nothing exists today. All proposed.

| Name | Signature | Status | Priority | Notes |
|------|-----------|--------|----------|-------|
| `Num.add(n)` | `number → number` | proposed | tier 1 | Also binary: `[number, number] → number` |
| `Num.sub(n)` | `number → number` | proposed | tier 1 | |
| `Num.mul(n)` | `number → number` | proposed | tier 1 | |
| `Num.div(n)` | `number → number` | proposed | tier 2 | |
| `Num.mod(n)` | `number → number` | proposed | tier 2 | |
| `Num.pow(n)` | `number → number` | proposed | tier 2 | |
| `Num.min(n)` | `number → number` | proposed | tier 2 | Also binary |
| `Num.max(n)` | `number → number` | proposed | tier 2 | Also binary |
| `Num.negate()` | `number → number` | proposed | tier 2 | |
| `Num.abs()` | `number → number` | proposed | tier 2 | |
| `Num.floor()` | `number → number` | proposed | tier 2 | |
| `Num.ceil()` | `number → number` | proposed | tier 2 | |
| `Num.round()` | `number → number` | proposed | tier 2 | |
| `Num.clamp(min, max)` | `number → number` | proposed | tier 2 | |
| `Num.tryDiv(n)` | `number → Option<number>` | proposed | tier 3 | Safe division by zero |

All Num operations need new Rust BuiltinKind variants. Each is 1–5 lines of Rust. Parameterized forms (with arg) operate on pipeline value + constant. Binary forms (no arg) operate on `[number, number]` tuples from `all`.

### Namespace name

Use `Num`, not `Math`. Avoids shadowing the JS global `Math`.

---

## 6. Boolean

Nothing exists today. All proposed.

| Name | Signature | Status | Priority | Notes |
|------|-----------|--------|----------|-------|
| `Bool.not()` | `boolean → boolean` | proposed | tier 1 | |
| `Bool.and()` | `[boolean, boolean] → boolean` | proposed | tier 2 | Binary |
| `Bool.or()` | `[boolean, boolean] → boolean` | proposed | tier 2 | Binary |
| `Bool.branch(t, f)` | `boolean → Out` | proposed | tier 1 | Dispatch on bool value |

### Bool.branch implementation

Two approaches:
- **BoolToTagged builtin** + reuse `branch` — more principled, reuses existing machinery
- **Dedicated IfElse AST node** — simpler at runtime

Recommendation: BoolToTagged + branch desugaring. `Bool.branch` is sugar that hides the conversion.

---

## 7. Comparison

Nothing exists today. All proposed.

| Name | Signature | Status | Priority | Notes |
|------|-----------|--------|----------|-------|
| `Cmp.eq(v)` | `T → boolean` | proposed | tier 1 | Deep JSON equality. Also binary. |
| `Cmp.neq(v)` | `T → boolean` | proposed | tier 1 | |
| `Cmp.lt(n)` | `number → boolean` | proposed | tier 1 | Also binary |
| `Cmp.lte(n)` | `number → boolean` | proposed | tier 1 | |
| `Cmp.gt(n)` | `number → boolean` | proposed | tier 1 | Also binary |
| `Cmp.gte(n)` | `number → boolean` | proposed | tier 1 | |

All need new Rust BuiltinKind variants. Parameterized (with arg) compares pipeline value to constant. Binary (no arg) compares `[T, T]` tuple from `all`.

---

## 8. String

Nothing exists today. All proposed.

| Name | Signature | Status | Priority | Notes |
|------|-----------|--------|----------|-------|
| `Str.length()` | `string → number` | proposed | tier 1 | |
| `Str.isEmpty()` | `string → boolean` | proposed | tier 1 | |
| `Str.concat(s)` | `string → string` | proposed | tier 1 | Also binary |
| `Str.includes(s)` | `string → boolean` | proposed | tier 1 | |
| `Str.template(tpl)` | `{...} → string` | proposed | tier 1 | `"${field}"` interpolation from object fields |
| `Str.trim()` | `string → string` | proposed | tier 2 | |
| `Str.toUpperCase()` | `string → string` | proposed | tier 2 | |
| `Str.toLowerCase()` | `string → string` | proposed | tier 2 | |
| `Str.startsWith(s)` | `string → boolean` | proposed | tier 2 | |
| `Str.endsWith(s)` | `string → boolean` | proposed | tier 2 | |
| `Str.split(sep)` | `string → string[]` | proposed | tier 2 | |
| `Str.replace(pat, rep)` | `string → string` | proposed | tier 2 | |
| `Str.slice(start, end?)` | `string → string` | proposed | tier 2 | |
| `Str.padStart(len, fill?)` | `string → string` | proposed | tier 2 | |
| `Str.padEnd(len, fill?)` | `string → string` | proposed | tier 2 | |
| `Str.parseNumber()` | `string → number` | proposed | tier 2 | Panics on non-numeric |
| `Str.parseJson()` | `string → unknown` | proposed | tier 2 | Panics on malformed |
| `Str.tryParseNumber()` | `string → Option<number>` | proposed | tier 3 | Safe variant |

---

## 9. Array

Some exist today. Others proposed.

| Name | Signature | Status | Priority | Notes |
|------|-----------|--------|----------|-------|
| `flatten` | `T[][] → T[]` | **exists**, postfix | — | |
| `forEach` | `(T→U) → T[]→U[]` | **exists**, postfix | — | |
| `getIndex` | `Tuple → Tuple[N]` | **exists** | — | |
| `splitFirst` | `T[] → Option<[T, T[]]>` | **exists**, postfix | — | Head/tail |
| `splitLast` | `T[] → Option<[T[], T]>` | **exists**, postfix | — | Init/last |
| `first` | `T[] → Option<T>` | **exists**, postfix | — | |
| `last` | `T[] → Option<T>` | **exists**, postfix | — | |
| `Option.collect` | `Option<T>[] → T[]` | **exists**, namespace | — | Filter Somes |
| `Arr.length()` | `T[] → number` | proposed | tier 1 | |
| `Arr.isEmpty()` | `T[] → boolean` | proposed | tier 1 | |
| `Arr.join(sep)` | `string[] → string` | proposed | tier 1 | |
| `Arr.reverse()` | `T[] → T[]` | proposed | tier 2 | New Rust builtin (can't compose) |
| `Arr.take(n)` | `T[] → T[]` | proposed | tier 2 | |
| `Arr.skip(n)` | `T[] → T[]` | proposed | tier 2 | |
| `Arr.contains(v)` | `T[] → boolean` | proposed | tier 2 | |
| `Arr.enumerate()` | `T[] → {index, value}[]` | proposed | tier 2 | |
| `Arr.sortBy(field)` | `T[] → T[]` | proposed | tier 2 | |
| `Arr.unique()` | `T[] → T[]` | proposed | tier 2 | |
| `Arr.zip()` | `[T[], U[]] → [T, U][]` | proposed | tier 2 | Binary |
| `Arr.append()` | `[T[], T[]] → T[]` | proposed | tier 2 | Binary concat |
| `Arr.first()` | `T[] → T` | proposed | tier 2 | Panics on empty (unsafe) |
| `Arr.last()` | `T[] → T` | proposed | tier 2 | Panics on empty (unsafe) |
| `Arr.tryFirst()` | `T[] → Option<T>` | proposed | tier 3 | Safe variant (= existing `first`) |
| `Arr.tryLast()` | `T[] → Option<T>` | proposed | tier 3 | Safe variant (= existing `last`) |

Note: existing `first`/`last` already return `Option<T>`. The proposed unsafe `Arr.first()`/`Arr.last()` would be a separate panicking variant. Consider whether both are needed or if the existing safe versions suffice.

### Composable array operations (no new Rust builtins)

| Name | Composition | Notes |
|------|-------------|-------|
| `filter(pred)` | `forEach(pred).then(Option.collect())` | `pred: T → Option<T>` |
| `flatMap(action)` | `forEach(action).then(flatten())` | |

---

## 10. Object

Some exist today. Others proposed.

| Name | Signature | Status | Priority | Notes |
|------|-----------|--------|----------|-------|
| `getField` | `Obj → Obj[K]` | **exists**, postfix | — | |
| `pick` | `Obj → Pick<Obj, Keys>` | **exists**, postfix | — | |
| `wrapInField` | `T → { K: T }` | **exists** | — | |
| `merge` | `[...objects] → merged` | **exists** | — | |
| `Obj.omit(...keys)` | `T → Omit<T, Keys>` | proposed | tier 1 | Complement of pick |
| `Obj.has(key)` | `Record → boolean` | proposed | tier 1 | |
| `Obj.set(key, value)` | `T → T & { K: V }` | proposed | tier 1 | Add/overwrite constant field |
| `Obj.keys()` | `Record → string[]` | proposed | tier 2 | |
| `Obj.values()` | `Record<K, T> → T[]` | proposed | tier 2 | |
| `Obj.entries()` | `Record<K, T> → {key, value}[]` | proposed | tier 2 | |
| `Obj.fromEntries()` | `{key, value}[] → Record` | proposed | tier 2 | |
| `Obj.size()` | `Record → number` | proposed | tier 2 | |

### Option-returning extractors

| Name | Signature | Status | Notes |
|------|-----------|--------|-------|
| `tryGetField` | `Obj → Option<Obj[K]>` | proposed | For when field presence is dynamic |
| `tryGetIndex` | `Tuple → Option<Tuple[N]>` | proposed | For when index might be OOB |

Current `getField`/`getIndex` return null on missing — silently wrong. Keep them as-is for internal use (engine knows fields exist). Add `tryGetField`/`tryGetIndex` for user-facing fallible access.

---

## 11. Type Conversions

Nothing exists today. All proposed.

| Name | Signature | Status | Priority | Notes |
|------|-----------|--------|----------|-------|
| `Convert.toString()` | `T → string` | proposed | tier 2 | |
| `Convert.toNumber()` | `string → number` | proposed | tier 2 | |
| `Convert.toBool()` | `T → boolean` | proposed | tier 2 | JS truthiness rules |
| `Convert.toJson()` | `T → string` | proposed | tier 2 | JSON.stringify |
| `Convert.fromJson()` | `string → unknown` | proposed | tier 2 | JSON.parse |
| `Convert.tryFromJson()` | `string → Option<unknown>` | proposed | tier 3 | Safe JSON parse |

---

## 12. Handler & Execution

| Name | Status | Notes |
|------|--------|-------|
| `createHandler` | exists | Define TS handler with optional Zod validators |
| `createHandlerWithConfig` | exists | Handler that takes step config |
| `runPipeline` | exists | Run pipeline to completion |
| `config` | exists | Simple config factory |
| `zodToCheckedJsonSchema` | exists | Zod → JSON Schema conversion |

---

## 13. Types (re-exported)

| Name | Status | Notes |
|------|--------|-------|
| `TypedAction<In, Out>` | exists | Core pipeline-typed action |
| `Pipeable<In, Out>` | exists | Parameter type for combinators |
| `Action` | exists | Untyped AST union |
| `Config` | exists | Top-level workflow config |
| `TaggedUnion<TDef>` | exists | Discriminated union `{ kind, value }` |
| `Option<T>` | exists | Some/None tagged union |
| `Result<TValue, TError>` | exists | Ok/Err tagged union |
| `LoopResult<TC, TB>` | exists | Continue/Break for loops |
| `VarRef<TValue>` | exists | Typed bound variable reference |
| `ExtractInput<T>` | exists | Extract input phantom type |
| `ExtractOutput<T>` | exists | Extract output phantom type |
| `PipeIn<T>` | exists | Maps never/void → any for positioning |
| `Handler<V, O>` | exists | Opaque handler reference |

---

## Design Decisions

### Parameterized vs binary builtins

All numeric, comparison, and string builtins support two forms:
- **Parameterized** `Num.add(5)`: `number → number` (pipeline value + constant)
- **Binary** `Num.add()`: `[number, number] → number` (both from pipeline via `all`)

Overloaded in TypeScript: presence of arg determines which form.

### Error handling in builtins

Builtins that can fail at runtime (div by zero, parse non-numeric, index OOB) are Byzantine faults — type system promised valid input. Scheduler panics the workflow.

For fallible operations, provide `Option`-returning `try*` variants. Users handle failure via `Option.unwrapOr`, `Option.map`, etc.

### Namespace naming

| Namespace | Covers |
|-----------|--------|
| `Num` | Arithmetic, rounding, clamping |
| `Bool` | Boolean logic, conditional dispatch |
| `Cmp` | Equality, ordering comparisons |
| `Str` | String manipulation |
| `Arr` | Array reshaping |
| `Obj` | Object field operations |
| `Convert` | Type coercion |
| `Option` | Option combinators (already exists) |
| `Result` | Result combinators (already exists) |

### Postfix methods for primitives

Defer. The namespace form (`Num.add(5)`) is clear and discoverable. Postfix on TypedAction is reserved for structural operations (`.branch()`, `.getField()`, `.flatten()`, etc.). Adding `action.add(5)` would bloat the TypedAction interface.

### Thunk builtins (`ActionLike`)

Zero-arg builtins (`drop`, `identity`, `recur`, `done`, `merge`, `flatten`) could be accepted as bare references instead of function calls. Combinators would accept `TypedAction | (() => TypedAction)` and resolve at construction time.

This is an ergonomic improvement orthogonal to the builtin inventory. See THUNK_BUILTINS.md for details.

---

## Priority Tiers

### Tier 1 — needed for basic pipeline logic
- `Cmp.eq`, `Cmp.neq`, `Cmp.gt`, `Cmp.lt`, `Cmp.gte`, `Cmp.lte`
- `Bool.not`, `Bool.branch`
- `Num.add`, `Num.sub`, `Num.mul`
- `Arr.length`, `Arr.isEmpty`, `Arr.join`
- `Str.length`, `Str.isEmpty`, `Str.concat`, `Str.includes`, `Str.template`
- `Obj.omit`, `Obj.set`, `Obj.has`
- `allObject`, `withRetries`, curried `withTimeout`

### Tier 2 — data shaping
- Remaining Num operations (div, mod, pow, min, max, negate, abs, floor, ceil, round, clamp)
- Remaining Str operations (trim, case, startsWith, endsWith, split, replace, slice, pad, parse)
- Remaining Arr operations (reverse, take, skip, contains, enumerate, sortBy, unique, zip, append)
- Remaining Obj operations (keys, values, entries, fromEntries, size)
- Bool.and, Bool.or
- All Convert operations

### Tier 3 — safe `try*` variants
- `Num.tryDiv`, `Str.tryParseNumber`, `Convert.tryFromJson`
- `Arr.tryFirst`, `Arr.tryLast` (= existing `first`/`last`)
- `tryGetField`, `tryGetIndex`

### Tier 4 — binary overloads
- Binary forms of all Num and Cmp builtins

---

## What this doc consolidates

Content from these docs was folded in here:
- `PRIMITIVE_BUILTINS.md` — All content (deleted)
- `BARNUM_NEXT.md` sections 1–4 — curried withTimeout, withRetries, allObject, array ops (removed; section 5 structural/architectural retained)
- `OPTION_RETURNING_EXTRACTORS.md` — All content (deleted)

Related docs kept separate (different concerns):
- `THUNK_BUILTINS.md` — Ergonomic `ActionLike` type for zero-arg builtins (referenced above but not subsumed)
- `INLINE_BUILTINS.md` — Execution model (resolve builtins in advance phase), not API surface
- `UNION_POSTFIX_DISPATCH.md` — Runtime dispatch mechanism for `.map()` etc. across union families
- `VOID_INPUTS.md` — Type convention for pipeline-ignoring actions
