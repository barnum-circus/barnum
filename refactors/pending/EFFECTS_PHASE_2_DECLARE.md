# Phase 2: Variable Declarations (ReadVar Effect)

## Goal

Implement `bind` as the first real algebraic effect. This exercises the resume path: a Perform fires, the Handle reads from its opaque state and Resumes with the value. End-to-end validation that the Phase 1 substrate works for data flow.

**Phase 2 covers concurrent bindings only.** Sequential bindings (where a binding depends on previously-bound values) are deferred to Phase 2a — see `EFFECTS_PHASE_2A_SEQUENTIAL_DECLARE.md`.

## Prerequisites

Phase 1 (Effect Substrate) complete.

## Surface syntax

The canonical form is an array of bindings followed by a body callback:

```ts
bind([exprA, exprB], ([a, b]) => body)
```

All bindings are actions (Pipeable) — evaluated concurrently with the bind's pipeline input.

The body callback receives an array of all VarRefs, one per binding. VarRefs are destructured positionally.

### Example

```ts
bind([fetchUser, fetchConfig], ([user, config]) =>
  pipe(user, processWithConfig(config))
)
```

Both `fetchUser` and `fetchConfig` run concurrently. The body receives VarRefs for both.

### `bindInput` — convenience for the common case

The most common pattern is binding the pipeline input itself so it can be referenced multiple times without threading:

```ts
bindInput<WorktreeEnv>((env) => pipe(
  pipe(env, pick("worktreePath"), commit).drop(),
  pipe(env, pick("branch", "description"), preparePRInput, createPR),
))
```

`bindInput` is syntactic sugar for `bind([identity()], ([input]) => body(input))`.

## How bind compiles

Each binding gets its own effectId — the natural HOAS representation where each binder creates a fresh name. This avoids the dynamic typing problem that a shared effectId with runtime index would introduce (see "Why not a shared effectId" below).

```
bind([fetchUser, fetchConfig], ([user, config]) => body)

// user   = Perform(effectId_0)   — no payload
// config = Perform(effectId_1)   — no payload

// Pseudo-AST notation:
//   Handle(effectId, handler, body)

// Compiles to:
Chain(
  All(fetchUser, fetchConfig, Identity),       // → [User, Config, Input]
  Handle(effectId_0, readVar(0),
    Handle(effectId_1, readVar(1),
      Chain(ExtractIndex(2), body)             // body gets pipeline_input
    )
  )
)
```

`All(fetchUser, fetchConfig, Identity)` evaluates all bindings concurrently AND preserves the pipeline input (via Identity). The result is a tuple `[User, Config, pipeline_input]`.

Handle initializes its state to the pipeline value (a one-line engine change: `state: None` → `state: Some(value.clone())`). All nested Handles receive the same tuple as state. Each handler extracts the bound value at its known index: `readVar(n)` does `ExtractField("state") → ExtractIndex(n) → Tag("Resume")`. The Perform carries no payload — the effectId alone identifies which binding.

N bindings produce N nested Handle frames. This is the natural representation of N lexical bindings — `let a = ... in let b = ... in body`. Each `let` is a scope, each Handle is a scope.

### Why not a shared effectId

An alternative design uses one shared effectId with the binding index as payload and a runtime index lookup. This is dynamically typed — the index is a runtime value and each tuple element has a different type, so the result is `unknown`. The per-binding-effectId design eliminates this: each handler extracts a statically-known index, so the types are preserved through the TS compiler.

### Engine change: Handle initializes state from pipeline value

The only engine change needed: when creating a Handle frame, set `state: Some(value.clone())` instead of `state: None`. The body still receives the pipeline value unchanged. This is a one-line change in the `FlatAction::Handle` arm of `advance()`.

## Function definitions

All functions introduced in this doc, with concrete implementations.

### `readVar(n)` — handler DAG for the nth binding

Returns an action that extracts the nth value from the Handle's state tuple and resumes with it. This is the handler DAG passed to each Handle in the compiled output.

When a Perform fires, the engine calls the handler with `{ payload, state }`. For bind, `state` is the full All output tuple (e.g. `[User, Config, pipeline_input]`). The handler extracts `state[n]` and wraps it as `{ kind: "Resume", value: state[n] }`.

