# Declare Bindings

## Motivation

Barnum pipelines thread data through each step as a single value. When a step produces a result that later steps need, the intervening steps must accept and pass through fields they don't use — classic prop drilling. The current workarounds (`tap`, `augment`, `withResource`) keep the data flowing but add ceremony and obscure intent.

A `declare` combinator introduces variable bindings: evaluate expressions, capture results, and make them available as AST references inside a callback body. The callback runs at definition time in JavaScript, producing an AST that the Rust scheduler later executes.

```ts
// Barnum today — manual threading via augment + tap + pick:
pipe(
  deriveBranch,
  createWorktree,
  tap(pipe(pick("worktreePath", "description"), implement)),
  tap(pipe(pick("worktreePath"), commit)),
)

// With declare:
declare({
  worktree: pipe(deriveBranch, createWorktree),
}, ({ worktree }) =>
  pipe(
    worktree.then(pick("worktreePath", "description")).then(implement).dropOutput(),
    worktree.then(pick("worktreePath")).then(commit),
  ),
)
```

`worktree` in the callback is not a value — it's a `TypedAction<never, T>` AST node. Placing it in a pipeline tells the scheduler "resolve this variable from the environment." The callback is metaprogramming: JavaScript code that produces an AST.

### Realistic example: deployment pipeline

A CI/CD workflow that derives several values from input and uses them at scattered points throughout the pipeline. Today, every intermediate step must accept and pass through fields it doesn't use.

```ts
// Input type:
type DeployInput = {
  repo: string;
  sha: string;
  environment: string;
};

// Handlers — each accepts only what it needs (invariant types):
const buildImage     = createHandler<{ repo: string; sha: string }, { imageTag: string }>(...);
const runTests       = createHandler<{ imageTag: string }, { passed: boolean }>(...);
const deployToK8s    = createHandler<{ imageTag: string; environment: string }, { podName: string }>(...);
const notifySlack    = createHandler<{ repo: string; environment: string; podName: string }, void>(...);
const updateDashboard = createHandler<{ repo: string; sha: string; podName: string }, void>(...);
```

Without `declare`, you must thread everything through the entire pipeline:

```ts
// Without declare — every step must carry fields it doesn't need.
// The pipeline value grows to include all fields any future step might use.
pipe(
  // Start: { repo, sha, environment }
  augment(pipe(pick("repo", "sha"), buildImage)),
  // Now: { repo, sha, environment, imageTag }
  tap(pipe(pick("imageTag"), runTests)),
  // Still: { repo, sha, environment, imageTag }
  augment(pipe(pick("imageTag", "environment"), deployToK8s)),
  // Now: { repo, sha, environment, imageTag, podName }
  tap(pipe(pick("repo", "environment", "podName"), notifySlack)),
  // Still: { repo, sha, environment, imageTag, podName }
  pipe(pick("repo", "sha", "podName"), updateDashboard),
)
```

Every step threads `repo`, `sha`, and `environment` through augment/tap/pick even though most steps don't use them. The pipeline value is a growing accumulation of every field any step has ever produced.

With `declare` — bind the contextual values, use nested declares when a mid-pipeline result is needed later:

```ts
// Outer declare: bind values derived from the initial input.
// These are evaluated in parallel before the body starts.
declare({
  input: identity<DeployInput>(),
  image: pipe(pick("repo", "sha"), buildImage),
}, ({ input, image }) =>
  pipe(
    // runTests: just needs imageTag from image
    image.then(pick("imageTag")).then(runTests).dropOutput(),

    // deployToK8s: needs imageTag from image + environment from input.
    // Assemble its exact input from the two variables.
    parallel(
      image.then(pick("imageTag")),
      input.then(pick("environment")),
    ).then(merge()).then(deployToK8s),

    // deployToK8s output: { podName }
    // podName is a mid-pipeline result. Nested declare captures it
    // as a variable so downstream steps can reference it alongside
    // the original input and image.
    declare({
      deploy: identity<{ podName: string }>(),
    }, ({ deploy }) =>
      pipe(
        // notifySlack: repo + environment from input, podName from deploy
        parallel(
          input.then(pick("repo", "environment")),
          deploy.then(pick("podName")),
        ).then(merge()).then(notifySlack).dropOutput(),

        // updateDashboard: repo + sha from input, podName from deploy
        parallel(
          input.then(pick("repo", "sha")),
          deploy.then(pick("podName")),
        ).then(merge()).then(updateDashboard),
      ),
    ),
  ),
)
```

