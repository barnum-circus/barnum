# Trait Dispatch and Iterators

## Context

Dynamic dispatch in barnum uses **prefix-based dispatch** via the `ExtractPrefix` builtin and `matchPrefix` combinator. Tagged union values carry namespaced kind strings (`"Option.Some"`, `"Result.Ok"`). `ExtractPrefix` splits on `'.'` to restructure the value so `branch()` can dispatch on the family first, then the variant. No runtime dispatch tables — the AST encodes the dispatch.

Currently, transformation methods like `.map()` and `.andThen()` are postfix methods on TypedAction that use `matchPrefix` to dispatch across Option and Result.

**This doc introduces Iterator as an additional transformation interface.** Option and Result keep their existing `.map()`, `.andThen()`, etc. methods. Iterator adds sequence-oriented methods (map, flatMap, filter, collect) that don't belong on Option/Result. Option, Result, and arrays gain `.iterate()` postfix to enter Iterator.

---

## Design: Iterator alongside Option/Result

**Option/Result keep their existing methods.** `.map()`, `.andThen()`, `.unwrapOr()`, etc. remain as postfix methods dispatching via `matchPrefix`. They are the primary API for working with Option and Result values.

**Iterator adds sequence operations.** Methods like `.filter()` and `.collect()` only make sense on sequences. Iterator is the interface for these.

**`.iterate()` bridges into Iterator.** Postfix on Option, Result, and arrays. Use it when you need sequence operations:

```ts
// Option/Result methods still work as before:
option.map(validate).unwrapOr(defaultValue)
result.map(transform).unwrapOr(fallback)

// Enter Iterator when you need sequence ops:
option.iterate().filter(pred).collect()
array.iterate().map(transform).filter(pred).collect()
```

### Postfix methods by family

**Option postfix methods** (unchanged):
- `.map(f)` — `Option<T> → Option<U>`
- `.andThen(f)` — `Option<T> → Option<U>`
- `.unwrapOr(default)` — exit Option
- `.unwrap()` — exit Option (panic on None)
- `.isSome()` / `.isNone()` — query
- `.transpose()` — `Option<Result<T,E>> → Result<Option<T>,E>`
- `.iterate()` — enter Iterator

**Result postfix methods** (unchanged):
- `.map(f)` — `Result<T,E> → Result<U,E>`
- `.andThen(f)` — `Result<T,E> → Result<U,E>`
- `.unwrapOr(default)` — exit Result
- `.unwrap()` — exit Result (panic on Err)
- `.mapErr(f)` — transform error variant
- `.or(fallback)` — recover from Err
- `.isOk()` / `.isErr()` — query
- `.transpose()` — `Result<Option<T>,E> → Option<Result<T,E>>`
- `.iterate()` — enter Iterator

**Array postfix methods:**
- `.iterate()` — enter Iterator

**Iterator postfix methods** (new):
- `.map(f)` — `Iterator<T> → Iterator<U>`
- `.flatMap(f)` — `Iterator<T> → Iterator<U>` where `f` returns any IntoIterator type
- `.filter(pred)` — `Iterator<T> → Iterator<T>` (pred: `T → bool`)
- `.collect()` — `Iterator<T> → T[]`

---

## Iterator<T> — a wrapper type with its own prefix

### What is Iterator<T>?

`Iterator<T>` is a tagged union wrapper — like Option and Result — with namespaced kind: `"Iterator.Iterator"`. `.iterate()` converts Option/Result/arrays into an Iterator. Iterator is where sequence-oriented transformation methods live.

### Runtime representation — tagged wrapper

```ts
type IteratorDef<TElement> = { Iterator: TElement[] };
type Iterator<TElement> = TaggedUnion<"Iterator", IteratorDef<TElement>>;
// Runtime: { kind: "Iterator.Iterator", value: [1, 2, 3] }
```

This means:
- `Iterator.fromArray()` wraps the array: `[1, 2, 3]` → `{ kind: "Iterator.Iterator", value: [1, 2, 3] }`
- Iterator methods operate on `.value` (the inner array), then re-wrap
- `.collect()` unwraps: `{ kind: "Iterator.Iterator", value: [1, 2, 3] }` → `[1, 2, 3]`

**Note on single-variant representation:** `"Iterator.Iterator"` is redundant — the prefix is the only thing that matters for dispatch, and there's no second variant to distinguish. The `.Iterator` suffix exists solely to fit the `TaggedUnion<Name, Def>` pattern. Consistency with Option/Result has value — `branch()` works on it, and the Rust engine treats it uniformly.

