# Test Restructuring

**Status:** Design — awaiting approval.

## Motivation

Tests are currently split by *kind* — AST structure (`patterns.test.ts`), types (`types.test.ts`), execution (`run.test.ts`), and serialization (`round-trip.test.ts`). This makes it hard to answer "is Option.map fully tested?" because its tests are scattered across three files. Worse, execution test coverage is almost nonexistent: only 7 trivial tests (constant, identity, pipe), and zero execution tests for any combinator, builtin, Option method, or Result method.

## Goals

1. **Organize by module, not by test kind.** Each module (Option, Result, builtins, pipe, etc.) gets one test file containing type tests, AST structure tests, and execution tests.
2. **One test per branch.** Every combinator that dispatches (Option/Result methods, branch, loop) gets one execution test per branch (e.g., Ok case + Err case for `Result.unwrapOr`).
3. **Comprehensive execution coverage.** Every public function and combinator gets at least one execution test through `runPipeline`.

## Current state

| File | Contents | Test count |
|------|----------|------------|
| `patterns.test.ts` | AST structure assertions | ~52 |
| `types.test.ts` | Compile-time type assertions | ~100 |
| `run.test.ts` | Execution via `runPipeline` | 7 |
| `round-trip.test.ts` | Serde round-trip via `barnum check` | 6 |
| `schema.test.ts` | `zodToCheckedJsonSchema` | ~60 |
| `handlers.ts` | Test fixture handlers | (not tests) |

## Proposed structure

Each file below contains **type tests**, **AST structure tests** (where useful), and **execution tests** colocated together.

### Files to create

| File | Module | Migrates from |
|------|--------|---------------|
| `builtins.test.ts` | constant, identity, drop, panic, range, wrapInField, merge, flatten, getField, getIndex, pick, splitFirst, splitLast, first, last, tag, extractPrefix, withResource | types: "builtin types". run: constant/identity tests. patterns: postfix operator AST tests for `.drop()`, `.flatten()`, `.getField()`, `.tag()` |
| `option.test.ts` | Option.some/none, map, andThen, unwrap, unwrapOr, filter, collect, isSome, isNone, transpose. Postfix dispatch for Option. | types: "Option namespace types". patterns: "Option namespace" |
| `result.test.ts` | Result.ok/err, map, mapErr, andThen, or, and, unwrap, unwrapOr, toOption, toOptionErr, transpose, isOk, isErr. Postfix dispatch for Result. | types: "Result types", "Result.unwrapOr with throw tokens". patterns: "Result combinators" |
| `pipe.test.ts` | pipe, chain, .then() | types: "pipe type safety", "combinator types" (pipe subset). patterns: "pipe". run: multi-step pipeline tests. |
| `branch.test.ts` | branch, matchPrefix, postfix .branch() | types: "{ kind, value } convention", "postfix .branch() type safety". patterns: "branch", postfix `.branch()` tests |
| `forEach.test.ts` | forEach, postfix .forEach() | types: "combinator types" (forEach subset). patterns: "forEach" |
| `all.test.ts` | all | types: "combinator types" (all subset). patterns: "all", "reader monad pattern" |
| `loop.test.ts` | loop, recur, earlyReturn | types: "loop type parameter constraints", "combinator types" (loop subset). patterns: "loop" |
| `bind.test.ts` | bind, bindInput, VarRef | types: "bind types", "bindInput types". patterns: "bind", "bindInput" |
| `effects.test.ts` | tryCatch, race, withTimeout, sleep | types: "tryCatch types", "race types", "withTimeout types" |
| `handler.test.ts` | createHandler, createHandlerWithConfig | types: "handler types", "optional handler types: createHandler", "optional handler types: createHandlerWithConfig" |

### Files to keep as-is

| File | Reason |
|------|--------|
| `schema.test.ts` | Already well-organized, self-contained |
| `round-trip.test.ts` | Cross-cutting serialization tests; keep separate |
| `handlers.ts` | Shared fixture handlers |

### Files to delete

| File | Reason |
|------|--------|
| `patterns.test.ts` | All tests migrated to per-module files |
| `types.test.ts` | All tests migrated to per-module files |
| `run.test.ts` | All tests migrated to per-module files |

---

## Test inventory per file

Below: `[E]` = existing test (migrated), `[N]` = new test to add.

### `builtins.test.ts`