What this demonstrates:

1. **Outer `declare`** binds `input` and `image` from the pipeline input. Both are available throughout the entire body without threading.

2. **Nested `declare`** captures `podName` after `deployToK8s` runs. The inner body can reference `input`, `image` (from the outer scope), and `deploy` (from the inner scope). No collisions — each gets a unique ID.

3. **Each handler receives exactly what it needs.** `notifySlack` gets `{ repo, environment, podName }` assembled from two variables. No step carries fields it doesn't use.

4. **No augment/tap/pick threading.** The pipeline value at each point is either the output of the previous step or ignored (via `.dropOutput()`). Context comes from variable references, not the pipeline.

Compare the two approaches:

| | Without `declare` | With `declare` |
|---|---|---|
| Pipeline value | Accumulates every field | Just the current step's output |
| Context access | `pick` from the bloated pipeline | `varRef.then(pick(...))` from variables |
| Adding a new step | Must check if pipeline has the right fields | Reference the right variable |
| Removing a step | Must verify downstream steps still have their fields | Variables are independent |

## Identity model: unique IDs, no names, no collisions

This is the most important design property and it permeates everything.

When the user writes `declare({ worktree: ... }, ({ worktree }) => ...)`, the key `"worktree"` is a **JavaScript-level name** — it exists only so the user can destructure the callback parameter. It is not stored in the AST. It does not appear anywhere in the serialized config. The Rust scheduler never sees it.

At definition time, `declare` assigns each binding a **globally unique ID** via a monotonic counter: `__declare_0`, `__declare_1`, etc. The `DeclareAction` node maps these IDs to binding ASTs. The `VarRef` nodes in the body carry these IDs. That's it.

```ts
// User writes:
declare({ a: foo, b: bar }, ({ a, b }) => pipe(a, b))

// AST produced:
{
  kind: "Declare",
  bindings: {
    "__declare_0": /* foo's AST */,
    "__declare_1": /* bar's AST */,
  },
  body: /* pipe(VarRef("__declare_0"), VarRef("__declare_1")) */
}
```

**Collisions are impossible.** The counter is global and monotonically increasing. Every binding ever created — across all `declare` calls, across nested scopes, across the entire config — gets a distinct ID. There is no name resolution. There is no shadowing. There is no scope chain to walk. A VarRef carries an ID; the environment is a flat `Map<Id, Value>`; lookup is O(1).

Even in pathological cases:

```ts
// User reuses the name "x" in nested scopes — no collision
declare({ x: foo }, ({ x: outer }) =>
  declare({ x: bar }, ({ x: inner }) =>
    parallel(outer, inner),  // outer is __declare_0, inner is __declare_1
  ),
)
```

The JS-level names `outer` and `inner` (or even both named `x`) are irrelevant. The IDs are `__declare_0` and `__declare_1`. The environment contains both. The VarRefs point to the right one. No resolution logic needed.

```ts
// Same binding referenced 5 times — same ID, same value
declare({ data: computeData }, ({ data }) =>
  pipe(
    data.then(stepA),
    data.then(stepB),
    parallel(data.then(stepC), data.then(stepD), data),
  ),
)
```

All five `data` references are the same VarRef node with the same ID. The binding is evaluated once. Every reference resolves to the same cached value.

### The scheduler's view

The scheduler maintains a single `HashMap<DeclareId, Value>`. When entering a `Declare` node:

1. Evaluate all bindings in parallel (each receives the pipeline input).
2. Insert `(id, value)` pairs into the map.
3. Execute the body.

When encountering a `VarRef { id }`: look up `id` in the map. Return the value. Done.

No stack of scopes. No chain walking. No name resolution. Just a flat map with unique keys. Nested `declare` blocks simply add more entries to the same map — the IDs can never collide because they're generated by a monotonic counter.

## Surface API

