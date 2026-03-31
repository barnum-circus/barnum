# Phase 4: Loop Migration and Early Return

## Goal

Migrate the existing LoopAction from a dedicated frame kind to Handle/Perform sugar. After this phase, LoopAction is removed from both the tree AST and the Rust scheduler. Loop behavior is unchanged — existing tests provide regression coverage.

Additionally, expose `scope`/`jump` as the general-purpose early return primitive that `done` is built on.

## Prerequisites

Phase 1 (Effect Substrate) complete. Phase 3's teardown (discarding continuations) is needed for the Break/jump path.

## The underlying primitive: scope + jump

`done` in a loop is early return. The body is executing a pipeline, and `done` says "stop executing, discard the rest of the pipeline, and exit with this value." This is the same mechanism needed for any early return pattern.

We expose this as `scope` + `jump`:

- **`scope(body)`** — establishes a boundary. The body runs normally. If a `jump` fires inside the body, execution short-circuits to the scope boundary and the scope produces the jumped value.
- **`jump()`** — like `done` in a loop. Fires an effect (Perform) that bubbles up to the enclosing scope's Handle and discards the continuation.

`loop` is built on top of scope. `done` = `jump` out of the loop body's scope. `recur` is the separate "restart" effect.

### How scope + jump compiles

```ts
// User writes:
scope(({ jump }) =>
  pipe(step1, step2, branch({ Bad: jump(), Good: identity() }), step3)
)

// Builder gensyms one EffectId: jumpEffect
// jump() = Perform(jumpEffect)

// Compiles to:
Handle(jumpEffect, tag("Discard"),    // jump exits the scope
  pipe(step1, step2, branch({ Bad: jump(), Good: identity() }), step3)
)
```

The engine sees the Discard tag and tears down all frames between the Perform and the Handle, producing the jumped value as the scope's output.

### How loop uses scope + jump

`loop` creates two effects: one for `recur` (RestartBody) and one for `done` (Discard). The `done` effect IS a jump — same mechanism, same Discard tag, same frame teardown:

```ts
// User writes:
loop(pipe(body, branch({ HasErrors: pipe(fix, recur), Clean: done })))

// Compiles to:
Handle(doneEffect, tag("Discard"),           // done = jump out of loop
  Handle(recurEffect, tag("RestartBody"),    // recur = restart body
    body
  )
)
```

`done` and `jump` are the same thing. `loop` just adds a second effect for `recur` and wraps the whole thing in a restart loop.

## Two effects, two nested Handles

Loop uses two separate effects. Each is a separate Handle with a trivial handler. No payload branching.

- `recur` = `Perform(recurEffect)` → caught by inner Handle → `RestartBody` → re-enter body
- `done` = `Perform(doneEffect)` → bubbles past inner Handle → caught by outer Handle → `Discard` → exit loop

### Why RestartBody, not a cyclic graph edge?

The theoretically pure approach: model `recur` as a cyclic `Step` back to the body's ActionId, not as an effect. This is how Scheme/Lisp does it with `call/cc`. But without generalized tail call optimization (TCO) in the scheduler, each iteration pushes a new frame onto the slab. 10,000 iterations = 10,000 frames → OOM.

RestartBody is a localized trampoline. The inner Handle frame tears down the old body frames and re-advances the body ActionId. O(1) memory. No complex tail-call analysis. If we later add TCO to the scheduler (which benefits all cyclic patterns, not just loops), RestartBody becomes unnecessary and can be removed.

## How loop compiles

```ts
// User writes:
loop((recur, done) =>
  pipe(body, branch({ HasErrors: pipe(fix, recur), Clean: done }))
)

// Builder gensyms two EffectIds: recurEffect, doneEffect
// recur = Perform(recurEffect)
// done = Perform(doneEffect)

// Compiles to:
Handle(doneEffect, tag("Discard"),           // outer: done exits the loop
  Handle(recurEffect, tag("RestartBody"),    // inner: recur restarts the body
    body
  )
)
```

Each handler DAG is trivial — just tag the value with the continuation operation. No branching.

### Nested loops: labeled breaks for free

Each loop invocation mints its own pair of EffectIds. Nested loops have distinct Handles:

```ts
loop((recurOuter, doneOuter) =>
  loop((recurInner, doneInner) =>
    pipe(body, branch({
      ContinueInner: recurInner,
      BreakInner: doneInner,
      BreakBoth: doneOuter,       // breaks out of BOTH loops
    }))
  )
)
```

`doneOuter` bubbles past the inner loop's two Handles (wrong EffectIds) and is caught by the outer loop's done Handle.

### Nested scope + loop: jump past a loop

The same bubbling works for `jump` inside a loop inside a scope:

```ts
scope(({ jump }) =>
  loop(
    pipe(body, branch({
      CatastrophicError: jump(),   // exits the SCOPE, not the loop
      HasErrors: pipe(fix, recur),
      Clean: done,
    }))
  )
)
```

`jump()` bubbles past both of the loop's Handles (wrong EffectIds) and is caught by the scope's Handle. The loop is abandoned entirely.