```ts
function readVar(n: number): Action {
  return pipe(extractField("state"), extractIndex(n), tag("Resume")) as Action;
}
```

Type parameters on `extractField`, `extractIndex`, and `tag` are elided — this is an internal function, not user-facing. The expanded AST is `Chain(ExtractField("state"), Chain(ExtractIndex(n), Tag("Resume")))`.

### `VarRef<TValue>` — typed reference to a bound value

A VarRef is a `Perform` node wrapped with phantom types. Input is `never` because VarRefs don't consume pipeline input — they raise an effect. Output is `TValue`, the concrete type of the bound value.

```ts
type VarRef<TValue> = TypedAction<never, TValue>;

function createVarRef<TValue>(effectId: EffectId): VarRef<TValue> {
  return typedAction({ kind: "Perform", effectId });
}
```

### `InferVarRefs<TBindings>` — map bindings to VarRef types

Maps each binding's output type to a VarRef. TypeScript resolves `ExtractOutput` from each binding expression.

```ts
type InferVarRefs<TBindings extends Pipeable<any, any>[]> = {
  [K in keyof TBindings]: VarRef<ExtractOutput<TBindings[K]>>;
};
```

### `bind()` — the user-facing function

```ts
let nextEffectId = 0;

function bind<TIn, TBindings extends Pipeable<TIn, any>[], TOut>(
  bindings: [...TBindings],
  body: (vars: InferVarRefs<TBindings>) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TOut> {
  // 1. Gensym one effectId per binding.
  const effectIds = bindings.map(() => EffectId(nextEffectId++));

  // 2. Create VarRefs (Perform nodes) for each binding.
  const varRefs = effectIds.map((id) => createVarRef(id));

  // 3. Invoke the body callback with the VarRefs.
  const bodyAction = body(varRefs as any);

  // 4. Build nested Handles from inside out.
  //    Innermost: extract pipeline_input (last All element) → user body
  const pipelineInputIndex = bindings.length;
  let inner = chain(extractIndex(pipelineInputIndex), bodyAction);
  for (let i = effectIds.length - 1; i >= 0; i--) {
    // Handle and Perform are Phase 1 AST nodes — no constructor yet.
    inner = typedAction({
      kind: "Handle",
      effectId: effectIds[i],
      handler: readVar(i),
      body: inner as Action,
    });
  }

  // 5. All(...bindings, identity()) → nested Handles
  return chain(all(...bindings, identity()), inner);
}
```

**Note:** Phase 2 only accepts `Pipeable` bindings (concurrent). Phase 2a adds function bindings for sequential dependencies.

### `bindInput()` — bind the pipeline input

Convenience wrapper for the common pattern of capturing the pipeline input as a VarRef. The body's pipeline input is `never` — the input is dropped, so the body **must** access it through the VarRef. This forces explicit data flow.

```ts
function bindInput<TIn, TOut>(
  body: (input: VarRef<TIn>) => Pipeable<never, TOut>,
): TypedAction<TIn, TOut> {
  return bind([identity<TIn>()], ([input]) =>
    pipe(drop(), body(input)) as Pipeable<TIn, TOut>,
  );
}
```

The `drop()` discards the pipeline value before entering the user's body. Everything inside the body starts from a VarRef, not from implicit pipeline threading. This eliminates `tap` and `augment` — there's no implicit context to preserve or merge back into.

## The HOAS pattern

Each binding gensyms its own fresh `EffectId`. Each VarRef is a `Perform(effectId)` node with no payload — the effectId alone identifies the binding. TypeScript's lexical scoping ensures VarRefs can only be used within the callback body.

Per the HOAS pattern:
1. Gensym a fresh `EffectId` **per binding**
2. Create nested Handles, one per binding, each keyed on its effectId
3. Provide `Perform(effectId_i)` wrappers to the callback as opaque `Pipeable` nodes

For nested binds, each binding across all bind calls has its own `EffectId`. Inner VarRefs are caught by the nearest matching Handle. Outer VarRefs bubble past inner Handles (wrong effectId) and are caught by the outer Handle.

## What this replaces