Why tagged wrapper over phantom brand:
- Consistent with every other barnum type (Option, Result, all TaggedUnion)
- `.branch()` works on it (you can pattern-match on `{ kind: "Iterator.Iterator" }`)
- The Rust engine can recognize and optimize it
- Handlers that receive an Iterator see a proper `{ kind, value }` object, not a bare array that happens to be branded

The wrap/unwrap overhead is real but small — it's a Rust builtin (WrapInField/GetField), not a subprocess call.

### IntoIterator — conversion to Iterator

| Self type | Postfix | Standalone | Runtime behavior |
|-----------|---------|------------|------------------|
| `Option<T>` | `.iterate()` | `Iterator.fromOption()` | Branch: Some → `[value]`, None → `[]`, then wrap |
| `Result<T, E>` | `.iterate()` | `Iterator.fromResult()` | Branch: Ok → `[value]`, Err → `[]`, then wrap |
| `T[]` | `.iterate()` | `Iterator.fromArray()` | Wrap in `{ kind: "Iterator.Iterator", value: array }` |

`.iterate()` is a postfix method that uses `matchPrefix` for all three families. The standalone constructors (`Iterator.fromArray()`, `Iterator.fromOption()`, `Iterator.fromResult()`) are also available when you need to construct an Iterator without a preceding chain. For arrays, `ExtractPrefix` produces `{ kind: "Array", value: array }` as a fallback when the input has no `kind` field (see Task 1.6).

### IntoIterator for `.flatMap()` return types

Iterator's `.flatMap(f)` accepts any function whose return type is "IntoIterator" — meaning the return value can be normalized to an array. This mirrors Rust's `flat_map` which takes `FnMut(T) -> impl IntoIterator<Item=U>`.

Supported return types for `f`:
- `Iterator<U>` — unwrap `.value`
- `Option<U>` — Some → `[value]`, None → `[]`
- `Result<U, E>` — Ok → `[value]`, Err → `[]`
- `U[]` — identity (via Array ExtractPrefix fallback)

Implementation: after calling `f`, normalize the return value via `matchPrefix`:

```ts
// Conceptual implementation of flatMap's inner transform:
chain(action, matchPrefix({
  Iterator: branch({ Iterator: identity() }),  // unwrap value (auto-unwrap)
  Option: branch({ Some: wrapInArray(), None: constant([]) }),
  Result: branch({ Ok: wrapInArray(), Err: constant([]) }),
  Array: identity(),  // already an array
}))
```

Type-level: four overloads on `.flatMap()` for Iterator:

```ts
flatMap<TIn, T, U>(this: TypedAction<TIn, Iterator<T>>, action: Pipeable<T, Iterator<U>>): TypedAction<TIn, Iterator<U>>;
flatMap<TIn, T, U>(this: TypedAction<TIn, Iterator<T>>, action: Pipeable<T, Option<U>>): TypedAction<TIn, Iterator<U>>;
flatMap<TIn, T, U, E>(this: TypedAction<TIn, Iterator<T>>, action: Pipeable<T, Result<U, E>>): TypedAction<TIn, Iterator<U>>;
flatMap<TIn, T, U>(this: TypedAction<TIn, Iterator<T>>, action: Pipeable<T, U[]>): TypedAction<TIn, Iterator<U>>;
```

---

## Iterator methods

All Iterator methods unwrap `{ kind: "Iterator.Iterator", value: T[] }` → operate on `T[]` → re-wrap (for methods that stay in Iterator) or unwrap (for `.collect()`). The pattern is: `getField("value")` → array operation → `Iterator.fromArray()`.

### Phase 1 — implement now (used in demos)

| Method | Rust equivalent | Signature | Implementation | Notes |
|--------|----------------|-----------|----------------|-------|
| `.map(f)` | `Iterator::map` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(f)` → rewrap | Per-element transform |
| `.flatMap(f)` | `Iterator::flat_map` | `Iterator<T> → Iterator<U>` | Unwrap → `forEach(chain(f, intoIteratorNormalize))` → flatten → rewrap | `f` returns any IntoIterator type |
| `.filter(pred)` | `Iterator::filter` | `Iterator<T> → Iterator<T>` | New `Filter` builtin | pred: `T → bool`. New Rust builtin. |
| `.collect()` | `Iterator::collect` | `Iterator<T> → T[]` | Unwrap (getField("value")) | Exit Iterator |

Future Iterator methods are cataloged in `ITERATOR_METHODS.md`.

---

## Example chains

```ts
// Option — use existing methods for simple transforms:
option.map(validate).unwrapOr(defaultValue)

