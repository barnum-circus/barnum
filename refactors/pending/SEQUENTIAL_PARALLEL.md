# Replace `pipe` with `sequential` (and rename `all` to `parallel`)

## Motivation

`pipe` currently serves two roles that should be distinct:

1. **Sequencing** — "do A, then do B, then do C" (ordering without data flow)
2. **Data threading** — "A's output flows into B's input" (composition)

The problem: `pipe(a, b)` *looks* like data flows from `a` to `b` because it enforces `a`'s output type matches `b`'s input type. But the canonical usage pattern in practice is:

```ts
pipe(foo.drop(), bar.call(varRef))
```

Here, `foo.drop()` outputs `null`, and `bar.call(varRef)` accepts `null` (because `.call()` erases the input slot). No meaningful data flows — it's pure sequencing. The type constraint (`null` → `null`) is structural noise, not semantic signal.

Meanwhile, `.call(x)` is the actual data-flow primitive: "pass x's output as input to this action." Every real data dependency is already expressed via `.call()`. `pipe`'s type threading is redundant.

### The insight

If `.call()` handles all data flow, then `pipe`'s only job is *ordering*. And "ordering without data flow" has a clearer name: **`sequential`**.

Similarly, `all` means "run these in parallel" — but `parallel` communicates that intent directly.

## Current state

### `pipe` signature (simplified)

```ts
function pipe<A, B, C>(
  a1: Pipeable<A, B>,
  a2: Pipeable<B, C>,
): TypedAction<PipeIn<A>, C>;
```

Each step's output type must match the next step's input type. The pipeline's input is step 1's input; its output is the last step's output.

### `all` signature (simplified)

```ts
function all<TInput, TOut1, TOut2>(
  a1: Pipeable<TInput, TOut1>,
  a2: Pipeable<TInput, TOut2>,
): TypedAction<TInput, [TOut1, TOut2]>;
```

All branches receive the same input; outputs are collected into a tuple.

### Current usage patterns

**Sequencing (no data flow):**
```ts
pipe(clearQueue.drop(), all(producer1, producer2, consumer))
pipe(foo.drop(), bar.call(varRef))
pipe(implement.call(input).drop(), typeCheckFix.call(path).drop(), reviewLoop, commit.call(path).drop(), createPR.call(prInput))
```

**Data flow between steps (rare):**
```ts
pipe(judgeRefactor, classifyJudgment)  // judgment result flows into classifier
```

## Proposed design

### `sequential(...steps)` — ordering without data flow

```ts
function sequential<TOut1, TOut2>(
  a1: Pipeable<null, TOut1>,
  a2: Pipeable<null, TOut2>,
): TypedAction<null, TOut2>;

function sequential<TOut1, TOut2, TOut3>(
  a1: Pipeable<null, TOut1>,
  a2: Pipeable<null, TOut2>,
  a3: Pipeable<null, TOut3>,
): TypedAction<null, TOut3>;
```

Every step takes `null` input. No data threads between steps. Output is the last step's output (or `null` if we want to discard all). Steps run in order.

This means:
- Steps that produce output which will flow somewhere use `.call()` at the call site
- Steps that are pure side effects use `.drop()` in the sequence
- No fake `null → null` type threading — the constraint is stated once: "all inputs are null"

### `parallel(...steps)` — concurrent execution

Rename of `all`. No semantic change, just clarity:

```ts
function parallel<TOut1, TOut2>(
  a1: Pipeable<null, TOut1>,
  a2: Pipeable<null, TOut2>,
): TypedAction<null, [TOut1, TOut2]>;
```

Same constraint: all inputs are `null`. No shared input threading.

### What happens to data flow?

All data flow is expressed exclusively through `.call()`:

```ts
// Before (pipe threading data):
pipe(judgeRefactor, classifyJudgment)

// After (.call() makes data flow explicit):
classifyJudgment.call(judgeRefactor)
```

### Rewritten examples

**Event bus (before):**
```ts
pipe(clearQueue.drop(), all(producer1, producer2, consumer))
```

**Event bus (after):**
```ts
sequential(clearQueue.drop(), parallel(producer1, producer2, consumer))
```

**Implement-and-review (before):**
```ts
pipe(
  implement.call(allObject({ worktreePath, description })).drop(),
  typeCheckFix.call(resource.pick("worktreePath")).drop(),
  reviewLoop,
  commit.call(resource.pick("worktreePath")).drop(),
  createPR.call(preparePRInput.call(allObject({ branch, description }))),
)
```

