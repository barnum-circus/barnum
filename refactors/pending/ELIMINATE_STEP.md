# Eliminate Step

## Motivation

The Step AST node and the `registerSteps`/`Config.steps` machinery exist to give pipelines names so they can be referenced from multiple places (including recursively). This is a significant amount of infrastructure:

- `StepAction` and `StepRef` (Named/Root) in both the TS and Rust ASTs
- `Config.steps: HashMap<StepName, Action>` — a global step registry
- `ConfigBuilder.registerSteps()` — two overload forms (object and callback)
- `stepRef()` — creates untyped step references (loses input/output types for mutual recursion)
- `ValidateStepRefs<R>` — compile-time validation that step names resolve
- Flattening pass 1: step bodies flattened separately, Step(Root) rejected in step bodies
- Flattening pass 2: resolve all Named step references to ActionIds
- `FlatAction::Step { target }` — a pure redirect (zero semantics, just a goto)
- `FlattenError::StepRootInStepBody`, `FlattenError::UnknownStep`
- `StepName`, `StepTarget`, `StepTarget::Named`/`StepTarget::Resolved`

All of this exists because there's no other way to reference a pipeline from another pipeline. But there is: resumptive handlers.

A resumptive handler (ResumeHandle/ResumePerform, or the current Handle/Perform with Resume output) captures a value and makes it available via Perform. `bind` already uses this to make computed values available as VarRefs. The "value" can be a pipeline (an Action). If the handler, instead of returning the captured value directly, *executes* the captured pipeline with the Perform's payload as input, then we have function calls.

## The idea

A "defined function" is a pipeline wrapped in a ResumeHandle. Calling the function is a ResumePerform. The handler receives `[payload, captured_pipeline]` — but unlike bind (which returns the captured value directly), the handler *runs* the captured pipeline on the payload.

Wait — that doesn't work directly, because the handler DAG is a static action tree. It can't dynamically "run" an arbitrary pipeline stored as a value. The captured value in bind is a *runtime* JSON value, not an Action.

But Step doesn't work that way either. Step is resolved at flatten time — it's a static reference. The pipeline being called is known at compile time. So the "function" is really just: a named action tree that multiple call sites can jump to.

The question is: can we express that jump without a dedicated AST node?

## How Step works today

Step is a pure redirect in the engine:

```rust
// crates/barnum_engine/src/lib.rs:922
FlatAction::Step { target } => {
    self.advance(target, value, parent)?;
}
```

No frame created. Execution jumps to the target action with the same value and parent. At the flat config level, `Step { target: ActionId }` is a goto.

The flattener resolves named steps by:
1. Flattening each step body into the flat config (getting an ActionId for each)
2. Replacing `StepTarget::Named("X")` with the ActionId of step X's body

So `Step(Named("X"))` becomes `Step { target: 42 }` where 42 is wherever X's body was flattened.

## What replaces it

Since Step is just a goto to a known ActionId, the flattener can eliminate it entirely. Wherever the AST has `Step(Named("X"))`, replace it with the ActionId of X's body. No `FlatAction::Step` needed — the call site directly references the target action.

This is already what happens for `Step(Root)` in the workflow tree — it resolves to the root ActionId. The same works for named steps.

In the flat config, `FlatAction::Step` disappears. Every reference to a step is replaced with the step's body ActionId. The engine never sees Step.

### Self-recursion

`Step(Root)` in the workflow tree lets the workflow call itself. After flattening, this is `Step { target: workflow_root_id }`. Eliminating Step means the Chain/Branch/etc. that referenced it directly points at `workflow_root_id`. The engine re-enters the root action. This already works — `advance` is reentrant.

### Mutual recursion

Two steps referencing each other:

```ts
.registerSteps(({ stepRef }) => ({
  A: pipe(process, stepRef("B")),
  B: pipe(transform, stepRef("A")),
}))
```

Flattened today:
```
0: Chain { rest: 1 }       ← A's body
1: Step { target: 2 }      ← goto B
2: Chain { rest: 3 }       ← B's body
3: Step { target: 0 }      ← goto A
```

After eliminating Step:
```
0: Chain { rest: 2 }       ← A's body, rest points directly to B
2: Chain { rest: 0 }       ← B's body, rest points directly to A
```

The ActionIds form a cycle in the flat config. The engine follows them — `advance` is a loop that processes actions, and cycles work because the actions create frames (Chain, Handle, etc.) that produce dispatches. A cycle without any Invoke is an infinite loop, same as today.