// Result — use existing methods:
result.map(transform).unwrapOr(fallback)

// Array → Iterator → transform → collect:
users.iterate()                              // Iterator<User>
  .map(getName)                              // Iterator<string>
  .collect()                                 // string[]

// flatMap with IntoIterator returns:
files.iterate()                              // Iterator<File>
  .flatMap(analyze)                          // analyze: File → Refactor[] (array is IntoIterator)
  .collect()                                 // Refactor[]

option.iterate()                             // Iterator<Request>
  .flatMap(validate)                         // validate: Request → Result<Response, Error>
  .collect()                                 // Response[] (Errs dropped)
```

---

## What Iterator adds

1. **Sequence operations.** Methods like `.filter()` that only make sense on sequences. These don't belong on Option/Result.

2. **IntoIterator for `.flatMap()`.** The callback can return Option, Result, array, or Iterator — all normalized to Iterator. Mirrors Rust's `flat_map` with `impl IntoIterator`.

3. **Uniform entry point.** `.iterate()` on Option, Result, and arrays. One method to enter the sequence world from any starting type.

Future methods are cataloged in `ITERATOR_METHODS.md`.

---

## Open questions

1. ~~**Naming**~~ **Decided:** `.iterate()`.

2. ~~**Array → Iterator**~~ **Decided:** Postfix `.iterate()` works on all three families (Option, Result, arrays). `ExtractPrefix` is extended to produce `{ kind: "Array", value: input }` when the input has no `kind` field. This lets `matchPrefix` dispatch arrays to an `Array` case alongside Option/Result.

3. ~~**`filter` predicate type**~~ **Decided:** `T → bool`. New `Filter` Rust builtin. Consistent with Rust's `Iterator::filter`.

4. ~~**Short-circuit semantics**~~ **Not an issue now:** All Phase 1 methods (map, flatMap, filter, collect) are inherently non-short-circuiting.

5. ~~**`chain` naming collision**~~ **Not an issue:** barnum's `chain()` is an internal combinator; users see `.then()`. Iterator can use `.chain()` for concatenation without ambiguity.

---

## ForEach AST node

`ForEach` is a fundamental AST node — `{ kind: "ForEach", action: Action }` applies an action to every element of an array. It's how the Rust engine does element-wise operations. Exposed as a standalone combinator: `forEach(action)` — `TypedAction<T[], U[]>`. Used internally by Iterator's `.map()`, `.flatMap()`, and `.filter()`.

The postfix `array.forEach(f)` method is removed — use `array.iterate().map(f).collect()` instead.

---

## Demo migration plan

Demos adopt Iterator, replacing postfix `.forEach()` with `.iterate().map()`. The standalone `forEach` combinator remains as an internal primitive.

### `identify-and-address-refactors/run.ts`

```ts
// BEFORE (line 54): forEach + flatten for flat-map
forEach(analyze).flatten(),

// AFTER: flatMap — analyze returns Refactor[] which is IntoIterator
constant({ folder: srcDir })
  .then(listTargetFiles)
  .iterate()                                    // T[] → Iterator<T>
  .flatMap(analyze)                             // each file → Refactor[] (IntoIterator), concatenated

// BEFORE (line 57): forEach + Option.collect for filter
forEach(assessWorthiness).then(Option.collect()),

// AFTER: filter with bool predicate (or compose: .filter(chain(assessWorthiness, Option.isSome())))
  .filter(assessWorthiness)
```

Full pipeline becomes:
```ts
constant({ folder: srcDir })
  .then(listTargetFiles)
  .iterate()                                    // T[] → Iterator<T>
  .flatMap(analyze)                             // each file → Refactor[], concatenated (IntoIterator)
  .filter(assessWorthiness)                     // keep only worthwhile (bool predicate)
  .map(withResource({
    create: createBranchWorktree,
    action: implementAndReview,
    dispose: deleteWorktree,
  }))
  .collect()                                    // Iterator<T> → T[]
```

### `convert-folder-to-ts/run.ts`

```ts
// BEFORE (line 26):
listFiles.forEach(migrate({ to: "Typescript" })).drop(),

