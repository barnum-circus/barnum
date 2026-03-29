# Phase 3: Error Handling (Throw Effect)

## Goal

Implement `tryCatch` as the first effect that discards its continuation. When a Throw effect fires, the handler drops the suspended subgraph and runs a recovery branch instead. This validates frame teardown and the discard path.

## Prerequisites

Phase 1 (Effect Substrate) complete. Phase 2 (Declare) is not a prerequisite — Phases 2 and 3 can proceed in parallel.

## The effect

```
Effect: Throw
Payload: Value (the error data)
Handler behavior: Discard the continuation (clean up suspended frames).
                  Advance the recovery branch with the error payload.
                  The Handle frame delivers the recovery branch's result, not the body's.
```

This is the first zero-shot continuation pattern. The handler never resumes. The continuation is orphaned and must be cleaned up.

## How tryCatch compiles

```ts
// User writes:
tryCatch(
  (throwError) => body_using_throwError,
  recovery,
)

// TypeScript builder:
// 1. Gensyms a fresh EffectId for this tryCatch instance
// 2. Creates throwError = Perform(freshEffectId) wrapper
// 3. Calls the callback to get the body AST
// 4. Compiles to:
Handle(
  { [freshEffectId]: recoveryHandler },   // handler DAG = the recovery branch
  body                                     // contains Perform(freshEffectId) at throw sites
)
```

The `throwError` token has type `Pipeable<TError, never>`. It takes the error payload and never returns — the continuation is discarded. TypeScript can enforce that code after `throwError` in a Chain is unreachable.

Because each tryCatch mints its own EffectId, nested tryCatch gives precise targeting:

```ts
tryCatch((throwOuter) =>
  tryCatch((throwInner) =>
    pipe(
      riskyAction,
      branch({
        Recoverable: throwInner,   // caught by inner
        Fatal: throwOuter,          // skips inner, caught by outer
      }),
    ),
    innerRecovery,
  ),
  outerRecovery,
)
```

No re-throwing needed. `throwOuter` is `Perform(effectId_7)`, `throwInner` is `Perform(effectId_8)`. Each Handle matches its own ID.

The handler DAG for Throw receives `{ payload: errorData }` and runs the recovery action on the payload. The handler produces a Discard tagged output: the continuation is dropped and the Handle frame exits with the recovery result.

```ts
// The handler DAG:
pipe(
  pick("payload"),    // extract the error data
  recovery,           // run the recovery branch
  tag("Discard"),     // produces { kind: "Discard", value: recoveryResult }
)
```

The Handle frame interprets `{ kind: "Discard", value }`: it moves the continuation to orphaned_continuations and delivers `value` to its parent. The continuation is cleaned up when the Handle frame exits. This is deterministic — see Phase 1's cleanup lifecycle.

## Where Throw is performed

Throw can come from two sources:

### 1. Explicit throw via the intent pattern

Handlers are opaque — they cannot emit effects directly. They return discriminated unions describing their intent. The AST interprets those unions and throws when appropriate.

```ts
// Handler returns a result union:
type HandlerResult =
  | { kind: "Ok"; value: Output }
  | { kind: "Err"; error: string };

// AST interprets the intent:
tryCatch(
  (throwError) => pipe(
    invoke(riskyHandler),
    branch({
      Ok: pick("value"),
      Err: pipe(pick("error"), throwError),
    }),
  ),
  handleError,
)
```

A convenience combinator wraps the boilerplate. It takes the throw token as a parameter (explicit propagation):

```ts
// invokeWithThrow: Invoke + branch on error union + throw
function invokeWithThrow<TIn, TOut, TErr>(
  handler: Pipeable<TIn, { kind: "Ok"; value: TOut } | { kind: "Err"; error: TErr }>,
  throwError: Pipeable<TErr, never>,
): TypedAction<TIn, TOut> {
  return pipe(
    handler,
    branch({
      Ok: pick("value"),
      Err: pipe(pick("error"), throwError),
    }),
  );
}

// Usage:
tryCatch(
  (throwError) => pipe(invokeWithThrow(riskyHandler, throwError), processResult),
  handleError,
)
```

The handler remains oblivious to the effect system. It returns data. The AST translates data into control flow. This is the Free Monad / Control Plane / Data Plane separation (see EFFECTS_ROADMAP.md).

The throw token is always passed explicitly. If a utility function needs to throw, its API surface declares it — same pattern as Rust's `Result` return types.

### 2. Handler execution failure

When an Invoke (external handler call) fails at runtime, the scheduler currently propagates the error up the frame tree. With Handle/Perform, a handler failure could instead emit a Throw effect, which bubble_effect routes to the nearest tryCatch Handle.

This changes error propagation from a special-case mechanism to an effect. Whether to do this in Phase 3 or later is an open question. Options:

