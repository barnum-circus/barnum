# Phase 4: Loop Migration and Early Return

## Goal

Migrate LoopAction from a dedicated frame kind to Handle/Perform sugar. Remove LoopAction from both the tree AST and the Rust scheduler. Expose `scope`/`jump` as the underlying primitive.

## Prerequisites

Phase 1 (Effect Substrate) complete.

## scope + jump

`scope` establishes a restart boundary. `jump` sends a value back to the beginning.

```ts
scope<TIn, TJump, TOut>(
  body: (jump: () => TypedAction<TJump, never>) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TJump | TOut>
```

- **`scope(jump => body)`** — runs the body. If it completes normally, the scope produces the body's output. If `jump(v)` fires, execution restarts at the scope boundary and the scope produces `v`.
- **`jump()`** — Perform that restarts the scope body. The jumped value becomes the scope's output.

`jump` is passed as a single argument, not destructured from an object.

### How scope compiles

```ts
// User writes:
scope(jump =>
  pipe(step1, branch({ Bad: jump(), Good: identity() }), step2)
)

// jump(v) = pipe(tag<LoopResultDef, "Break">("Break"), Perform(jumpEffect))

// Compiles to:
Handle(jumpEffect, RestartBody,
  Branch({
    Continue: pipe(step1, branch({ Bad: Perform(jumpEffect), Good: identity() }), step2),
    Break: identity(),
  })
)
```

The internal branch dispatches on `LoopResult` — the same control flow enum used by `loop`. `Continue` = run the body, `Break` = exit with the jumped value. Initial input tagged as `LoopResult::Continue`. When `jump(v)` fires, the handler restarts the body with `LoopResult::Break(v)`. The Break branch produces `v` via `identity()`, exiting the scope through normal completion.

One effect. One handler. No Discard mechanism. Same control flow enum as loop — scope and loop share `LoopResult` as their dispatch type.

## How loop compiles

Both `recur` and `done` are jumps — they both fire Performs targeting the same EffectId. The difference is the variant tag they carry. The branch at the top of the compiled scope dispatches on `LoopResult<TContinue, TBreak>` — the existing control flow enum (same pattern as `Option` and `Result`, already defined in `ast.ts`).

```ts
// User writes:
loop(({ recur, done }) =>
  pipe(body, branch({ HasErrors: pipe(fix, recur()), Clean: done() }))
)

// recur(v) = pipe(tag<LoopResultDef, "Continue">("Continue"), Perform(jumpEffect))
// done(v)  = pipe(tag<LoopResultDef, "Break">("Break"), Perform(jumpEffect))

// Compiles to:
Handle(jumpEffect, RestartBody,
  Branch({
    Continue: pipe(body, branch({ ... })),  // initial entry + recur
    Break: identity(),                       // done: exit
  })
)
```

Initial input tagged as `LoopResult` with `{ kind: "Continue", value: input }`. Recur produces `LoopResult::Continue`, done produces `LoopResult::Break`. The handler always responds with RestartBody. The branch dispatches on the `LoopResult` variants: Continue runs the body, Break exits.

The `LoopResult` type carries `__def` so the branch gets exhaustiveness checking and type-safe payload extraction — same as branching on `Result<Ok, Err>` or `Option<T>`.

### Why both recur and done are effects

If `done` were just normal completion (`identity()`), it would only exit the innermost loop. An outer loop's `done` couldn't skip past an inner loop — there's no mechanism for normal values to unwind frames. Making `done` a Perform means it bubbles through Handle frames until it finds its matching EffectId, enabling labeled breaks for free.

## Early return: the `?` operator

```ts
scope(jump =>
  pipe(
    tryAction(step1),
    branch({ Ok: identity(), Err: jump() }),  // ? operator
    tryAction(step2),
    branch({ Ok: identity(), Err: jump() }),
  ),
)
// output type: TStep2Output | TErr
```

Sugar:

```ts
function propagate<TValue, TError>(
  jump: () => TypedAction<TError, never>,
): TypedAction<Result<TValue, TError>, TValue> {
  return branch({ Ok: identity(), Err: jump() });
}
```

## Migration strategy

1. **Implement scope + jump** — Handle with RestartBody + LoopResult branch (Continue = run body, Break = exit).
2. **Compile loop to scope** — Both recur and done are Performs producing LoopResult variants. Branch dispatches on LoopResult.
3. **Verify test parity** — All existing loop tests pass.
4. **Remove LoopAction** — From tree AST, Rust scheduler, and flattener.
5. **Add early return to kitchen-sink demo** — scope + jump for catastrophic error bailout.

## Files to change

| File | What changes |
|------|-------------|
| `libs/barnum/src/builtins.ts` | Add `scope()`, rewrite `loop()` to desugar to scope |
| `libs/barnum/src/ast.ts` | Remove `LoopAction` from `Action` union |
| `libs/barnum/tests/patterns.test.ts` | Verify AST shapes for new compilation |
| `libs/barnum/tests/types.test.ts` | Type tests for scope + jump |
| Rust engine | Remove `FrameKind::Loop`. Handle frames already support RestartBody. |