// AFTER:
listFiles.iterate().map(migrate({ to: "Typescript" })).collect().drop(),
```

### `simple-workflow/run.ts`

```ts
// BEFORE (lines 17-27):
listFiles.forEach(pipe(implementRefactor, typeCheckFiles, ...)),

// AFTER:
listFiles.iterate().map(
  implementRefactor
    .then(typeCheckFiles)
    .then(fixTypeErrors)
    .then(commitChanges)
    .then(createPullRequest),
).collect(),
```

### `babysit-prs/run.ts`

```ts
// BEFORE (lines 44-56):
forEach(bindInput<number>((prNumber) => prNumber.then(checkPR).branch({
  ChecksFailed: fixIssues.drop().then(prNumber).some(),
  ChecksPassed: landPR.drop().then(Option.none()),
  Landed: drop.then(Option.none()),
}))),
Option.collect<number>(),

// AFTER:
Iterator.fromArray<number>()
  .filter(
    bindInput<number>((prNumber) =>
      prNumber.then(checkPR).branch({
        ChecksFailed: fixIssues.drop().then(constant(true)),
        ChecksPassed: landPR.drop().then(constant(false)),
        Landed: drop.then(constant(false)),
      }),
    ),
  )
  .collect()
```

### `*/handlers/type-check-fix.ts` (both demos)

```ts
// BEFORE:
HasErrors: forEach(fix).drop().then(recur),

// AFTER:
HasErrors: Iterator.fromArray<TypeError>().then(Iterator.map(fix)).drop().then(recur),
```

---

## Implementation tasks

Per `refactors/PROCESS.md`, every task follows test-first: failing test → implement → remove failure marker (3 commits).

### What needs new builtins vs what composes from existing primitives

**No new builtins needed for Phase 1 except `AsOption` (bool → Option<void>), which is a prerequisite for `filter` but defined separately.**

| Method | Implementation |
|--------|----------------|
| `Iterator.fromArray()` | `tag("Iterator", "Iterator")` — reuses existing `tag` |
| `Iterator.collect()` | `getField("value")` — reuses existing `getField` |
| `Iterator.map(f)` | `getField("value")` → `forEach(f)` → `tag("Iterator", "Iterator")` |
| `Iterator.flatMap(f)` | `getField("value")` → `forEach(chain(f, intoIteratorNormalize))` → `flatten()` → `tag("Iterator", "Iterator")` |
| `Iterator.filter(pred)` | Implemented as `flatMap` — converts bool to `Option<T>` via `AsOption`, flatMap normalizes Option via IntoIterator. |
| `.iterate()` postfix | `matchPrefix` → branch per family → wrap |

**`wrapInArray()`**: `T → T[]`. Implemented as `all(identity())` — may warrant a dedicated builtin later.

**`intoIteratorNormalize`**: `matchPrefix` that converts any IntoIterator return to a plain array. Used inside `.flatMap()`.

```ts
function wrapInArray<T>(): TypedAction<T, T[]> {
  return all(identity()) as TypedAction<T, T[]>;
}