**Implement-and-review (after):**
```ts
sequential(
  implement.call(allObject({ worktreePath, description })).drop(),
  typeCheckFix.call(resource.pick("worktreePath")).drop(),
  reviewLoop,
  commit.call(resource.pick("worktreePath")).drop(),
  createPR.call(preparePRInput.call(allObject({ branch, description }))),
)
```

**With-retry catch handler (before):**
```ts
pipe(drop, checkRetries.call(retriesRemaining).branch({...}))
```

**With-retry catch handler (after):**
```ts
checkRetries.call(retriesRemaining).branch({...})
```

Wait — this one is interesting. The `pipe(drop, ...)` pattern exists because `tryCatch`'s catch branch receives the error string, and `checkRetries` doesn't want it. With `sequential`, the pattern would be:

```ts
sequential(drop, checkRetries.call(retriesRemaining).branch({...}))
```

But `drop` here isn't a side effect — it's discarding an unwanted input. That's a data-shaping concern, not a sequencing concern. This suggests `sequential` might not cover this case cleanly. See "Open questions" below.

## What `pipe` does that `sequential` doesn't

### 1. Input erasure / data shaping

`pipe(drop, foo)` means "ignore whatever input arrived, then run foo with null." This is data shaping: the first step transforms the input for the second step. `sequential` can't express this because it asserts all inputs are `null` — there's no "incoming data to discard."

Possible solutions:
- **Keep `pipe` for this narrow case** — `pipe` becomes a low-level primitive for data shaping, while `sequential` is the user-facing sequencing combinator.
- **Use `.call(drop)` instead** — `foo.call(drop)` means "call foo, ignoring the current input." But `drop` outputs `null`, so foo would need `null` input. This works when foo already takes `null`.
- **Introduce a `discard` method** — `foo.discard()` ≡ `TypedAction<any, Out>` — makes `foo` accept any input by ignoring it. Essentially `.call(drop)` as a method.

### 2. Multi-step data threading (rare)

```ts
pipe(judgeRefactor, classifyJudgment)
```

This is genuine composition: judgeRefactor's output feeds classifyJudgment's input. With `sequential`, this becomes `classifyJudgment.call(judgeRefactor)` — which is fine. `.call()` is the composition operator.

### 3. Pipeline input threading

`pipe(a, b, c)` currently has input type `PipeIn<A>` — the first step's input becomes the pipeline's input. This lets you write:

```ts
const pipeline: TypedAction<SomeInput, SomeOutput> = pipe(step1, step2, step3);
```

With `sequential`, the pipeline's input is always `null`. If the first step needs external input, you'd use `.call()` at the use site.

## Open questions

### Should `sequential` return the last step's output or `null`?

Returning the last step's output is more useful (it can participate in `.call()` chains). Returning `null` is more honest ("this is just sequencing"). Leaning toward: return last step's output.

### What replaces `pipe(drop, x)`?

Options:
1. Keep `pipe` as a low-level escape hatch for data shaping
2. Add a `.ignoreInput()` method to TypedAction
3. Use `.call(drop)` (works when `x` takes `null`)
4. The `tryCatch` catch handler signature changes to pass `null` instead of the error

Option 3 already works for most cases. Option 4 is a breaking change but arguably better design (if you want the error, use a `VarRef`).

### Should `parallel` thread a shared input?

Current `all` threads the same input to all branches. Should `parallel` do the same, or should all branches receive `null`? If `null`, branches that need input use `.call()`. This is more consistent with the "`.call()` = data flow" principle, but it's a bigger change.

### Is `sequential` just `pipe` with a constraint?

Technically, `sequential(a, b, c)` ≡ `pipe(a.drop(), b.drop(), c)` (or similar). Should `sequential` be sugar over `pipe`, or replace it entirely?

If `sequential` is the only user-facing API and `pipe` becomes internal, users never see fake data threading. But `pipe` remains available for the rare case where genuine multi-step composition (without `.call()`) is desired.

## Recommendation

Introduce `sequential` and rename `all` to `parallel`. Keep `pipe` as a low-level internal primitive (not exported from the public API, or exported but documented as "you probably want `sequential`"). The user-facing model becomes:

- **`.call(x)`** — data flows from x into this action
- **`sequential(a, b, c)`** — ordering: a runs first, then b, then c
- **`parallel(a, b, c)`** — concurrency: a, b, c run simultaneously

This is a cleaner mental model than "`pipe` does both sequencing and composition depending on whether `.drop()` appears."