So mutual recursion works. The flattener just needs to handle forward references — allocate ActionIds for all step roots before flattening any step bodies, then fill in the bodies. This is already how flattening works (the two-pass approach with `StepTarget::Named` → `StepTarget::Resolved`). The only difference: instead of emitting `FlatAction::Step { target }`, emit nothing — the *referencing* action directly uses the target ActionId.

### What about the TypeScript API?

The engine-level change is trivial: drop `FlatAction::Step`, resolve step references inline during flattening. The harder question is the user-facing API.

Currently, steps serve two purposes:
1. **Naming** — give a pipeline a name so it can be referenced.
2. **Recursion** — allow cycles (self-recursion via `self`, mutual recursion via `stepRef`).

For purpose 1, steps are unnecessary. A pipeline is already a value:

```ts
const typeCheckFix = loop((recur) =>
  pipe(typeCheck, classifyErrors).branch({
    HasErrors: pipe(forEach(fix).drop(), recur),
    Clean: drop,
  }),
);

// Use it directly — no registration needed
.workflow(() => pipe(setup, typeCheckFix, deploy))
```

This already works today. Steps are only needed when a pipeline needs to reference another pipeline that references it back (mutual recursion), or when a pipeline needs to reference itself (self-recursion outside of `loop`).

For purpose 2, the user proposed: **a higher-order combinator that takes an array of pipelines and makes them mutually callable.**

## `defineRecursiveFunctions`: the replacement

```ts
defineRecursiveFunctions(
  (callA, callB) => [
    // A's body — can call B
    pipe(process, callB),
    // B's body — can call A
    pipe(transform, callA),
  ],
  ([fnA, fnB]) => {
    // Use fnA and fnB in the workflow
    return pipe(setup, fnA, deploy);
  },
)
```

The first callback receives call tokens (one per function). It returns an array of pipeline bodies, each of which can use any of the call tokens. The second callback receives the bound functions and builds the rest of the pipeline.

### Desugaring: Branch + RestartHandle

A group of N mutually recursive functions desugars to a single RestartHandle with a Branch at the top. Each function is a branch case. "Calling" a function is a RestartPerform that wraps the payload in the appropriate tag.

For two mutually recursive functions A and B:

```
RestartHandle(restartHandlerId,
  Branch({
    CallA: Chain(ExtractField("value"), bodyA),
    CallB: Chain(ExtractField("value"), bodyB),
  }),
  ExtractPayloadHandler  // [payload, state] → payload
)
```

Where:
- `callA(x)` desugars to `Chain(Tag("CallA"), RestartPerform(restartHandlerId))`
- `callB(x)` desugars to `Chain(Tag("CallB"), RestartPerform(restartHandlerId))`

When `callA` fires:
1. Body is torn down (RestartPerform semantics)
2. Handler extracts payload: `{ kind: "CallA", value: x }`
3. Body restarts with `{ kind: "CallA", value: x }`
4. Branch routes to A's body with input `x`
5. A runs. If A calls B, same cycle — tear down, restart, Branch routes to B.

When a function body completes without calling another function, the value flows up through the Branch, out of the RestartHandle, and into the continuation.

This is exactly the `loop`/`tryCatch` pattern. The "functions" are branch arms. "Calling" a function is a tagged restart.

### Self-recursion

Self-recursion is the degenerate case with one function:

```ts
defineRecursiveFunction(
  (recur) => pipe(process, branch({
    Continue: recur,
    Done: identity,
  })),
  (fn) => pipe(setup, fn, deploy),
)
```

This is `loop` with an explicit exit. In fact, `loop` can be reimplemented in terms of `defineRecursiveFunction` — loop's `recur` is a restart, and `done` exits the branch.

### What replaces `self`?

Today, `self` (`Step(Root)`) lets the workflow restart from the top. With `defineRecursiveFunction`, the workflow itself can be wrapped:

```ts
defineRecursiveFunction(
  (self) => pipe(
    process,
    branch({
      Retry: pipe(drop, self),
      Success: identity,
    }),
  ),
  (workflow) => workflow,
)
```

Or more precisely, `self` is just a RestartPerform that restarts the outermost RestartHandle. The identity continuation `(workflow) => workflow` means the function IS the workflow.

## What disappears