**Type tests:**
- `[E]` constant: `any → T`
- `[E]` identity: `T → T`
- `[E]` drop: `any → void`
- `[E]` range: `any → number[]`
- `[E]` flatten: `T[][] → T[]`
- `[E]` getField: `{ key: V } → V`
- `[E]` merge: `[A, B] → A & B`
- `[N]` panic: `any → never`
- `[N]` wrapInField: `T → Record<F, T>`
- `[N]` getIndex: `Tuple → Option<Tuple[N]>`
- `[N]` pick: `Obj → Pick<Obj, Keys>`
- `[N]` splitFirst: `T[] → Option<[T, T[]]>`
- `[N]` splitLast: `T[] → Option<[T[], T]>`
- `[N]` first: `T[] → Option<T>`
- `[N]` last: `T[] → Option<T>`
- `[N]` tag: `T → TaggedUnion<TDef>`
- `[N]` extractPrefix: `TypedAction<any, any>` (untyped — transforms kind string)
- `[N]` withResource: `TIn → TOut` (through create/action/dispose lifecycle)

**AST structure tests:**
- `[E]` `.drop()` produces Chain → Drop
- `[E]` `.flatten()` produces Chain → Flatten
- `[E]` `.getField()` produces Chain → GetField
- `[E]` `.tag()` produces Chain → tag composition
- `[E]` postfix methods are chainable

**Execution tests (all `[N]`):**
- constant(42) returns 42
- constant("hello") returns "hello" *(migrate from run.test.ts)*
- constant({x: 1}) returns object *(migrate from run.test.ts)*
- constant(null) returns null *(migrate from run.test.ts)*
- identity passes through input *(migrate from run.test.ts)*
- drop returns null
- panic halts pipeline (test that runPipeline rejects)
- range(0, 5) returns [0, 1, 2, 3, 4]
- range(3, 3) returns []
- wrapInField("foo")(42) → {foo: 42}
- merge([{a: 1}, {b: 2}]) → {a: 1, b: 2}
- flatten([[1, 2], [3]]) → [1, 2, 3]
- flatten([]) → []
- getField("name")({name: "alice", age: 30}) → "alice"
- getIndex(0) on [10, 20, 30] → Option.Some(10)
- getIndex(5) on [10, 20, 30] → Option.None
- getIndex(0) on [] → Option.None
- pick("a", "b") from {a: 1, b: 2, c: 3} → {a: 1, b: 2}
- splitFirst on [1, 2, 3] → Some([1, [2, 3]])
- splitFirst on [] → None
- splitLast on [1, 2, 3] → Some([[1, 2], 3])
- splitLast on [] → None
- first on [10, 20] → Some(10)
- first on [] → None
- last on [10, 20] → Some(20)
- last on [] → None
- tag("Ok", "Result")(42) → {kind: "Result.Ok", value: 42}
- tag("None", "Option")(null) → {kind: "Option.None", value: null}
- extractPrefix on {kind: "Result.Ok", value: 42} → {kind: "Result", value: {kind: "Result.Ok", value: 42}}
- withResource: create acquires, action runs, dispose cleans up, returns action output

### `option.test.ts`

**Type tests:**
- `[E]` Option.map: `Option<T> → Option<U>`
- `[E]` Option.map composes in pipe
- `[E]` Option.andThen: `Option<T> → Option<U>`
- `[E]` Option.andThen composes in pipe
- `[E]` Option.unwrapOr: `Option<T> → T`
- `[E]` Option.filter: `Option<T> → Option<T>`
- `[E]` Option.collect: `Option<T>[] → T[]`
- `[E]` Option.isSome: `Option<T> → boolean`
- `[E]` Option.isNone: `Option<T> → boolean`
- `[E]` full Option pipeline: construct → map → unwrapOr
- `[E]` forEach + Option.collect pipeline
- `[N]` Option.unwrap: `Option<T> → T`
- `[N]` Option.transpose: `Option<Result<T,E>> → Result<Option<T>,E>`
- `[N]` Option.some: `T → Option<T>`
- `[N]` Option.none: `void → Option<T>`

**AST structure tests:**
- `[E]` Option.map produces Branch with Some/None cases
- `[E]` Option.andThen produces Branch with action Some and tag None
- `[E]` Option.unwrapOr produces Branch with identity Some and default None
- `[E]` Option.filter produces Branch with predicate Some and tag None
- `[E]` Option.collect produces CollectSome builtin
- `[E]` Option.isSome produces Branch with Constant(true)/Constant(false)
- `[E]` Option.isNone is the inverse of isSome