| Current pattern | With bind |
|---|---|
| `augment(pipe(extract, transform))` | `bindInput` + VarRef: reference input and transform result independently |
| `tap(sideEffect)` (to preserve context) | Side effect in body with `.drop()`, context from VarRef |
| `pick("field1", "field2")` (to narrow for invariance) | Still needed — VarRef gives the full value, pick narrows it |

`augment` and `tap` become unnecessary for context threading. `pick` remains necessary for handler input narrowing (invariance at serialization boundaries).

## Demo migration: identify-and-address-refactors

The `ImplementAndReview` step is the primary beneficiary. It currently uses `tap`, `augment`, and `pick` with verbose type annotations to thread a 5-field context through a sequence of side effects.

### Before (current)

```ts
type WorktreeEnv = Refactor & { worktreePath: string; branch: string };

ImplementAndReview: pipe(
  // Side effects: tap preserves WorktreeEnv, pick narrows for invariance
  tap(pipe(
    pick<WorktreeEnv, ["worktreePath", "description"]>("worktreePath", "description"),
    implement,
  )),
  tap(pipe(pick<WorktreeEnv, ["worktreePath"]>("worktreePath"), commit)),

  // Type-check/fix cycle
  tap<WorktreeEnv, "TypeCheck">(stepRef("TypeCheck")),

  // Judge/revise loop
  tap<WorktreeEnv, "TypeCheck">(
    loop(
      pipe(drop(), judgeRefactor, classifyJudgment).branch({
        NeedsWork: pipe(applyFeedback.drop(), stepRef("TypeCheck"), recur<any, any>()),
        Approved: done<any, any>(),
      }),
    ),
  ),

  // Create PR: augment merges { prUrl } back into WorktreeEnv
  augment<WorktreeEnv, { prUrl: string }>(pipe(
    pick<WorktreeEnv, ["branch", "description"]>("branch", "description"),
    preparePRInput,
    createPR,
  )),
),
```

**Problems:**
- Every `tap` needs an explicit `<WorktreeEnv>` type annotation to prevent narrowing.
- `augment` needs both `<WorktreeEnv, { prUrl: string }>` type parameters.
- `pick` duplicates field names as both type parameters and string arguments.
- The intent (preserve context, run side effect) is obscured by the machinery.

### After (with bindInput)

```ts
ImplementAndReview: bindInput<WorktreeEnv>((env) => pipe(
  // Note: the explicit <WorktreeEnv> annotation is needed today because
  // handler types aren't statically declared. Once we add static handler
  // type declarations, bindInput will infer the type from context and
  // this annotation disappears.

  // Side effects: env VarRef provides context, .drop() discards output
  pipe(env, pick("worktreePath", "description"), implement).drop(),
  pipe(env, pick("worktreePath"), commit).drop(),

  // Type-check/fix cycle
  pipe(env, pick("worktreePath"), stepRef("TypeCheck")).drop(),

  // Judge/revise loop
  loop(
    pipe(drop(), judgeRefactor, classifyJudgment).branch({
      NeedsWork: pipe(applyFeedback.drop(), stepRef("TypeCheck"), recur<any, any>()),
      Approved: done<any, any>(),
    }),
  ).drop(),

  // Create PR — no augment needed, env provides context independently
  pipe(env, pick("branch", "description"), preparePRInput, createPR),
)),
```

**What changed:**
- `tap(X)` → `X.drop()` — no `tap` needed to preserve context; `env` VarRef gives it back anytime.
- `augment<WorktreeEnv, { prUrl: string }>(...)` → just `pipe(env, ...)` — no merge-back step; the VarRef is always available.
- `<WorktreeEnv>` type annotations on `tap` disappear — `env` carries the type.
- `pick` no longer needs explicit type parameters — `env` produces `WorktreeEnv`, so `pick("worktreePath")` infers from the pipeline.
- `pick` is still used for invariance narrowing (serialization boundaries require exact types).

### Demo migration: convert-folder-to-ts

#### Before (current)

```ts
forEach(
  pipe(
    pipe(
      extractField<FileEntry, "file">("file"),
      migrate({ to: "Typescript" }),
    ).augment().pick("content", "outputPath"),
    writeFile,
  ),
)
```

