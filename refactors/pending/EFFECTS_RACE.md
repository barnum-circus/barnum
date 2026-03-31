# Race

## Goal

Run multiple actions concurrently. The first to complete wins. The losers are cancelled. Race is Handle + Parallel + cancellation — not a new AST node, only a new handler DAG and TS surface function.

## Prerequisites

Phase 1 (Substrate — Handle/Perform), Phase 3 (TryCatch — for the discard path and teardown), Parallel.

## The problem

Run multiple actions concurrently. The first to complete wins. The losers are cancelled.

## The solution

Race is Handle + Parallel + cancellation. Not a new AST node.

```ts
// User writes:
race(actionA, actionB)

// Builder gensyms a fresh EffectId: raceEffect.
// Handler DAG: pipe(pick("payload"), tag("Discard"))
// Compiles to:
Handle(raceEffect, raceHandler,
  Parallel(
    Chain(actionA, Perform(raceEffect)),
    Chain(actionB, Perform(raceEffect)),
  )
)
```

The handler receives `{ payload: firstResult, state: ... }`, extracts the payload, and produces Discard. The Handle frame tears down the body (including the un-completed Parallel branch) and exits with the first result.

## Cancellation semantics

When the Handle exits with a live Parallel frame below it, teardown must:
1. Cancel pending external tasks in the losing branch.
2. Run Bracket dispose for any resources the losing branch acquired.
3. Remove all frames from the slab.

This uses the same `teardown_body` from Phase 1. Race doesn't add new teardown logic — it exercises existing teardown under Parallel.

## Type safety

`race(a, b)` requires both branches to produce the same output type (since either could win). The TypeScript function enforces this:

```ts
function race<TIn, TOut>(
  ...actions: Pipeable<TIn, TOut>[]
): TypedAction<TIn, TOut>
```

## Test strategy

1. Two branches, first completes. Second is cancelled.
2. Two branches, second completes. First is cancelled.
3. Losing branch has pending external task. Task is cancelled.
4. Losing branch has acquired resources. Resources are disposed during teardown.
5. Race with 3+ branches.
6. Nested race.

## Deliverables

1. `race()` TypeScript function (gensyms EffectId, builds Handle + Parallel + Perform DAG)
2. Race handler DAG (`pick("payload") → tag("Discard")`)
3. Rust engine tests for race-shaped ASTs
4. TypeScript compilation tests (AST structure)
5. TypeScript type-level tests (output type homogeneity)