**Execution tests (all `[N]`):**
- Option.some wraps value: `some(42)` → `{kind: "Option.Some", value: 42}`
- Option.none produces None: `none(null)` → `{kind: "Option.None", value: null}`
- Option.map on Some transforms value
- Option.map on None stays None
- Option.andThen on Some, action returns Some → Some
- Option.andThen on Some, action returns None → None
- Option.andThen on None → None
- Option.unwrap on Some extracts value
- Option.unwrap on None panics (runPipeline rejects)
- Option.unwrapOr on Some returns value
- Option.unwrapOr on None runs fallback
- Option.filter on Some where predicate returns Some → keeps
- Option.filter on Some where predicate returns None → drops
- Option.filter on None → None
- Option.collect on [Some(1), None, Some(3)] → [1, 3]
- Option.collect on [] → []
- Option.isSome on Some → true
- Option.isSome on None → false
- Option.isNone on Some → false
- Option.isNone on None → true
- Option.transpose Some(Ok(x)) → Ok(Some(x))
- Option.transpose Some(Err(e)) → Err(e)
- Option.transpose None → Ok(None)
- Postfix .map on Option output dispatches correctly (matchPrefix)
- Postfix .unwrap on Option output
- Postfix .unwrapOr on Option output
- Postfix .andThen on Option output
- Postfix .isSome on Option output
- Postfix .isNone on Option output
- Postfix .collect on Option[] output

### `result.test.ts`

**Type tests:**
- `[E]` Result.map transforms Ok type, preserves Err type
- `[E]` Result.mapErr transforms Err type, preserves Ok type
- `[E]` Result.andThen input is Result, output is Result with new Ok type
- `[E]` Result.or input is Result, output has new Err type
- `[E]` Result.and replaces Ok type, preserves Err type
- `[E]` Result.unwrapOr extracts TValue from Result
- `[E]` Result.toOption converts to Option<TValue>
- `[E]` Result.toOptionErr converts to Option<TError>
- `[E]` Result.transpose swaps Result/Option nesting
- `[E]` Result.isOk returns boolean
- `[E]` Result.isErr returns boolean
- `[E]` Result branches with Ok/Err cases (pipeline test)
- `[E]` Result.unwrapOr accepts throw token with explicit types
- `[E]` .unwrapOr() infers types from this constraint
- `[E]` .unwrapOr() composes in tryCatch pipeline
- `[E]` .unwrapOr() chains into further pipeline steps
- `[E]` .unwrapOr() produces Chain AST node
- `[E]` rejects .unwrapOr() on non-Result output
- `[N]` Result.unwrap: Ok → TValue, Err → panic
- `[N]` Result.ok: `T → Result<T, E>`
- `[N]` Result.err: `E → Result<T, E>`

**AST structure tests:**
- `[E]` Result.map desugars
- `[E]` Result.mapErr desugars
- `[E]` Result.andThen desugars
- `[E]` Result.or desugars
- `[E]` Result.and desugars
- `[E]` Result.unwrapOr desugars
- `[E]` Result.toOption desugars
- `[E]` Result.toOptionErr desugars
- `[E]` Result.isOk desugars
- `[E]` Result.isErr desugars
- `[E]` Result.transpose desugars to nested branches

**Execution tests (all `[N]`):**
- Result.ok wraps value: `ok(42)` → `{kind: "Result.Ok", value: 42}`
- Result.err wraps error: `err("oops")` → `{kind: "Result.Err", value: "oops"}`
- Result.map on Ok transforms value
- Result.map on Err stays Err
- Result.mapErr on Ok stays Ok
- Result.mapErr on Err transforms error
- Result.andThen on Ok chains to inner Result
- Result.andThen on Err propagates
- Result.or on Ok stays Ok
- Result.or on Err applies fallback
- Result.and on Ok replaces with other
- Result.and on Err stays Err
- Result.unwrap on Ok extracts value
- Result.unwrap on Err panics
- Result.unwrapOr on Ok returns value
- Result.unwrapOr on Err runs fallback
- Result.toOption on Ok → Some
- Result.toOption on Err → None
- Result.toOptionErr on Ok → None
- Result.toOptionErr on Err → Some
- Result.transpose Ok(Some(x)) → Some(Ok(x))
- Result.transpose Ok(None) → None
- Result.transpose Err(e) → Some(Err(e))
- Result.isOk on Ok → true
- Result.isOk on Err → false
- Result.isErr on Ok → false
- Result.isErr on Err → true
- Postfix .map on Result output dispatches correctly (matchPrefix)
- Postfix .mapErr on Result output
- Postfix .unwrap on Result output
- Postfix .unwrapOr on Result output
- Postfix .andThen on Result output
- Postfix .or on Result output
- Postfix .toOption on Result output
- Postfix .toOptionErr on Result output
- Postfix .isOk on Result output
- Postfix .isErr on Result output