const intoIteratorNormalize = matchPrefix({
  Iterator: branch({ Iterator: identity() }),     // unwrap → T[]
  Option: branch({ Some: wrapInArray(), None: constant([]) }),
  Result: branch({ Ok: wrapInArray(), Err: constant([]) }),
  Array: identity(),                              // already T[]
});
```

---

### Phase 1: Iterator foundation

#### Task 1: ExtractPrefix Array fallback

**Goal:** `ExtractPrefix` produces `{ kind: "Array", value: input }` when the input has no `kind` field. This enables `matchPrefix` to dispatch arrays alongside Option/Result/Iterator.

**File:** `crates/barnum_builtins/src/lib.rs` (in `ExtractPrefix` handler)

Currently, `ExtractPrefix` expects a `kind` field and errors if missing. Add fallback: if no `kind` field, produce `{ kind: "Array", value: input }`.

**File:** `libs/barnum/src/builtins/tagged-union.ts` (TypeScript `extractPrefix`)

Same fallback for the TypeScript runtime.

---

#### Task 2: Add `Iterator` types and namespace (TypeScript)

**Goal:** Define types and the `Iterator` namespace with standalone combinators.

##### 3.1: Add types to `ast.ts`

```ts
export type IteratorDef<TElement> = { Iterator: TElement[] };
export type Iterator<TElement> = TaggedUnion<"Iterator", IteratorDef<TElement>>;
```

##### 3.2: Create `iterator.ts`

The `Iterator` namespace with standalone combinators: `fromArray`, `fromOption`, `fromResult`, `collect`, `map`, `flatMap`, `filter`.

```ts
export const Iterator = {
  fromArray<TElement>(): TypedAction<TElement[], IteratorT<TElement>> {
    return tag<"Iterator", IteratorDef<TElement>, "Iterator">("Iterator", "Iterator");
  },

  collect<TElement>(): TypedAction<IteratorT<TElement>, TElement[]> {
    return getField("value") as TypedAction<IteratorT<TElement>, TElement[]>;
  },

  map<TIn, TOut>(action: Pipeable<TIn, TOut>): TypedAction<IteratorT<TIn>, IteratorT<TOut>> {
    return chain(
      toAction(getField("value")),
      chain(toAction(forEach(action)), toAction(tag("Iterator", "Iterator"))),
    ) as TypedAction<IteratorT<TIn>, IteratorT<TOut>>;
  },

  flatMap<TIn, TOut>(
    action: Pipeable<TIn, unknown>,
  ): TypedAction<IteratorT<TIn>, IteratorT<TOut>> {
    return chain(
      toAction(getField("value")),
      chain(
        toAction(forEach(chain(toAction(action), toAction(intoIteratorNormalize)))),
        chain(toAction(flatten()), toAction(tag("Iterator", "Iterator"))),
      ),
    ) as TypedAction<IteratorT<TIn>, IteratorT<TOut>>;
  },

  filter<TElement>(
    predicate: Pipeable<TElement, boolean>,
  ): TypedAction<IteratorT<TElement>, IteratorT<TElement>> {
    // Implemented as flatMap where f returns Option<T>.
    // pred → asOption → branch to Option<T> → flatMap normalizes via IntoIterator.
    return Iterator.flatMap(
      bindInput((element) =>
        element.then(predicate).asOption().branch({
          Some: element.some(),
          None: Option.none(),
        })
      ),
    ) as TypedAction<IteratorT<TElement>, IteratorT<TElement>>;
  },
} as const;
```

Where `intoIteratorNormalize` is a module-level constant:

```ts
const intoIteratorNormalize: Action = matchPrefix({
  Iterator: branch({ Iterator: identity() }),
  Option: branch({ Some: wrapInArray(), None: constant([]) }),
  Result: branch({ Ok: wrapInArray(), Err: constant([]) }),
  Array: identity(),
});
```

##### 3.3: Export from `index.ts`

---

#### Task 3: Add `.iterate()` postfix method (TypeScript)

**Goal:** Postfix `.iterate()` on Option, Result, and arrays.

##### 4.1: Add type signatures to `TypedAction`

Three overloads:

```ts
iterate<TIn, TElement>(
  this: TypedAction<TIn, Option<TElement>>,
): TypedAction<TIn, Iterator<TElement>>;
iterate<TIn, TElement, TError>(
  this: TypedAction<TIn, Result<TElement, TError>>,
): TypedAction<TIn, Iterator<TElement>>;
iterate<TIn, TElement>(
  this: TypedAction<TIn, TElement[]>,
): TypedAction<TIn, Iterator<TElement>>;
```

##### 4.2: Add method implementation

```ts
function iterateMethod(this: TypedAction): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Option: branch({
      Some: chain(toAction(wrapInArray()), toAction(tag("Iterator", "Iterator"))),
      None: chain(toAction(constant([])), toAction(tag("Iterator", "Iterator"))),
    }),
    Result: branch({
      Ok: chain(toAction(wrapInArray()), toAction(tag("Iterator", "Iterator"))),
      Err: chain(toAction(constant([])), toAction(tag("Iterator", "Iterator"))),
    }),
    Array: tag("Iterator", "Iterator"),
  })));
}
```

##### 4.3: Register in `typedAction()`

---

#### Task 4: Add Iterator postfix methods (TypeScript)

**Goal:** `.map()`, `.flatMap()`, `.filter()`, `.collect()` as postfix methods when output is `Iterator<T>`.

##### 5.1: Add type signatures

Add Iterator overload to existing `.map()`, and add new `.flatMap()` method (Iterator-only):

```ts
// Iterator .map overload:
map<TIn, TElement, TOut>(
  this: TypedAction<TIn, Iterator<TElement>>,
  action: Pipeable<TElement, TOut>,
): TypedAction<TIn, Iterator<TOut>>;