```ts
function declare<
  TIn,
  TBindings extends Record<string, Pipeable<TIn, unknown>>,
  TOut,
  TRefs extends string = never,
>(
  bindings: TBindings,
  body: (vars: {
    [K in keyof TBindings]: TypedAction<never, ExtractOutput<TBindings[K]>>
  }) => Pipeable<TIn, TOut, TRefs>,
): TypedAction<TIn, TOut, TRefs>
```

- **`bindings`**: Object mapping JS-level names to ASTs. Each AST receives the pipeline input (`TIn`) and produces its bound value. The keys are for the user; the AST uses generated IDs.
- **`body`**: Callback that receives VarRef AST nodes (one per binding) and returns the body AST. Each VarRef is `TypedAction<never, T>` — takes no pipeline input, produces the bound type. The body also receives the original pipeline input as its pipeline value.
- **Return type**: `TypedAction<TIn, TOut>`. The declare block's input is the pipeline input; its output is the body's output.

### Example

```ts
declare({
  branch: pipe(extractField("description"), deriveBranch),
}, ({ branch }) =>
  pipe(
    implement,       // doesn't need branch — just uses pipeline input
    commit,          // doesn't need branch either
    branch.then(createPR),  // only this step uses branch
  ),
)
```

## Implementation: new AST nodes (Declare + VarRef)

### AST additions

```ts
// New Action variant
interface DeclareAction {
  kind: "Declare";
  bindings: Record<string, Action>;  // keys are unique IDs, not user names
  body: Action;                       // may contain VarRef nodes
}

// New BuiltinKind variant
{ kind: "VarRef"; id: string }  // the unique ID from the counter
```

### Why not TS-side closure conversion?

See the collapsed section below. Short version: the AST blowup is only ~3x (a constant factor, not explosive), but the real costs are elsewhere — every AST node type needs a recursive transform case, feature interactions compound quadratically, the scheduler can't produce meaningful error messages, and future features (RAII, lazy eval) require restructuring the entire desugaring instead of localizing changes in the scheduler. Since we control the scheduler, adding a native node is cheaper long-term than maintaining a closure conversion pass. See also `TS_VS_RUST_TRANSFORMS.md` for the general framework.

<details>
<summary>Full Approach B analysis (closure conversion)</summary>

### The technique

Closure conversion is a standard compiler technique. When a language has variables accessible from arbitrary positions in the AST and the target runtime only supports linear data flow, the compiler eliminates the free variables by threading them as an explicit environment parameter.

Each step `f: A → B` in the body becomes `parallel(pipe(extractIndex(0), f), extractIndex(1))`, transforming it to `[A, Env] → [B, Env]`. VarRef sentinels are replaced with `extractIndex` into the environment.

### Why it was rejected

**1. Every AST node type needs a transform case, and several are subtle.**

- **Loop**: Must restructure Continue/Break signals to carry env.
- **ForEach**: Can't iterate over `[items[], env]` — must extract, pair each item with env, run, extract results.
- **Parallel**: Output `[[out1, env], [out2, env], ...]` must be transposed to `[[out1, out2, ...], env]`.
- **Branch**: Match predicates need the pipeline value extracted from the tuple.

**2. Feature interactions compound quadratically.** N desugared features → O(N²) interaction cases.

**3. Scheduler opacity.** Error messages reference synthetic nodes instead of user-written variable names.

**4. Future features require re-doing the desugaring** instead of extending the scheduler.

**5. TypeScript type inference degrades** with deeply nested tuple types.

Closure conversion is the right choice when the target runtime is fixed (CPU, VM bytecode). The Barnum scheduler is ours to extend.

</details>

## Evaluation strategy

### Eager (chosen)

All bindings evaluate in parallel when the `Declare` node is entered. Each binding receives the pipeline input and runs to completion. Results are inserted into the environment. The body executes after all bindings complete.

```
Enter Declare → evaluate all bindings in parallel → store results → execute body
```

Call-by-value. Simple, predictable. Side effects happen in a known order: bindings first (concurrently), body second. If a binding is never referenced, it still executes.

### Lazy (deferred — not implementing now)

Bindings recorded as thunks. VarRef forces on first access; result memoized. Call-by-need.