### `pipe.test.ts`

**Type tests:**
- `[E]` pipe: input of first, output of last
- `[E]` rejects mismatched adjacent types
- `[E]` rejects unrelated types
- `[E]` accepts compatible types
- `[E]` rejects non-exhaustive branch (missing case)
- `[E]` accepts exhaustive branch
- `[E]` config accepts workflows starting with constant
- `[E]` full pipeline: constant → handlers → forEach → loop

**AST structure tests:**
- `[E]` pipe chains setup → build → verify → deploy
- `[E]` pipe chains three steps correctly
- `[E]` pipe rejects mismatched types (ts-expect-error)
- `[E]` pipe rejects unrelated types (ts-expect-error)

**Execution tests:**
- `[E]` multi-step pipeline (constant → setup → build) *(migrate from run.test.ts)*
- `[N]` pipe of 4+ steps through handlers
- `[N]` .then() postfix chains correctly
- `[N]` chain(a, b) is equivalent to pipe(a, b)

### `branch.test.ts`

**Type tests:**
- `[E]` branch: input is discriminated union, output is case union
- `[E]` postfix .branch() type safety (4 tests: non-exhaustive, wrong handler, bare drop, non-discriminated)
- `[E]` { kind, value } convention tests
- `[E]` phantom __def on tagged unions
- `[E]` postfix .branch(): input preserved, output is union
- `[E]` postfix .branch() + .drop() compose

**AST structure tests:**
- `[E]` branch accepts cases with same output type
- `[E]` .branch() produces Chain → Branch AST
- `[E]` postfix .branch() produces valid AST for loop pattern

**Execution tests (all `[N]`):**
- branch dispatches on kind string, extracts value, routes to correct case
- branch with 3+ cases selects the right one
- matchPrefix dispatches Option correctly (prefix = "Option")
- matchPrefix dispatches Result correctly (prefix = "Result")
- postfix .branch() on handler output dispatches correctly

### `forEach.test.ts`

**Type tests:**
- `[E]` forEach wraps input/output in arrays

**AST structure tests:**
- `[E]` forEach produces ForEach AST
- `[E]` forEach composes with pipe

**Execution tests (all `[N]`):**
- forEach maps action over array elements
- forEach on empty array → []
- forEach on single-element array
- forEach composes in pipe (constant → forEach → collect)
- Postfix .forEach() chains correctly

### `all.test.ts`

**Type tests:**
- `[E]` all: same input, tuple output

**AST structure tests:**
- `[E]` all accepts actions with same input type
- `[E]` all rejects actions with different input types
- `[E]` all composes with branch
- `[E]` reader monad: all + identity + merge preserves context

**Execution tests (all `[N]`):**
- all runs actions, returns tuple of results
- all with identity preserves input alongside other action
- all with 3 actions returns 3-tuple

### `loop.test.ts`

**Type tests:**
- `[E]` loop: input matches Continue, output is Break
- `[E]` loop with branch/recur/done: output is null with void defaults
- `[E]` loop with done: zero type params (terminate pattern)
- `[E]` loop<TBreak, TIn>: both explicit (stateful pattern)
- `[E]` without explicit TBreak, done has input=null
- `[E]` without explicit TBreak, done rejects non-null
- `[E]` done and recur output never
- `[E]` recur's input type is TIn
- `[E]` done's input type is TBreak
- `[E]` loop with TIn=void has any input
- `[E]` loop with explicit TIn has exact input
- `[E]` .drop() before recur connects void output

**AST structure tests:**
- `[E]` loop produces Chain(tag(Continue), RestartHandle(...)) AST
- `[E]` loop composes type-check loop with branch

**Execution tests (all `[N]`):**
- loop: body iterates N times then breaks (stateful counter)
- loop: terminate pattern (type-check-fix style)
- earlyReturn: body exits early with value
- earlyReturn: body completes normally without early return
- recur: body restarts with new input

### `bind.test.ts`

**Type tests:**
- `[E]` VarRef output type matches binding output
- `[E]` VarRef pipes into action expecting matching input
- `[E]` VarRef rejects piping into wrong input
- `[E]` multiple bindings infer distinct VarRef types
- `[E]` bind output type matches body output type
- `[E]` bind input type matches binding input type
- `[E]` bindInput: infers VarRef type from explicit type parameter
- `[E]` bindInput: output type matches body return type
- `[E]` bindInput: input type matches TIn parameter
- `[E]` bindInput: body pipeline input is any