// Iterator .flatMap overloads (IntoIterator return types):
flatMap<TIn, TElement, TOut>(
  this: TypedAction<TIn, Iterator<TElement>>,
  action: Pipeable<TElement, Iterator<TOut>>,
): TypedAction<TIn, Iterator<TOut>>;
flatMap<TIn, TElement, TOut>(
  this: TypedAction<TIn, Iterator<TElement>>,
  action: Pipeable<TElement, Option<TOut>>,
): TypedAction<TIn, Iterator<TOut>>;
flatMap<TIn, TElement, TOut, TError>(
  this: TypedAction<TIn, Iterator<TElement>>,
  action: Pipeable<TElement, Result<TOut, TError>>,
): TypedAction<TIn, Iterator<TOut>>;
flatMap<TIn, TElement, TOut>(
  this: TypedAction<TIn, Iterator<TElement>>,
  action: Pipeable<TElement, TOut[]>,
): TypedAction<TIn, Iterator<TOut>>;

// Iterator .filter (bool predicate, Iterator-only):
filter<TIn, TElement>(
  this: TypedAction<TIn, Iterator<TElement>>,
  predicate: Pipeable<TElement, boolean>,
): TypedAction<TIn, Iterator<TElement>>;

// Iterator .collect overload:
collect<TIn, TElement>(
  this: TypedAction<TIn, Iterator<TElement>>,
): TypedAction<TIn, TElement[]>;
```

##### 5.2: Extend method implementations

Add `Iterator` case to `matchPrefix` in `mapMethod`, `collectMethod`. Add new `flatMapMethod` and `filterMethod` for Iterator:

```ts
// mapMethod — add Iterator case:
Iterator: branch({
  Iterator: chain(toAction(forEach(action)), toAction(tag("Iterator", "Iterator"))),
}),

// flatMapMethod — new method, Iterator-only:
function flatMapMethod(this: TypedAction, action: Pipeable): TypedAction {
  return chain(toAction(this), toAction(matchPrefix({
    Iterator: branch({
      Iterator: chain(
        toAction(forEach(chain(toAction(action), toAction(intoIteratorNormalize)))),
        chain(toAction(flatten()), toAction(tag("Iterator", "Iterator"))),
      ),
    }),
  })));
}

// filterMethod — implemented as flatMap with bool → Option<T> conversion:
function filterMethod(this: TypedAction, predicate: Pipeable): TypedAction {
  return chain(toAction(this), toAction(Iterator.filter(predicate)));
}

// collectMethod — add Iterator case:
Iterator: branch({ Iterator: identity() }),
```

---

#### Task 5: Tests

**File:** `libs/barnum/tests/iterator.test.ts` (new file)

**Type tests:**
- `Iterator.fromArray()` — input `T[]`, output `Iterator<T>`
- `Iterator.collect()` — input `Iterator<T>`, output `T[]`
- `Iterator.map(f)` — input `Iterator<T>`, output `Iterator<U>`
- `Iterator.flatMap(f)` — input `Iterator<T>`, output `Iterator<U>` for each IntoIterator return type
- `Iterator.filter(pred)` — input `Iterator<T>`, output `Iterator<T>`
- Postfix `.iterate()` on Option, Result, array
- Postfix `.map(f)`, `.flatMap(f)`, `.filter(pred)`, `.collect()` on Iterator output

**Execution tests:**
- `Iterator.fromArray()` wraps array
- `Iterator.collect()` unwraps
- Round-trip: `pipe(constant([1,2,3]), Iterator.fromArray(), Iterator.collect())` → `[1,2,3]`
- `Iterator.map(f)` transforms each element
- `Iterator.flatMap(f)` where f returns Iterator — flat-maps
- `Iterator.flatMap(f)` where f returns Option — Some kept, None dropped
- `Iterator.flatMap(f)` where f returns Result — Ok kept, Err dropped
- `Iterator.flatMap(f)` where f returns array — concatenated
- `Iterator.filter(pred)` keeps true, discards false
- `.iterate()` on Some → Iterator with one element
- `.iterate()` on None → empty Iterator
- `.iterate()` on Ok → Iterator with one element
- `.iterate()` on Err → empty Iterator
- `.iterate()` on array → Iterator wrapping array
- Full chain: `option.iterate().map(f).collect()`
- Full chain: `array.iterate().filter(pred).collect()`
- Full chain: `array.iterate().flatMap(f_returning_option).collect()`

---

### Phase 2: Demo migration

Migrate all demos to use Iterator patterns. Demos are first-class artifacts that must reflect best practices — no legacy `forEach` patterns should remain.