What it gets you:
- Efficiency (don't compute unused bindings)
- Dependent bindings (binding B references binding A)
- Natural resource management (unreferenced binding never creates resource)

What it costs:
- Side effect ordering becomes unpredictable (Haskell's IO monad problem)
- Concurrent memo table needed for parallel branches referencing same lazy var
- Error locations become unpredictable (failure surfaces at VarRef, not Declare)

**Recommendation**: Start eager. Add `lazy_declare` later if needed. Don't default to lazy — it's Haskell's most controversial feature for good reason.

### Are we reinventing Haskell?

| Haskell | Barnum |
|---|---|
| `>>=` (monadic bind) | `pipe` / `.then()` |
| `do` notation | `pipe(a, b, c)` |
| `let` in `do` blocks | `declare` (this proposal) |
| `pure` / `return` | `constant` |
| `void` | `drop` |
| `fmap` | `forEach` (on arrays) |
| `<*>` (applicative) | `parallel` |
| Case analysis | `branch` |
| Recursion | `loop` / `stepRef` |

Adding lazy bindings with memoization makes the parallel even tighter. We should be deliberate about which language features we adopt.

## Relation to existing combinators

| Current pattern | With `declare` |
|---|---|
| `augment(pipe(extract, transform))` | `declare({ x: pipe(extract, transform) }, ({ x }) => ...)` |
| `tap(sideEffect)` | Side effect in a binding that's never referenced (eager) |
| `withResource({ create, action, dispose })` | See below |

### RAII: handlers declare their own cleanup

The key insight: RAII is a property of **handlers**, not of **bindings**. A handler that creates a resource knows how to clean it up. The binding site shouldn't need to specify cleanup — it should happen automatically.

#### The model

A handler can optionally declare a `dispose` function alongside its `handle` function:

```ts
const createWorktree = createHandler({
  inputValidator: z.object({ branch: z.string() }),
  handle: async ({ value }) => {
    // create worktree, return { worktreePath, branch }
  },
  dispose: async ({ value }) => {
    // value is the handler's output: { worktreePath, branch }
    // delete worktree at value.worktreePath
  },
});
```

The `dispose` function receives the handler's output value — the same value the pipeline sees. This is the Rust `Drop` trait pattern: the type knows how to clean itself up. The caller doesn't specify cleanup; the producer does.

#### How it interacts with declare

`declare` doesn't change. No `{ create, dispose }` pairs. No special syntax. The user writes:

```ts
declare({
  worktree: pipe(deriveBranch, createWorktree),
}, ({ worktree }) =>
  pipe(
    worktree.then(pick("worktreePath", "description")).then(implement),
    worktree.then(pick("worktreePath")).then(commit),
    worktree.then(pick("branch", "description")).then(createPR),
  ),
)
```

The scheduler knows that `createWorktree`'s output is disposable because the handler declared a `dispose` function. When the `declare` scope exits (body completes or fails), the scheduler runs `dispose` on the bound value automatically.

This is "free" — `declare` provides the scope boundaries, handlers provide the cleanup logic, and the scheduler connects them. No additional user code.

#### Scope exit = dispose trigger

The scheduler tracks which values in the environment were produced by handlers that have `dispose`. When a `Declare` frame is removed (body completed or errored):

1. Inspect all bindings in the frame's scope.
2. For each binding whose handler declared `dispose`: run `dispose` on the bound value.
3. Then deliver the body's result (or error) to the parent.

The guarantee: if a binding was successfully evaluated and its handler has `dispose`, cleanup runs. Body success or failure doesn't matter.

#### Nested declares give ordering for free

```ts
declare({
  worktree: pipe(deriveBranch, createWorktree),
}, ({ worktree }) =>
  declare({
    file: worktree.then(openTempFile),
  }, ({ file }) =>
    file.then(write),
    // inner scope exits → openTempFile's dispose runs (close file)
  ),
  // outer scope exits → createWorktree's dispose runs (delete worktree)
)
```

The inner scope's dispose runs first because the inner `declare` body completes before the outer `declare` body continues. File closed before worktree deleted. No explicit ordering needed — the nesting IS the ordering.

#### Serialization: dispose in the handler pool

Dispose actions are handler metadata, not AST structure. In the serialized config, the handler pool (see handler dedup in implementation doc) would carry dispose information:

```json
{
  "handlers": {
    "__handler_0": {
      "kind": "TypeScript",
      "module": "./git.ts",
      "func": "createWorktree",
      "dispose": {
        "kind": "TypeScript",
        "module": "./git.ts",
        "func": "deleteWorktree"
      }
    }
  }
}
```

The AST itself is unchanged — `InvokeAction` still just references a handler ID. The scheduler looks up the handler's dispose when it needs to clean up.

#### Error semantics

**Body succeeds, dispose succeeds**: Normal case. Declare produces the body's output.

**Body fails, dispose succeeds**: Cleanup ran correctly. Declare propagates the body's error.

**Body succeeds, dispose fails**: Body's work is done but cleanup leaked. Options: propagate dispose error (strict) or log and return body's output (pragmatic). Recommend strict — a leaked resource is an error.

**Body fails, dispose fails**: Propagate body's error (primary). Attach dispose error as suppressed/secondary.

**Binding fails**: Bindings evaluated in parallel. If one fails, the others that completed still need their dispose run. The scheduler tracks which bindings completed successfully.

#### This replaces withResource

`declare` + handler-level dispose subsumes `withResource` entirely. The user doesn't think about cleanup at the binding site at all — they just bind and use. Cleanup is the handler's responsibility.

```ts
// withResource today — user specifies cleanup:
withResource({
  create: createWorktree,
  action: pipe(implement, commit),
  dispose: deleteWorktree,
})

// declare + handler dispose — cleanup is automatic:
declare({
  worktree: pipe(deriveBranch, createWorktree),
}, ({ worktree }) =>
  pipe(
    worktree.then(implement),
    worktree.then(commit),
  ),
)
```

## Callbacks as scope boundaries: effects, error handling, and context

The `declare` callback pattern — "run some setup, execute a body, run some cleanup" — is a general mechanism. It's the same shape as try/catch, RAII, React context providers, and database transactions. All of them create a **scope** with entry/exit behavior controlled by the framework.

### What scopes give you

A scope has three properties:
1. **Entry**: something happens when the scope is entered (evaluate bindings, acquire resource, start transaction)
2. **Body**: the user's code runs within the scope
3. **Exit**: something happens when the scope exits (dispose resources, commit/rollback, catch errors)

The callback in TypeScript is the natural way to express "here is a region of code where X is true." The callback's extent IS the scope.

### How this generalizes

**RAII** (discussed above): Handlers declare cleanup. The scheduler runs cleanup when the enclosing `declare` scope exits. Entry = evaluate bindings. Exit = dispose.

**Error handling / catch**: A `tryCatch` combinator wraps a body in error-handling logic:

```ts
tryCatch(
  body,
  (error) => recoveryAction,  // callback receives error info
)
```

The callback here is on the TS side (definition time) — it constructs the recovery AST. The scheduler catches errors from the body and executes the recovery action instead of propagating the error. This is a scope where entry = try and exit = catch.

But `tryCatch` doesn't need the declare-style callback pattern — it's a combinator that takes two actions. It could be:

```ts
tryCatch({
  body: pipe(deploy, verify),
  catch: pipe(rollback, notify),  // receives the error as pipeline input
})
```

No callback needed because there's nothing to "capture" — unlike `declare` where the callback captures VarRef AST nodes.

**Timeouts**: A scope that limits execution time:

```ts
withTimeout(30_000, body)
```

Entry = start timer. Exit = cancel timer (if body completed) or cancel body (if timer fires). The scheduler's scope-exit logic handles cleanup. Again, no callback needed — just a combinator.

**Context / providers**: More speculative. React-style context would provide a value to all handlers within a scope:

```ts
provide({ database: dbConnection }, body)
```

This is fundamentally different from `declare` because context crosses the handler boundary — TypeScript subprocess handlers would need access to the provided value. `declare` variables are AST-level references that resolve in the scheduler; context would be runtime values passed through the handler invocation protocol. This requires changes to the handler IPC protocol, not just the AST.

Context is worth exploring separately, but it's a different mechanism from `declare` scopes.

### The pattern

All of these share the shape: **combinator creates a scope, scheduler manages entry/exit behavior.** The callback is only needed when the user needs to capture an opaque reference (VarRef in `declare`, restart in `scope`). Otherwise, the combinator just takes arguments.

What they share at the scheduler level:
- A frame type that tracks the scope's state
- Entry logic in `advance`
- Exit logic in `deliver` (or a new scope-exit path)
- Error path handling (what happens if the body fails?)

`declare` is the first scope mechanism. RAII is the first scope-exit behavior. Once both exist, adding more scope-exit behaviors (timeout cancellation, transaction rollback, error catching) is extending the same infrastructure.

### Implementation order

1. **`declare`** — scope with entry behavior (evaluate bindings). No exit behavior.
2. **Handler-level dispose** — scope-exit behavior triggered by handler metadata. `declare` provides the scope boundary.
3. **`tryCatch`** — scope-exit behavior triggered by errors. May or may not use `declare`; could be a standalone combinator.
4. **`withTimeout`** — scope-exit behavior triggered by time. Requires scheduler-level timer support.
5. **Context** — if ever. Requires handler IPC changes.

## Relationship to scopes and continuations

`declare` introduces a **scope** — a region of the AST where additional data is available. But the scope has no name, no label, no user-visible identity. It's purely structural: "within this callback, these VarRefs resolve." The scope boundary is the body's extent in the AST.

### Could Loop be implemented as a scope?

Loop is a hardcoded continuation: it captures a body `ActionId` and re-enters it on Continue. A general scope primitive could subsume it:

```ts
// Hypothetical: loop as scope + continuation
scope((restart) =>
  pipe(
    body,
    branch({
      Continue: restart,     // jump back to scope entry
      Break: identity(),     // exit scope with value
    }),
  ),
)
```

Note: `scope` takes a callback, not a name. The callback receives `restart` — a `TypedAction` whose AST carries an auto-generated unique ID (same pattern as `declare`'s VarRefs). The user never names the scope. The ID is invisible. Collisions are impossible for the same reason as `declare`: monotonic counter.

The scheduler would implement `restart` by re-entering the scope's body ActionId with a new value — exactly what Loop's Continue already does.

### Declare vs scope: orthogonal concepts

They capture different things:

| | `declare` | `scope` (hypothetical) |
|---|---|---|
| **Captures** | Values (data) | Program point (control flow) |
| **Mechanism** | VarRef → environment lookup | Jump → re-enter body ActionId |
| **Body executes** | Exactly once | Zero or more times |
| **Solves** | Prop drilling | Loop, early return, restart |

Both use the same pattern at the JS level: a callback that receives opaque AST references carrying auto-generated IDs. Both are structurally anonymous — no user-facing names in the AST. But `declare` stores values in a `HashMap<Id, Value>`, while `scope` would store re-entry points as `ActionId`s in the frame tree.

They share no implementation beyond the ID generation pattern. Don't unify them.

### Recommendation

Implement `declare` standalone. If `scope` is added later, it's a separate AST node with its own scheduler logic. The shared insight — "callback receives opaque references with auto-generated IDs, no user-facing names, no collisions" — is a pattern, not a shared abstraction.

## Scope rules

- **Lexical**: Variables are available within the `declare` body. Not visible outside.
- **No collision, ever**: Unique IDs from a monotonic counter. No shadowing, no resolution, no scope chain.
- **Flat environment**: Single `HashMap<DeclareId, Value>`. Nested `declare` adds entries; IDs never collide.
- **Shared across concurrency**: `forEach` iterations and `parallel` branches read the same environment. Variables are immutable — no copies needed.
- **Step boundaries are walls**: Step jumps (`stepRef`, `steps.X`) execute with an empty environment. Variables don't leak across step boundaries.
- **Handlers can't see variables**: The only way to get data into a handler is through its input. Variables are a workflow graph structuring mechanism, not a handler state channel.

## Resolved questions

1. **Body input type**: `TIn`. The body receives the same pipeline input as the declare block. Variables are supplementary — they don't replace the pipeline value.

2. **Interaction with steps**: Yes. A step body can contain `declare`. A binding can reference a named step. Nesting works fine.

3. **Implicit input binding**: No. If you want the pipeline input as a variable, bind it explicitly with `identity()`. No magic.