**AST structure tests:**
- `[E]` single binding produces Chain(All, ResumeHandle) structure
- `[E]` two bindings produce nested Handles with distinct effectIds
- `[E]` VarRef is a ResumePerform node
- `[E]` resume_handler_ids are unique across bind calls
- `[E]` readVar(n) structure
- `[E]` bindInput compiles to bind([identity], ...)

**Execution tests (all `[N]`):**
- bind with single constant binding: body receives value
- bind with two bindings: body receives both values
- bind: pipeline input is available in body
- bindInput: captured input is available as VarRef
- bindInput: VarRef value pipes into subsequent action

### `effects.test.ts`

**Type tests:**
- `[E]` tryCatch: input from body, output matches body and recovery
- `[E]` throwError token is TypedAction<TError, never>
- `[E]` recovery input type matches throwError payload type
- `[E]` nested tryCatch: each throwError has independent TError
- `[E]` tryCatch produces Chain(Tag(Continue), Handle) AST
- `[E]` race: all branches same input/output
- `[E]` race produces Chain AST
- `[E]` sleep: any → void
- `[E]` sleep produces Invoke AST
- `[E]` withTimeout: preserves input, wraps output in Result<TOut, void>
- `[E]` withTimeout produces Chain AST
- `[E]` withTimeout with any-input body

**Execution tests (all `[N]`):**
- tryCatch: body succeeds, returns body result
- tryCatch: body throws, recovery runs with error value
- tryCatch: nested tryCatch with independent errors
- race: returns first completed result (two branches, one is sleep + constant, one is constant)
- withTimeout: action completes before timeout → Ok
- withTimeout: action exceeds timeout → Err (use sleep)
- sleep: delays then produces null

### `handler.test.ts`

**Type tests (all `[E]` — migrated from types.test.ts):**
- handler types (setup, build, verify, deploy, healthCheck, listFiles, migrate, typeCheck, fix)
- createHandler: inputValidator infers TValue
- createHandler: inputValidator + outputValidator infers both
- createHandler: source handler (input is void)
- createHandler: explicit type params
- createHandler: rejection tests (wrong return type, wrong input type, contradicting validators)
- createHandler: pipeline composition
- createHandlerWithConfig: all existing tests (~20 tests)

**Execution tests (all `[N]`):**
- createHandler with inputValidator: handler validates and transforms
- createHandler source handler: no input, produces output
- createHandlerWithConfig: stepConfig is passed to handle

---

## Migration strategy

1. Create new test files one at a time, starting with `builtins.test.ts` (foundational).
2. For each new file: copy existing tests from patterns/types/run, then add new tests.
3. After all new files are created and passing, delete `patterns.test.ts`, `types.test.ts`, `run.test.ts`.
4. Run full suite after each file to catch regressions.

Order of implementation:
1. `builtins.test.ts` — foundation, most new execution tests
2. `option.test.ts` — depends on builtins being tested
3. `result.test.ts` — parallel with option
4. `pipe.test.ts` — core combinator
5. `branch.test.ts` — dispatching, matchPrefix
6. `forEach.test.ts` — simple
7. `all.test.ts` — simple
8. `loop.test.ts` — complex control flow
9. `bind.test.ts` — effect system
10. `effects.test.ts` — tryCatch, race, timeout
11. `handler.test.ts` — mostly migration
12. Delete old files

## Test counts

| File | Existing (migrated) | New | Total |
|------|--------------------:|----:|------:|
| `builtins.test.ts` | ~12 | ~32 | ~44 |
| `option.test.ts` | ~17 | ~30 | ~47 |
| `result.test.ts` | ~22 | ~36 | ~58 |
| `pipe.test.ts` | ~12 | ~3 | ~15 |
| `branch.test.ts` | ~11 | ~5 | ~16 |
| `forEach.test.ts` | ~3 | ~5 | ~8 |
| `all.test.ts` | ~5 | ~3 | ~8 |
| `loop.test.ts` | ~14 | ~5 | ~19 |
| `bind.test.ts` | ~16 | ~5 | ~21 |
| `effects.test.ts` | ~12 | ~7 | ~19 |
| `handler.test.ts` | ~40 | ~3 | ~43 |
| `schema.test.ts` | ~60 | 0 | ~60 |
| `round-trip.test.ts` | ~6 | 0 | ~6 |
| **Total** | **~230** | **~134** | **~364** |

## Execution test infrastructure

All execution tests use the existing `runPipeline` helper from `src/run.ts`. Tests that require the Rust binary use `describe.skipIf(!HAS_BINARY)` as in the current `run.test.ts`. A shared test helper module exports the binary check and common pipeline runners.