**Option A: Error propagation remains separate.** Handler failures propagate via the existing error path. Throw is only for explicit user-initiated errors. TryCatch handles Throw effects; handler failures propagate past it.

**Option B: Handler failures become Throw effects.** The scheduler converts handler failures to Perform(Throw). TryCatch catches both explicit throws and handler failures. The existing error propagation path is simplified (it only handles truly unrecoverable errors, like slab corruption).

Option B is cleaner long-term but changes existing error semantics. Recommend Option A for Phase 3, migrate to Option B later if desired.

## Frame teardown on discard

When the Handle frame exits with an un-resumed continuation, it must clean up:

```rust
fn teardown_continuation(&mut self, root: FrameId) {
    let mut stack = vec![root];
    while let Some(frame_id) = stack.pop() {
        let frame = self.frames.remove(frame_id);
        match frame.kind {
            FrameKind::Parallel { children, .. } => {
                // Recurse into active children
                for child in children {
                    if let Some(child_id) = child.frame_id() {
                        stack.push(child_id);
                    }
                }
            }
            FrameKind::ForEach { iterations, .. } => {
                for iter in iterations {
                    if let Some(iter_id) = iter.frame_id() {
                        stack.push(iter_id);
                    }
                }
            }
            FrameKind::Handle { continuations, .. } => {
                // Nested Handle with its own continuations — tear those down too
                for (_, cont_root) in continuations {
                    stack.push(cont_root);
                }
            }
            _ => {}
        }

        // Cancel any pending external tasks mapped to this frame
        if let Some(task_id) = self.pending_tasks.remove_by_frame(frame_id) {
            self.pending_cancellations.push(task_id);
        }
    }
}
```

This is depth-first traversal of the orphaned subgraph. Every frame is removed from the slab. Pending external tasks are cancelled. Nested Handle frames with their own continuations are recursively torn down.

## Nested tryCatch

```ts
tryCatch(
  tryCatch(
    riskyAction,
    innerRecovery,
  ),
  outerRecovery,
)
```

If riskyAction throws, the inner Handle catches it. If innerRecovery itself throws (or if the inner body throws an effect the inner Handle doesn't catch), the outer Handle catches it.

This works naturally with bubble_effect: the Throw walks past the inner Handle (if it already handled its effect and exited) or is caught by the inner Handle (if it's still active). Standard lexical scoping.

## tryCatch + declare interaction

```ts
declare({ resource: acquireResource }, ({ resource }) =>
  tryCatch(
    pipe(resource.then(useResource), riskyStep),
    pipe(resource.then(cleanupPartialWork), reportError),
  ),
)
```

The declare Handle is the outer scope. The tryCatch Handle is the inner scope. If riskyStep throws:
1. bubble_effect walks up from the Perform(Throw) site.
2. It finds the tryCatch Handle first (it's nearer).
3. The tryCatch Handle discards the continuation and runs recovery.
4. Recovery can reference `resource` via Perform(ReadVar) — the declare Handle is still active above.
5. When recovery completes, the tryCatch Handle delivers the result.
6. Eventually the declare Handle's body completes and scope cleanup runs.

The ReadVar and Throw effects compose without interference because they're routed to different Handle frames.

## Test strategy

### Rust scheduler tests

1. **Simple tryCatch**: Body throws, recovery runs, produces a value. Verify the value.
2. **No throw**: Body completes normally. Recovery never runs. Verify body's output.
3. **Throw discards continuation**: Body has work after the throw point. Verify it doesn't execute.
4. **Throw inside Chain**: Throw is the first half of a Chain. Verify the rest doesn't execute.
5. **Throw inside Parallel**: One parallel branch throws. Verify the other branch is cancelled during teardown.
6. **Throw inside ForEach**: One iteration throws. Verify other iterations are cancelled.
7. **Nested tryCatch**: Inner throw caught by inner handler. Outer handler doesn't fire.
8. **Throw propagation**: Inner tryCatch doesn't match (e.g., catches a different effect type). Throw propagates to outer tryCatch.
9. **Teardown cancels external tasks**: Body has a pending Invoke when throw happens. Verify the external task is cancelled.
10. **tryCatch + declare**: Recovery branch references a variable from an outer declare. Verify it resolves correctly.

### TypeScript compilation tests

1. `tryCatch(body, recovery)` produces correct Handle/Perform AST.
2. Recovery receives the error payload.
3. Type checking: recovery input type matches the throw payload type.

## Deliverables

1. `EffectType::Throw` variant
2. Handle frame Throw handler logic (discard continuation, advance recovery)
3. `teardown_continuation` method — recursive frame cleanup
4. Pending task cancellation during teardown
5. `tryCatch()` TypeScript function
6. `throwError()` / `Perform("Throw")` TypeScript helper
7. Flattener support
8. Tests per above