`augment` runs `extractField → migrate`, then merges `{ content }` back into the original `FileEntry` to produce `{ content, file, outputPath }`. Then `pick` narrows to `{ content, outputPath }` for `writeFile`.

#### After (with bindInput)

```ts
forEach(
  bindInput<FileEntry>((entry) => pipe(
    pipe(entry, extractField("file"), migrate({ to: "Typescript" })),
    // Pipeline value is now { content }. entry VarRef still gives FileEntry.
    // Merge { content } with { outputPath } from the original entry.
    all(identity(), pipe(entry, pick("outputPath"))),
    merge(),
    writeFile,
  )),
)
```

**What changed:**
- No `augment` — data flow is explicit. `entry` VarRef provides the original `FileEntry` whenever needed.
- `all(identity(), pipe(entry, pick("outputPath")))` collects `[{ content }, { outputPath: string }]`, then `merge()` combines them into `{ content, outputPath }`.
- No explicit type parameters on `extractField` — `entry` produces `FileEntry`, so `extractField("file")` infers.

## Test strategy

### TypeScript compilation tests (AST output)

These tests verify that `bind()` produces the correct AST structure. Compare the output of `bind(...)` against expected `Action` objects using `toEqual` (non-enumerable methods are invisible to `toEqual`).

1. **Single binding**: `bind([exprA], ([a]) => body)` produces:
   - `Chain(All(exprA, Identity), Handle(effectId_0, readVar(0), Chain(ExtractIndex(1), body)))`.
   - Verify: outer All has 2 actions (binding + Identity). Handle wraps the body. ExtractIndex(1) feeds pipeline_input to body.

2. **Two bindings**: `bind([exprA, exprB], ([a, b]) => body)` produces:
   - `Chain(All(exprA, exprB, Identity), Handle(effectId_0, readVar(0), Handle(effectId_1, readVar(1), Chain(ExtractIndex(2), body))))`.
   - Verify: outer All has 3 actions. Two nested Handles with distinct effectIds. readVar indices match positions. ExtractIndex(2) for pipeline_input.

3. **VarRef is a Perform node**: Each VarRef passed to the body callback is `{ kind: "Perform", effectId: <unique> }` with `TypedAction<never, TValue>` phantom types.

4. **EffectIds are unique across bindings**: Two bindings produce two different effectIds. Two separate `bind` calls produce four distinct effectIds total.

5. **readVar(n) structure**: `readVar(0)` is `Chain(ExtractField("state"), Chain(ExtractIndex(0), Tag("Resume")))`. Verify for n=0, n=1, n=2.

6. **bindInput compiles to bind with identity**: `bindInput(body)` produces the same AST as `bind([identity()], ([input]) => body(input))`.

### TypeScript type-level tests (tsc compilation)

These tests verify compile-time type safety by asserting that certain expressions typecheck (or fail to typecheck).

1. **VarRef output type matches binding output**: If `computeName` is `Pipeable<SomeInput, string>`, then the VarRef `name` in `bind([computeName], ([name]) => ...)` is `VarRef<string>` (i.e. `TypedAction<never, string>`). Verify by piping the VarRef into an action that expects `string` input — should compile. Piping into an action that expects `number` input — should fail.

2. **Body input type is the bind input type**: The body callback returns `Pipeable<TIn, TOut>` where `TIn` matches the bind's input. If `bind` receives `Pipeable<Config, ...>` bindings, the body pipeline's input type is `Config`.

3. **Multiple bindings infer distinct VarRef types**: `bind([stringAction, numberAction], ([s, n]) => ...)` gives `s: VarRef<string>` and `n: VarRef<number>`. Mixing them up (passing `s` where `number` is expected) fails.

4. **Bind output type matches body output type**: `bind([...], ([...]) => actionReturningFoo)` has output type `Foo`.

5. **bindInput infers VarRef type from context**: `bindInput<FileEntry>((entry) => pipe(entry, extractField("file")))` — `entry` is `VarRef<FileEntry>`, output type is `string` (FileEntry["file"]).

### Rust engine tests

These tests verify the runtime behavior of the Handle/Perform substrate when driven by bind-shaped ASTs. Construct the ASTs directly in Rust (no TS macro involved).