From the AST:
- `StepAction`, `StepRef` (Named/Root) — gone
- `Config.steps` — gone. Config becomes `{ workflow: Action }`.

From the TypeScript API:
- `ConfigBuilder.registerSteps()` — gone
- `stepRef()` — gone
- `ValidateStepRefs<R>` — gone
- `StripRefs<TSteps>` — gone
- The `steps` parameter in `workflow()` callback — gone
- The `self` parameter in `workflow()` callback — replaced by `defineRecursiveFunction`

From the Rust AST:
- `StepAction`, `StepRef`, `StepName` — gone
- `Config.steps` — gone

From the flattener:
- Step flattening (pass 1 Named/Root handling) — gone
- Step resolution (pass 2) — gone
- `StepTarget`, `StepTarget::Named`/`StepTarget::Resolved` — gone
- `FlattenError::StepRootInStepBody`, `FlattenError::UnknownStep` — gone

From the engine:
- `FlatAction::Step { target }` — gone

## What stays

- Pipelines as values — always worked, no change
- `loop`, `tryCatch`, `race` — unchanged (they already use the restart pattern)
- `bind`/`bindInput` — unchanged (resumptive handlers, orthogonal)
- Non-recursive named pipelines — just use `const x = pipe(...)`. No registration.

## Open questions

1. **`defineRecursiveFunctions` type safety.** The current `stepRef` is `TypedAction<any, any, N>` — untyped. Can the new API do better? The call tokens need types (input/output of the function they call). But with mutual recursion, function A's output might depend on function B's output and vice versa. TypeScript can't infer circular types from a callback. We might need explicit type parameters: `defineRecursiveFunctions<[In1, Out1], [In2, Out2]>(...)`. Worse ergonomics than `stepRef` but more type-safe.

2. **Interaction with RESUME_VS_RESTART_HANDLERS.** The desugaring uses RestartHandle/RestartPerform. This refactor depends on RESUME_VS_RESTART_HANDLERS landing first (or uses the current Handle/Perform with RestartBody output, which works today but is less clean).

3. **Performance of restart-based function calls.** Every function call tears down the body and restarts. For deeply nested call stacks (A calls B calls C calls D...), each call tears down all frames since the last RestartHandle. This is O(frames) per call. Step was O(1) — a pure goto. For shallow recursion (loop iterations, retry) this is fine. For deep mutual recursion with many live frames, it's worse. Is deep mutual recursion a real use case?

4. **Single-function convenience.** `defineRecursiveFunction` (singular) for self-recursion is common enough to warrant its own combinator. It's a thin wrapper around the general form.

5. **Exit semantics.** When a function body completes without calling another function, the value exits the RestartHandle. This means the "last function called" determines the output. Is that always what the user wants? With steps, the call site controlled flow after the step returned. With restart-based calls, there's no "return" — the body is torn down. The continuation in the second callback handles what happens after. Functions that want to "return" a value to a caller would need a different mechanism (a resumptive handler, not a restart handler). This is worth thinking about carefully. The restart pattern works for control flow (loop/retry/tryCatch) but might not work for "call this function and use its result."

    Actually, this is a significant limitation. Steps are gotos — they jump, and whatever happens after the step becomes the new execution context. Restart handlers tear down everything. If function A calls function B midway through a pipeline, A's remaining pipeline is destroyed. With Step, B would execute and its result would flow into A's continuation.

    This means restart-based "functions" only work for tail calls — the call must be the last thing in the pipeline. Non-tail calls (call B, then do more work with B's result) require a different mechanism. bind could work: capture a pipeline as a value, then use it. But bind doesn't support recursion.

    This is the core tension. Steps are unrestricted gotos (tail and non-tail). Restart handlers are tail-call-only. Resumptive handlers preserve the call site but can't handle recursion (the handler DAG is static).

6. **Hybrid approach?** Maybe Step stays in the flattener as an internal representation (a goto in the flat config) but disappears from the user-facing API. The `defineRecursiveFunctions` combinator desugars into whatever internal AST nodes are needed (which might still include Step at the flat level for non-tail calls). The user never sees Step — they see `defineRecursiveFunctions` and raw pipeline values.

    Or: the flattener inlines step references (eliminating `FlatAction::Step`) as described in the "What replaces it" section. The user-facing API uses `defineRecursiveFunctions` for recursion and raw pipeline values for everything else. `FlatAction::Step` disappears from the flat config regardless.
