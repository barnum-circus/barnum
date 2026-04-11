# Barnum JS Cleanup: Clean Internals, Consistent Names

## Motivation

The barnum TypeScript library has accumulated inconsistencies, raw AST construction, naming mismatches, and missing operations. This document organizes the cleanup into tiers from smallest to largest.

The overarching goals:

1. **Clean internals** — no raw AST construction, no `as Action` casts, library eats its own dog food
2. **Consistent names** — both identifier naming and conceptual consistency across the API

---

## Tier 1: Trivial fixes (one-liner each)

### 1.1 Remove `taggedUnionSchema` 2-variant minimum

**File:** `libs/barnum/src/builtins.ts:56-58`

```ts
// Delete this:
if (variants.length < 2) {
  throw new Error("taggedUnionSchema requires at least 2 variants");
}
```

This exists because `z.discriminatedUnion` requires 2+ members. The fix is to handle 1-variant unions as a plain `z.object({ kind: z.literal(k), value: schema })` instead of punting. Zero variants can remain an error (that's genuinely nonsensical).

### 1.2 `ReadonlyArray` everywhere

The codebase uses `T[]` shorthand in type signatures. Replace with `ReadonlyArray<T>` in all type positions (function signatures, type aliases, interface fields). The `T[]` shorthand is fine for runtime array construction, but type signatures should use `ReadonlyArray<T>` to signal that the array is not mutated.

Affects: `ast.ts`, `builtins.ts`, `all.ts`, `pipe.ts`, `bind.ts`, `race.ts`, `handler.ts`.

### 1.3 VoidToNull JSDoc placement

**File:** `libs/barnum/src/ast.ts:318-325`

The `VoidToNull` type utility is an internal implementation detail, but its behavior (void variants become `null` at runtime) is user-visible. The JSDoc explaining this mapping should be on the public `TaggedUnion` type and on `Option`/`Result` type aliases, not buried on the internal utility type.

### 1.4 `UNUSED_STATE` sentinel

**File:** `libs/barnum/src/recursive.ts:34`

```ts
const UNUSED_STATE: any = undefined;
```

This is `undefined as any` with a name. It's used as the initial state for the ResumeHandle in `defineRecursiveFunctions`. The `any` hides what this actually is — it should be `null` with a real type, or the ResumeHandle API should support stateless handlers (no initial state required).

### 1.5 Global mutable effect ID counter

**File:** `libs/barnum/src/effect-id.ts:15`

```ts
let nextId = 0;
```

Module-level mutable state with `resetEffectIdCounter()` exported for test isolation. This is a code smell — tests that forget to call `resetEffectIdCounter()` in `beforeEach` produce non-deterministic IDs. The counter should be scoped to a builder/context object, or tests should not depend on specific ID values.

### 1.6 Tests import from internal modules instead of index

**Files:** `tests/patterns.test.ts:2-21`, `tests/round-trip.test.ts:6-15`

Tests import from `../src/ast.js` and `../src/builtins.js` separately instead of from `../src/index.js`. This means the barrel export isn't being tested as a public API surface, and tests reach into internals unnecessarily. The split imports exist because `resetEffectIdCounter` is only exported from `ast.ts` (via re-export from `effect-id.ts`), not from `index.ts` — which is itself a problem (test-only function leaking into the public module).

---

## Tier 2: Stop constructing raw AST

The single biggest internal consistency issue. Throughout `builtins.ts`, `ast.ts`, and `race.ts`, code constructs Chain/Invoke/All nodes as raw object literals with `as Action` casts, bypassing the library's own `chain()`, `all()`, `constant()`, `identity()`, `drop`, etc.

### The problem

```ts
// builtins.ts:200-208 — dropResult
return typedAction({
  kind: "Chain",
  first: action as Action,    // WTF
  rest: {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Drop" } },
  },
});
```

Should be: `return chain(action, drop);`

```ts
// builtins.ts:310-336 — tap
return typedAction({
  kind: "Chain",
  first: {
    kind: "All",
    actions: [
      {
        kind: "Chain",
        first: action as Action,
        rest: {
          kind: "Invoke",
          handler: {
            kind: "Builtin",
            builtin: { kind: "Constant", value: {} },  // ???
          },
        },
      },
      // ...
```

Should be: `return chain(all(chain(action, constant({})), identity()), merge());`

Or better yet, since `constant({})` is a hack to produce an empty object for merge:
`return chain(all(dropResult(action).then(constant({})), identity()), merge())`

### The fix

1. **Move `chain()` and `typedAction()` into a leaf file** (e.g. `core.ts`) that has zero imports from `builtins.ts`. This breaks the circular dependency that forces raw AST construction.
2. **Rewrite every raw AST construction** in `builtins.ts`, `ast.ts`, `race.ts`, and `try-catch.ts` to use `chain()`, `all()`, `constant()`, `identity()`, `drop`, `tag()`, `getField()`, `getIndex()`, `merge()`, `flatten()`, `wrapInField()`.
3. **Delete all shared AST constants** (`TAG_SOME`, `TAG_NONE`, `TAG_OK`, `TAG_ERR`, `EXTRACT_VALUE`, `DROP`, `IDENTITY`, `TAG_CONTINUE`, `TAG_BREAK`, `EXTRACT_PAYLOAD`). These are just memoized raw AST — replace with function calls. If performance of repeated construction matters (it doesn't — these are object allocations at definition time), memoize at the function level.

### Locations that need rewriting

| File | Lines | What | Replacement |
|------|-------|------|-------------|
| `builtins.ts:200-208` | `dropResult` | `chain(action, drop)` |
| `builtins.ts:245-288` | `withResource` | Rewrite using `chain()`, `all()`, etc. |
| `builtins.ts:310-337` | `tap` | `chain(all(chain(action, constant({})), identity()), merge())` |
| `builtins.ts:424-446` | Shared AST constants | Delete, use function calls |
| `builtins.ts:449-457` | `optionBranch` | Use `chain()` and `getField()` |
| `builtins.ts:489-496` | `Option.map` | Use `chain()` |
| `builtins.ts:527-538` | `Option.unwrapOr` | Use `chain()`, `branch()` |
| `builtins.ts:588-601` | `Option.isSome` / `isNone` | Use `chain()`, `constant()` |
| `builtins.ts:659-667` | `resultBranch` | Use `chain()` and `getField()` |
| `builtins.ts:693-719` | Result combinators | Use `chain()` |
| `builtins.ts:832-851` | `Result.transpose` | Use `chain()`, `branch()` |
| `ast.ts:382-602` | All postfix method implementations | Use `chain()` and builtin functions |
| `ast.ts:750-764` | `TAG_CONTINUE`, `TAG_BREAK`, `IDENTITY` | Delete |
| `ast.ts:853-873` | `buildRestartBranchAction` | Use `chain()`, `branch()` |
| `ast.ts:891-914` | `loop` internals | Use `chain()`, `tag()` |
| `race.ts:20-41` | Shared AST constants | Delete |
| `race.ts:65-82` | `race` | Use `chain()`, `all()` |
| `race.ts:153-183` | `withTimeout` | Use `chain()`, `all()`, `tag()` |
| `try-catch.ts:42-46` | `throwError` construction | Uses `TAG_BREAK` constant, raw `RestartPerform` node |
| `recursive.ts:72-73` | Call token construction | `chain(tag(...), resumePerform as any) as Action` |
| `recursive.ts:84-87` | Branch case bodies | `chain(getField("value"), bodyActions[i] as any) as Action` |
| `recursive.ts:95-103` | ResumeHandle construction | Raw `ResumeHandle` node with `as Action` casts everywhere |
| `bind.ts:66-96` | `readVar` function | 30 lines of raw AST for `all(chain(getIndex(1), getIndex(n)), getIndex(1))` |
| `bind.ts:145-155` | Inner chain construction | Raw `Chain` + `Invoke` + `Builtin` + `GetIndex` for `chain(getIndex(n), body)` |
| `bind.ts:157-163` | Nested ResumeHandle | Raw `ResumeHandle` nodes in a loop |
| `bind.ts:166-174` | Outer chain+all | Raw `Chain` + `All` wrapping |
| `pipe.ts:90-92` | `reduceRight` | `{ kind: "Chain", first, rest }` — should use `chain()` |
| `handler.ts:247-267` | `createHandlerWithConfig` factory | 20 lines of raw `Chain`/`All`/`Invoke`/`Builtin` for `chain(all(identity(), constant(config)), invokeAction)` |
| `run.ts:132` | `runPipeline` | `chain(constant(input) as Pipeable, pipeline as Pipeable) as Action` |

### Duplicated `BodyResult` type

`BodyResult<TOut>` is defined identically in both `recursive.ts:28-31` and `bind.ts:124-127`:

```ts
type BodyResult<TOut> = Action & {
  __out?: () => TOut;
  __out_contra?: (output: TOut) => void;
};
```

Extract to the leaf `core.ts` alongside `chain`/`typedAction`.

---

## Tier 3: Naming and conceptual consistency

### 3.1 Rename `Option.collect()` to `Option.flatten()` (or just `flatten`)

**File:** `libs/barnum/src/builtins.ts:567-578`

`Option.collect()` takes `Option<T>[] -> T[]`. This is "flatten Option over Array" — filter out Nones, unwrap Somes. Calling it "collect" is confusing because Rust's `collect` is a much more general operation.

Better name: `Option.flatten()` — consistent with `Option.flatten()` for `Option<Option<T>> -> Option<T>` and `flatten()` for `T[][] -> T[]`. The operation is "flatten" applied to `Option<T>[]`.

Alternatively, this could be a top-level `flatten` that dispatches based on type, but that's a bigger change (see Tier 4).

### 3.2 Typed builtin fields on Rust side

**File:** `crates/barnum_ast/src/lib.rs:238-306`

Every parameterized builtin uses `value: Value` (i.e., `serde_json::Value`):

```rust
Tag { value: Value }         // should be: Tag { tag: String }
GetField { value: Value }    // should be: GetField { field: String }
GetIndex { value: Value }    // should be: GetIndex { index: usize }
Pick { value: Value }        // should be: Pick { fields: Vec<String> }
WrapInField { value: Value } // should be: WrapInField { field: String }
Sleep { value: Value }       // should be: Sleep { ms: u64 }
Constant { value: Value }    // this one is correct — it IS arbitrary JSON
```

This is lazy deserialization. Use the actual types. The `#[serde(rename = "value")]` attribute can preserve wire compatibility if needed during transition, but since we don't care about backward compat, just change the TS side's `value` key to match the new field names.

### 3.3 Unify `TagContinue`/`TagBreak` into `Tag`

**File:** `crates/barnum_ast/src/lib.rs:272-275`

Rust has `TagContinue` and `TagBreak` as separate `BuiltinKind` variants, but no `TagSome`, `TagNone`, `TagOk`, `TagErr`. Meanwhile, the TS side already uses `Tag { value: "Continue" }` for everything — it doesn't even use `TagContinue`/`TagBreak`.

Delete `TagContinue` and `TagBreak`. Use `Tag { tag: "Continue" }` and `Tag { tag: "Break" }`. One variant, one code path.

### 3.5 `HandlerOutput<void> = never` is surprising

**File:** `libs/barnum/src/handler.ts:95`

```ts
type HandlerOutput<TOutput> = [TOutput] extends [void] ? never : TOutput;
```

The JSDoc says "fire-and-forget handlers compose without `.drop()`" — but `never` as an output type means "this handler never returns," not "this handler returns nothing useful." The correct type for "returns nothing" is `null` (consistent with how `void` maps to `null` elsewhere via `VoidToNull`). A handler that genuinely never returns (infinite loop, throws always) should be `never`. A handler that returns `void` should produce `null`.

### 3.6 `bare T` type parameters in overloads

**Files:** `all.ts`, `pipe.ts`

`all.ts` uses `In, O1, O2, ...` and `pipe.ts` uses `T1, T2, T3, ...`. Neither follows the CLAUDE.md rule of descriptive type parameter names (`TInput`, `TOutput`, etc.). These are overload-heavy files where renaming is mechanical but improves readability at each call site's tooltip.

### 3.4 Rename `CollectSome` builtin

If `Option.collect()` is renamed to `Option.flatten()` (3.1), rename the Rust `CollectSome` builtin to match. Something like `FlattenOption` — it takes `Option<T>[]` and returns `T[]`.

---

## Tier 4: Reduce Rust builtin surface area

The principle: on the Rust side, prefer simple primitives composed in JS over specialized builtins. Perf is irrelevant — these are workflow orchestration steps, not hot loops.

### 4.1 `Pick` → `getField` + `all` + `merge`

`pick("a", "b")` on `{ a: 1, b: 2, c: 3 }` is equivalent to:

```ts
chain(all(getField("a").wrapInField("a"), getField("b").wrapInField("b")), merge())
```

Remove `Pick` from `BuiltinKind`. Implement `pick()` in JS as a composition.

### 4.2 `Tag` → `wrapInField` + `constant` + `all` + `merge`

`tag("Ok")` wraps input as `{ kind: "Ok", value: input }`. This is:

```ts
chain(all(constant("Ok").wrapInField("kind"), identity().wrapInField("value")), merge())
```

Remove `Tag` from `BuiltinKind`. Implement `tag()` in JS as a composition.

**Open question:** Does removing `Tag` and `Pick` from Rust make the AST harder to debug/visualize? The expanded form is noisier. Given that builtins are about to be inlined in advance (per `INLINE_BUILTINS.md`), the expanded form might produce more frames. Worth considering whether the cost is acceptable.

### 4.3 `wrapInArray` is just `all`

If we ever need `wrapInArray(x)` (wrap a single value in an array), it's just `all(identity())` which gives `[input]`. No dedicated builtin needed.

---

## Tier 5: API design changes

### 5.1 Curry `withTimeout`

**Current:** `withTimeout(ms, body)` where `ms: Pipeable<TIn, number>`

**Proposed:** `withTimeout(ms)(body)` — curried, ms first.

```ts
// Before
withTimeout(constant(5000), riskyStep)

// After
withTimeout(5000)(riskyStep)
// or for dynamic timeout:
withTimeout(getTimeoutMs)(riskyStep)
```

The curried form composes better with `withRetries`:

```ts
withRetries(3)(withTimeout(5000)(riskyStep))
```

### 5.2 Curry `withRetries`

`withRetries` doesn't exist yet. Define it as `withRetries(n)(action)`:

```ts
function withRetries<TIn, TOut>(
  count: number,
): (action: Pipeable<TIn, TOut>) => TypedAction<TIn, TOut>
```

Implementation: loop that runs the action, catches errors, decrements counter, recurs. Uses `loop` + `tryCatch` internally.

**Open question:** What does "retry" mean exactly? Does it re-execute the same action with the same input? Does it need a backoff strategy? The simplest version just re-runs with the same input `count` times.

### 5.3 `allObject({ keyName: action, ... })`

JS-land convenience that takes a record of actions and produces a `TypedAction` whose output is the record with each value replaced by its action's output:

```ts
allObject({
  user: getUser,
  settings: getSettings,
  permissions: getPermissions,
})
// Output type: { user: User; settings: Settings; permissions: Permissions }
```

Implementation: extract keys, create `all(...values)`, then zip keys back into an object. Done entirely in JS — desugars to `all` + `merge` + `wrapInField` per entry.

### 5.4 `first` and `last`

Return `Option<T>`, not the `[T, T[]]` tuple that `splitFirst`/`splitLast` produce:

```ts
function first<T>(): TypedAction<ReadonlyArray<T>, Option<T>>
// = chain(splitFirst(), Option.map(getIndex(0)))

function last<T>(): TypedAction<ReadonlyArray<T>, Option<T>>
// = chain(splitLast(), Option.map(getIndex(1)))
```

The `splitFirst`/`splitLast` builtins return `Option<[T, T[]]>` (the element and the rest of the array). `first` and `last` are the common case where you just want the element, not the rest. Compose by mapping over the Option to extract index 0 or 1 from the tuple.

---

## Tier 6: Array/iterable operations

### Needed operations

| Operation | Signature | Notes |
|-----------|-----------|-------|
| `flatten` (array) | `T[][] -> T[]` | Already exists as builtin |
| `flatten` (option) | `Option<Option<T>> -> Option<T>` | Already exists as `Option.flatten()` |
| `flatten` (result) | `Result<Result<V,E>,E> -> Result<V,E>` | Already exists as `Result.flatten()` |
| `flatten` (option array) | `Option<T>[] -> T[]` | Currently `Option.collect()`, rename per 3.1 |
| `map` (array) | `(T -> U) applied to T[] -> U[]` | This is `forEach` — already exists |
| `andThen` / `flatMap` (array) | `(T -> U[]) applied to T[] -> U[]` | `forEach(action).flatten()` — compose, don't add builtin |
| `filter` (array) | predicate-based filtering | `forEach(predicateReturningOption).then(Option.flatten())` |
| `concat` / `append` | `[T[], T[]] -> T[]` | New builtin or `all(identity(), identity()).flatten()` — but that doesn't work for concat of two different arrays. Needs a binary builtin or use `all` + `flatten`. |
| `reverse` | `T[] -> T[]` | New Rust builtin (can't compose from primitives) |

### Filter pattern

Filter is the interesting one. There's no obvious way to express "keep elements matching a predicate" without a dedicated builtin, because the predicate needs to return `Option<T>` (Some to keep, None to discard), and then you flatten the options:

```ts
function filter<T>(predicate: Pipeable<T, Option<T>>): TypedAction<ReadonlyArray<T>, ReadonlyArray<T>> {
  return chain(forEach(predicate), Option.flatten());  // renamed from collect
}
```

The predicate signature `T -> Option<T>` is a bit unusual but consistent — it's `andThen` for Option. The caller wraps the real predicate:

```ts
// Filter files that are .ts
filter(
  // file -> Option<file>
  pipe(getField("ext"), Cmp.eq(".ts"), Bool.branch(Option.some(), pipe(drop, Option.none())))
)
```

This is verbose. A `filterBy` that takes `T -> boolean` and wraps internally would be more ergonomic but requires `Bool.branch` (from `PRIMITIVE_BUILTINS.md`).

### `all` for arrays and options

The user notes that `all` should work for arrays and options naturally. Currently `all` only takes variadic actions. A separate overload or function could accept an `Option<T>[]` or `T[][]` and flatten:

- `all` on `ReadonlyArray<TypedAction<TIn, TOut>>` → `TypedAction<TIn, ReadonlyArray<TOut>>` (run all actions concurrently). This is what `all` already does via variadic args.
- There are only a few "iterable" types: arrays and options. For each, flatten is the natural operation. We can provide four specific flatten overloads rather than a generic mechanism:
  1. `T[][] -> T[]` (array flatten — exists)
  2. `Option<Option<T>> -> Option<T>` (option flatten — exists)
  3. `Result<Result<V,E>,E> -> Result<V,E>` (result flatten — exists)
  4. `Option<T>[] -> T[]` (option-over-array flatten — exists as `CollectSome`, renaming)

---

## Tier 7: Structural / architectural

### 7.1 Three-file builtin definition problem

Adding a new JS builtin requires changes in:

1. `ast.ts` — `BuiltinKind` type union (line 87-101)
2. `builtins.ts` — TypeScript function
3. `index.ts` — re-export
4. `crates/barnum_ast/src/lib.rs` — Rust `BuiltinKind` enum
5. `crates/barnum_builtins/src/lib.rs` — Rust implementation

Five files for a new builtin. The TS side alone is three files.

**Proposed fix (TS side):** The `BuiltinKind` type in `ast.ts` should be inferred from or generated alongside the builtin functions in `builtins.ts`, not maintained separately. Options:

- **Single source of truth in `builtins.ts`:** Each builtin function constructs its AST node. The `BuiltinKind` type in `ast.ts` can be a union derived from what the functions produce. Or simply: don't export `BuiltinKind` as a user-facing type — it's an internal wire format. Move it into a `wire-types.ts` or inline it.
- **Auto-export from `builtins.ts`:** The barrel `index.ts` already re-exports everything from `builtins.ts`. Adding a new builtin to `builtins.ts` with `export` makes it available. The `index.ts` explicit list exists only because `Option` and `Result` need declaration merging. Consider `export * from "./builtins.js"` instead of listing every name.

The Rust side (two files: AST enum + implementation) is harder to consolidate but less painful since Rust changes are less frequent.

### 7.2 Colocate tests

Tests are in `libs/barnum/tests/` instead of next to their source files. This makes it hard to find the test for a given module.

Move to colocated test files: `src/builtins.test.ts`, `src/ast.test.ts`, `src/schema.test.ts`, etc. The `handlers.ts` test helper can stay in a `tests/` dir or become `src/__test__/handlers.ts`.

This applies to both the TS and Rust sides. `patterns.test.ts` is the worst offender — a grab-bag of AST structure tests for pipe, all, branch, loop, bind, forEach, race, tryCatch, etc. all in one file. This makes it impossible to find the tests for a given combinator without searching. Split it: `pipe.test.ts` next to `pipe.ts`, `bind.test.ts` next to `bind.ts`, etc. Each test file tests exactly the module it sits next to. Same principle on Rust: tests for builtin execution should live next to the builtin implementation, not in a separate test directory.

### 7.3 List vs Array naming

The user raises whether "list" is a better name than "array" for the collection type. In JS/TS, `Array` is the native type. Using "list" would create a naming mismatch with the language. However, "list" better captures the semantics (ordered, variable-length, homogeneous) and avoids confusion with fixed-length tuples.

**Recommendation:** Keep "array" for now. The TS ecosystem universally calls it Array/ReadonlyArray. Fighting the language's naming creates confusion. If a standalone syntax emerges (per `STANDALONE_SYNTAX.md`), "list" could be the surface syntax name that compiles to array operations.

---

## Dependency order

Items that must happen before others:

1. **Tier 2 (stop constructing raw AST)** requires extracting `chain`/`typedAction`/`BodyResult` into a dependency-free file first. Once everything uses combinators that return `TypedAction`, the `as any` / `as Action` casts go away too — they exist because raw `Action` construction strips phantom types.
2. **Tier 4 (reduce builtin surface)** should happen after Tier 2 (since the JS implementations will use `chain()` etc.).
3. **Tier 3.2-3.3 (Rust builtin field types, unify Tag variants)** should happen before or alongside Tier 4 (since Tier 4 removes some builtins from Rust).
4. **Tier 5-6 (new API, new operations)** can happen independently of Tiers 2-4 but will be cleaner after them.
5. **Tier 7 (structural)** is independent and can happen anytime.

Suggested execution order: 1 → 2 → 3 → 4 → 5 → 6 → 7.

---

## Overlap with existing refactor docs

- **`PRIMITIVE_BUILTINS.md`** — Covers math, boolean, string, array, object operations. Tier 6 here overlaps with the Array section there. This doc focuses on the *cleanup* of existing operations; PRIMITIVE_BUILTINS covers *adding new* categories.
- **`INLINE_BUILTINS.md`** — Covers executing builtins inline during advance. Tier 4's reduction of Rust builtins reduces the surface area that INLINE_BUILTINS needs to handle.
- **`THUNK_BUILTINS.md`** — Covers accepting `() => TypedAction` in combinators. Orthogonal to this doc.
- **`TS_VS_RUST_TRANSFORMS.md`** — Thought piece on where transforms should live. Tier 4's movement of Pick/Tag to JS-side is consistent with that doc's recommendation for TS-side transforms.