1. **Single binding, single read**: Construct `Chain(All(Constant(42), Identity), Handle(e0, readVar(0), Chain(Perform(e0), Invoke(echo))))`. Advance with input `"input"`. All produces `[42, "input"]`. Handle state = `[42, "input"]`. Perform fires, handler extracts state[0] = 42, resumes. Chain trampolines to echo with 42. Complete echo. Verify workflow output.

2. **Single binding, body ignores VarRef**: `Chain(All(Constant(42), Identity), Handle(e0, readVar(0), Chain(ExtractIndex(1), Invoke(echo))))`. Body extracts pipeline_input (index 1) and passes it to echo, never Performing. Handle exits normally. Verify echo receives `"input"`.

3. **Two bindings, two reads**: `Chain(All(Constant("alice"), Constant(99), Identity), Handle(e0, readVar(0), Handle(e1, readVar(1), Chain(Perform(e0), Chain(Invoke(mid), Perform(e1))))))`. First Perform → handler0 → resumes with "alice". Chain to mid. Second Perform → handler1 → resumes with 99. Verify mid receives "alice", workflow exits with 99.

4. **Two bindings, reads in reverse order**: Same as test 3 but body is `Chain(Perform(e1), Perform(e0))`. Perform(e1) is caught by inner Handle (effectId matches). Perform(e0) bubbles past inner Handle, caught by outer. Verify correct values returned for each.

5. **Nested binds**: Outer bind binds "outer", inner bind binds "inner". Inner VarRef reads "inner" from inner Handle. Outer VarRef bubbles past inner Handle, reads "outer" from outer Handle. Verify both values correct. AST: `Handle(e_outer, readVar(0), Chain(ExtractIndex(1), Handle(e_inner, readVar(0), Chain(ExtractIndex(1), Chain(Perform(e_outer), Perform(e_inner))))))`. The outer All produces `["outer", <tuple from inner All>]`. The inner All produces `["inner", pipeline_input]`.

6. **Bind inside ForEach**: `ForEach(Chain(All(Identity, Identity), Handle(e0, readVar(0), Chain(Perform(e0), Invoke(echo)))))`. Input `[10, 20]`. Each iteration binds its own element. Verify echo receives 10 and 20 respectively (Handle frames are per-iteration, not shared).

7. **Bind inside All (shared outer Handle)**: `Handle(e_outer, readVar(0), Chain(ExtractIndex(1), All(Chain(Perform(e_outer), Invoke(a)), Chain(Perform(e_outer), Invoke(b)))))`. All tuple = `[42, pipeline_input]`. Both branches Perform(e_outer). First dispatches handler, second stashed (Handle suspended). Handler resumes, first branch continues. Sweep retries second Perform. Verify both branches receive 42.

8. **Handler receives correct state shape**: Construct a Handle with a TS handler (not builtin) so we can inspect the dispatch value. Advance, trigger Perform. Verify the handler dispatch value is `{ "payload": <perform_input>, "state": <all_output_tuple> }`.

9. **readVar(n) produces correct HandlerOutput**: Wire up readVar(1) as the handler for a Handle. State = `["a", "b", "c"]`. Trigger Perform. Drive builtins. Verify the handler output is `{ "kind": "Resume", "value": "b" }` and the resumed value delivered to the Perform site is `"b"`.

## No new builtins

The per-binding-effectId design uses only existing builtins (`ExtractIndex`, `ExtractField`, `Tag`). No new builtins are needed.

## Deliverables

1. `bind()` TypeScript function (concurrent bindings only, per-binding effectId)
2. `bindInput()` convenience wrapper
3. `VarRef` type alias and `createVarRef()` construction
4. `InferVarRefs` mapped type
5. `readVar(n)` handler DAG factory
6. Nested Handle compilation in `bind()`
7. Engine change: Handle initializes state from pipeline value
8. TypeScript compilation tests (AST structure)
9. TypeScript type-level tests (tsc)
10. Rust engine tests (runtime behavior)
11. Demo migration: identify-and-address-refactors `ImplementAndReview` step
12. Demo migration: convert-folder-to-ts `forEach` body
