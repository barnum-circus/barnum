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

A group of mutually recursive functions is a ResumeHandle whose handler contains a Branch — one arm per function, each arm containing that function's full pipeline body. Calling a function is a tagged ResumePerform. The handler DAG is a static action tree, but the JavaScript builder (higher-order abstract syntax) constructs it at AST-build time with all function bodies embedded directly in the handler's Branch. Recursive calls work because ResumePerform can fire recursively within handler execution — each call creates a ResumePerformFrame, forming a call stack. The caller's pipeline is preserved across the call (ResumeHandle semantics).

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

### Desugaring: ResumeHandle with Branch in the handler

The handler DAG is a static action tree, but the JavaScript builder constructs it at AST-build time with all function bodies embedded. Since the function bodies are known when the combinator runs, they go directly into the handler's Branch. Recursive calls work because ResumePerform can fire recursively within handler execution — each call creates a ResumePerformFrame, forming a call stack.

A group of N mutually recursive functions desugars to a single ResumeHandle. The handler contains a Branch with one arm per function. "Calling" a function is a ResumePerform that tags the payload with which function to call.

For two mutually recursive functions A and B:

```
ResumeHandle(resumeHandlerId,
  body: <continuation — the code that uses the functions>,
  handler: Chain(
    ExtractIndex(0),                      // extract payload from [payload, state]
    All(
      Branch({
        CallA: Chain(ExtractField("value"), bodyA),
        CallB: Chain(ExtractField("value"), bodyB),
      }),
      Constant(null),                     // state passthrough (unused)
    )
  )
)
```

Where:
- `callA` desugars to `Chain(Tag("CallA"), ResumePerform(resumeHandlerId))`
- `callB` desugars to `Chain(Tag("CallB"), ResumePerform(resumeHandlerId))`

When the continuation calls `callA(x)`:
1. ResumePerform fires with payload `{ kind: "CallA", value: x }`
2. Engine creates a ResumePerformFrame (body is NOT torn down)
3. Handler receives `[{ kind: "CallA", value: x }, null]`
4. Handler extracts payload, branches on "CallA", runs bodyA with `x`
5. If bodyA calls `callB(y)` mid-pipeline — another ResumePerform, another ResumePerformFrame. bodyA is preserved.
6. bodyB runs with `y`, completes with result `z`
7. `z` flows back to bodyA's perform site. bodyA continues with `z`.
8. bodyA completes with its result, which flows back to the continuation's perform site.

This is general recursion with a proper call stack. Non-tail calls work — the caller's pipeline is preserved across the call because ResumeHandle doesn't tear down the body.

### Self-recursion

Self-recursion is the degenerate case with one function:

```ts
defineRecursiveFunction(
  (self) => pipe(
    process,
    branch({
      Retry: self,
      Done: identity,
    }),
  ),
  (fn) => pipe(setup, fn, deploy),
)
```

Each recursive call creates a ResumePerformFrame. For tail recursion (like the branch above), this accumulates frames — O(n) frames for n iterations. `loop` (RestartHandle-based) is O(1) frames for tail recursion because it tears down the body each iteration. So `loop` remains the right tool for iteration; `defineRecursiveFunction` is for general recursion where calls can be non-tail.

### What replaces `self`?

Today, `self` (`Step(Root)`) lets the workflow call itself. With `defineRecursiveFunction`, the workflow wraps itself:

```ts
defineRecursiveFunction(
  (self) => pipe(
    process,
    branch({
      Retry: self,
      Success: identity,
    }),
  ),
  (workflow) => workflow,
)
```

The identity continuation `(workflow) => workflow` means the defined function IS the workflow.

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

## Relationship to bind

The mechanism is a generalization of bind. In bind, the handler returns a captured runtime value (a JSON value stored in the ResumeHandle's state). Here, the handler *executes a pipeline* — the function body is embedded directly in the handler DAG as a static action tree.

Another way to see it: bind captures a value and the handler returns it. `defineRecursiveFunctions` captures nothing (state is unused) and the handler *is* the function — the pipeline that would otherwise have been a registered step. The "lazy execution" is the ResumePerform/handler mechanism: the function body doesn't run until it's called, because it lives in the handler, and the handler only runs when a ResumePerform fires.

This means the function body has the same expressive power as any other pipeline. It can contain Invoke nodes (external handlers), other ResumePerforms (recursive calls), nested Handles, All/ForEach, everything. The handler DAG isn't limited to builtins — it's a full action tree.

## Open questions

1. **`defineRecursiveFunctions` type safety.** The current `stepRef` is `TypedAction<any, any, N>` — untyped. Can the new API do better? The call tokens need types (input/output of the function they call). But with mutual recursion, function A's output might depend on function B's output and vice versa. TypeScript can't infer circular types from a callback. We might need explicit type parameters: `defineRecursiveFunctions<[In1, Out1], [In2, Out2]>(...)`. Worse ergonomics than `stepRef` but more type-safe.

2. **Interaction with RESUME_VS_RESTART_HANDLERS.** The desugaring uses ResumeHandle/ResumePerform. This refactor depends on RESUME_VS_RESTART_HANDLERS landing first (or uses the current Handle/Perform with Resume output, which works today but is less clean).

3. **Stack depth for tail recursion.** Each function call accumulates a ResumePerformFrame. For tail-recursive patterns (loop iterations), this means O(n) frames. `loop` (RestartHandle-based) is O(1) because it tears down the body. So `loop` should remain the preferred tool for iteration. `defineRecursiveFunction` is for general recursion where non-tail calls need the caller's continuation preserved.

4. **Single-function convenience.** `defineRecursiveFunction` (singular) for self-recursion is common enough to warrant its own combinator. It's a thin wrapper around the general form.

5. **State field.** The ResumeHandle carries a `captured_value`/state. For function dispatch, there's no state to maintain — the state is unused (null). This works but wastes a field. Minor.
