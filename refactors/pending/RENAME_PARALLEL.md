# Rename `parallel` to `all`

## Problem

`parallel` sounds too academic. It describes an implementation detail (concurrent execution) rather than what the combinator actually does (run multiple actions on the same input, collect results as a tuple).

A user reading `parallel(setup, build)` has to know that "parallel" in this context means "fan-out the input to both actions and give me back a tuple." That's not the first thing the word "parallel" evokes — concurrency is.

## What it actually does

```
Input → all(a, b, c) → [A, B, C]
```

Broadcast one input to N actions. Collect their outputs into a tuple. Whether execution happens concurrently is an implementation detail of the scheduler.

## Proposed name: `all`

```ts
// Before
parallel(setup, build)
parallel(identity<Ctx>(), createPR)

// After
all(setup, build)
all(identity<Ctx>(), createPR)
```

**Why `all`**:

- Familiar from `Promise.all` — "run all of these, give me all the results"
- Reads naturally: `all(checkHealth, notify, report)` — "do all of these"
- Short
- Doesn't imply a specific execution strategy

## Alternatives considered

| Name | Reads as | Verdict |
|---|---|---|
| `all` | "run all of these" | **Recommended.** Familiar, natural. |
| `fanOut` | "fan the input out to multiple actions" | Precise but jargon-y. Haskell's `(&&&)` is called "fanout" in Arrow. |
| `fork` | "fork into multiple paths" | Implies process forking / concurrency. Same problem as `parallel`. |
| `split` | "split into multiple computations" | Ambiguous — could mean splitting an array or a string. |
| `broadcast` | "broadcast input to all handlers" | Accurate but verbose. Sounds like a messaging pattern. |
| `tee` | "copy input to multiple outputs" | Unix metaphor. Obscure for non-Unix users. |
| `both` | "run both of these" | Only works for exactly 2 args. Doesn't generalize to 3+. |
| `tuple` | "produce a tuple" | Describes the output shape, not the operation. |
| `zip` | "zip computations together" | Already means element-wise pairing of sequences. Confusing. |
| `gather` | "gather results from multiple actions" | Decent but focuses on the collection step, not the fan-out. |
| `together` | "run these together" | Vague. |
| `each` | "run each of these" | Collides conceptually with `forEach`. |

## AST node rename

The AST `kind` string and interface name should also change:

```ts
// Before
interface ParallelAction { kind: "Parallel"; actions: Action[] }

// After
interface AllAction { kind: "All"; actions: Action[] }
```

This is a breaking change to the serialized config format. Since we don't care about backward compatibility, this is fine.

## Scope

### TypeScript

| File | What changes |
|---|---|
| `libs/barnum/src/parallel.ts` | Rename file to `all.ts`. Rename function `parallel` → `all`. |
| `libs/barnum/src/ast.ts` | Rename `ParallelAction` → `AllAction`, `kind: "Parallel"` → `kind: "All"`, export `all` instead of `parallel`. |
| `libs/barnum/src/builtins.ts` | ~10 raw AST nodes with `kind: "Parallel"` in `withResource`, `augment`, `tap`. |
| `libs/barnum/tests/patterns.test.ts` | Import, describe blocks, test bodies. |
| `libs/barnum/tests/types.test.ts` | Import, test names, assertions. |
| `libs/barnum/tests/round-trip.test.ts` | Import, test name, call. |
| `libs/barnum/tests/steps.test.ts` | Import. |
| `demos/simple-workflow/run-parallel.ts` | Import, call site. |

### Rust (when it exists)

The Rust AST enum variant `Action::Parallel` → `Action::All`. The serde tag changes from `"Parallel"` to `"All"`.

### Documentation

~20 markdown files in `refactors/pending/` reference `parallel`. These should be updated in a batch find-and-replace pass.

## Migration

Mechanical rename. No semantic changes. Find-and-replace across the codebase:

- `parallel(` → `all(`
- `"Parallel"` → `"All"`
- `ParallelAction` → `AllAction`
- `parallel.ts` → `all.ts`
- `parallel.js` → `all.js`

## Downstream: rethink `merge`

`parallel` + `merge` is the current pattern for the reader monad (preserve context alongside a computation). With the rename:

```ts
// Before
pipe(parallel(identity(), build), merge())

// After
pipe(all(identity(), build), merge())
```

This reads better. `all(identity(), build)` — "run both identity and build, give me a tuple" — then `merge()` — "merge the tuple into one object."

But `augment` already handles this pattern without `all` + `merge`. The `all` + `merge` combo is mostly an internal implementation detail of `augment`, `tap`, and `withResource`. User-facing code rarely writes `all(...).then(merge())` directly.
