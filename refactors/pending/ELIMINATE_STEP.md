# Eliminate Step

## Motivation

Steps exist to give pipelines names for reuse and recursion. This requires: `StepAction`/`StepRef` in both ASTs, `Config.steps`, `ConfigBuilder.registerSteps()` (two overloads), `stepRef()`, `ValidateStepRefs<R>`, flattening passes 1 and 2, `FlatAction::Step`, `StepName`, `StepTarget`, and associated error variants.

Naming is already solved — pipelines are values (`const x = pipe(...)`). The only thing steps provide beyond that is recursion (self-reference and mutual reference). That can be expressed with resumptive handlers.

## Core mechanism

`bind` uses a ResumeHandle where the handler returns a captured value. `defineRecursiveFunctions` uses a ResumeHandle where the handler *is* the function — the function body is embedded directly in the handler DAG. The function doesn't run until called (Perform fires), and the caller's pipeline is preserved across the call (resume semantics). Recursive calls fire Perform recursively within handler execution, forming a call stack of ResumePerformFrames.

## API

```ts
// Define the functions. Returns a curried combinator.
const withFns = defineRecursiveFunctions((fnA, fnB) => [
  pipe(process, fnB),    // A's body — can call B
  pipe(transform, fnA),  // B's body — can call A
]);

// Apply to a body. Returns a TypedAction.
withFns((fnA, fnB) => pipe(setup, fnA, deploy))
```

The call tokens (`fnA`, `fnB`) are the same values in both callbacks. They're `Chain(Tag("Call0"), Perform(effectId))` — tagged Performs. The first callback uses them for recursion inside function bodies. The second uses them for initial calls in the workflow body. Both execute inside the Handle's scope, so the engine finds the enclosing Handle when walking ancestors.

Single-function convenience:

```ts
const withSelf = defineRecursiveFunction((self) =>
  pipe(process, branch({ Retry: self, Done: identity }))
);

withSelf((fn) => pipe(setup, fn, deploy))
```

This subsumes `Step(Root)` / `self`. No separate mechanism needed.

## Desugaring

`withFns((fnA, fnB) => body)` produces:

```
Handle(effectId,
  body: body,                               // the workflow body using fnA, fnB
  handler: Chain(
    ExtractIndex(1),                        // state from [payload, state]
    Branch({
      Call0: Chain(ExtractField("value"), bodyA),
      Call1: Chain(ExtractField("value"), bodyB),
    }),
    Tag("Resume")
  )
)
```

One Handle wrapping the body. The handler dispatches to function bodies by tag. No duplication — the handler appears once.

For the single-function case, no Branch needed. The handler runs the body directly.

### Recursive calls

When `bodyA` calls `fnB` mid-pipeline: Perform fires, engine walks ancestors and finds the enclosing Handle, handler runs (dispatches to bodyB). `bodyA`'s pipeline is preserved — it resumes when bodyB completes. The call stack is a chain of suspended handler invocations.

Tail recursion accumulates O(n) frames. `loop` (RestartHandle) is O(1) for tail recursion. `loop` stays for iteration; `defineRecursiveFunction` is for general recursion.

## Phases

### Phase 1: Add `defineRecursiveFunctions`

Purely additive. Implement in `libs/barnum/src/`. Desugars to Handle/Perform with Resume output. Tests for self-recursion, mutual recursion, non-tail calls.

### Phase 2: Migrate consumers

Non-recursive steps become `const` declarations. `self`/`stepRef` usage migrates to `defineRecursiveFunction(s)`.

### Phase 3: Remove Step

Delete `StepAction`, `StepRef`, `StepName`, `Config.steps`, `registerSteps()`, `stepRef()`, `ValidateStepRefs`, flattener step handling, `FlatAction::Step`. Config becomes `{ workflow: Action }`.

## Open questions

1. **Type safety.** Can call tokens carry input/output types? With mutual recursion, TypeScript can't infer circular types. May need explicit type parameters.

2. **Interaction with RESUME_VS_RESTART_HANDLERS.** Phase 1 uses current Handle/Perform with Resume output. After the refactor, desugaring becomes cleaner (direct ResumeHandle/ResumePerform).