### HOAS is required

There are no standalone `jump()` combinators without a scope context. The HOAS callback is the only way to get the tokens. Same as `recur()`/`done()` in the closure form of loop.

## Early return: demo use case

The identify-and-address-refactors demo has a type-check/fix cycle that loops until clean. In practice, some errors are catastrophic (missing dependencies, broken imports that affect the entire file) and shouldn't be retried — the loop should bail out entirely.

With scope + jump, the pipeline can short-circuit on catastrophic errors:

```ts
// Current: loops forever on unfixable errors
.registerSteps({
  TypeCheckFix: loop(
    pipe(drop(), typeCheck, classifyErrors).branch({
      HasErrors: forEach(fix).drop().then(recur()),
      Clean: done(),
    }),
  ),
})

// With early return: bail on catastrophic errors
.registerSteps({
  TypeCheckFix: scope(({ jump }) =>
    loop(
      pipe(drop(), typeCheck, classifyErrors).branch({
        CatastrophicError: jump(),    // unfixable — bail out of the whole cycle
        HasErrors: forEach(fix).drop().then(recur()),
        Clean: done(),
      }),
    ),
  ),
})
```

This requires `classifyErrors` to produce a three-variant union (`CatastrophicError | HasErrors | Clean`) instead of two. The scope catches the jump and produces whatever the catastrophic error payload is. The pipeline can then decide what to do (skip this file, log and continue, etc.).

### Demo: new kitchen-sink demo

The `demos/kitchen-sink/` directory (recently created, not yet populated) should demonstrate scope + jump alongside loops and other control flow patterns. A type-check/fix cycle with catastrophic error bailout is the natural fit:

```ts
// Type-check/fix loop with early return on catastrophic errors
scope(({ jump }) =>
  loop(
    typeCheck.then(classifyErrors).branch({
      CatastrophicError: jump(),    // bail: this file is unfixable
      HasErrors: forEach(fix).drop().then(recur()),
      Clean: done(),
    }),
  ),
)
```

This shows the core pattern: scope wraps a loop, and jump provides an escape hatch when the loop's own control flow (recur/done) isn't sufficient. The kitchen-sink demo should exercise this alongside forEach, bind, Option/Result combinators, and other framework features.

## Migration strategy

### Step 1: Implement scope + jump

Add `scope()` as a user-facing function that produces a Handle with Discard semantics. Add `jump()` as the HOAS-provided Perform token. This is the primitive that `done` will be built on.

### Step 2: Compile loop to Handle/Perform

Change `loop()` to produce two nested Handles instead of LoopAction. The `done` path uses the same Discard mechanism as `jump`. The HOAS callback replaces the current standalone `recur()`/`done()`.

### Step 3: Verify test parity

All existing loop tests must pass with the new compilation. The outputs must be identical.

### Step 4: Remove LoopAction

Remove `LoopAction` from the tree AST union. Remove `FrameKind::Loop` from the Rust scheduler. Remove the flattener's Loop handling. Remove standalone `recur()`/`done()`.

### Step 5: Verify demos

Run all demos. They should work unchanged because the surface API (`loop`) is the same.

### Step 6: Add early return to kitchen-sink demo

Add scope + jump for catastrophic error bailout to the kitchen-sink demo.

## Test strategy

### Regression tests

All existing loop tests must pass after migration:
- Simple loop with Break
- Loop with Continue (multiple iterations)
- Nested loops
- Loop inside ForEach
- Loop inside Parallel
- Loop with branch dispatching Continue/Break

### New tests

1. **recur/done outside loop**: Verify `UnhandledEffect` error.
2. **Labeled break**: Inner loop breaks out of outer loop via `doneOuter`.
3. **Loop with declare**: Variables from an outer declare are accessible inside the loop body, including across Continue re-entries.
4. **Loop with tryCatch**: Error inside loop body caught by tryCatch inside the loop. Loop continues.
5. **tryCatch around loop**: Error inside loop body propagates past the loop's Handles to the tryCatch Handle.
6. **O(1) memory**: Loop with 10,000 iterations does not grow the slab. RestartBody tears down old frames each iteration.
7. **scope + jump**: Jump exits the scope, discarding the continuation.
8. **scope + jump inside loop**: Jump exits the enclosing scope, abandoning the loop entirely.
9. **nested scopes**: Inner jump exits inner scope only. Outer jump exits outer scope.
10. **scope around forEach**: Jump inside a forEach iteration exits the scope, not just the current iteration.

## Deliverables

1. `scope()` + `jump()` implemented as Handle/Perform with Discard semantics
2. `loop()` rewritten to produce two nested Handles, `done` using the same Discard mechanism as `jump`
3. `recur` / `done` tokens provided via callback, not standalone combinators
4. Migration: test parity verified against existing loop tests
5. LoopAction and FrameKind::Loop removed from tree AST and Rust scheduler
6. Kitchen-sink demo updated to show early return via scope + jump
7. All existing tests pass
