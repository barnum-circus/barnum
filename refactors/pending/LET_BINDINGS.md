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

With `declare`:

```ts
declare({
  image: pipe(pick("repo", "sha"), buildImage),
}, ({ image }) =>
  pipe(
    // runTests: just needs imageTag
    image.then(pipe(pick("imageTag"), runTests)).dropOutput(),

    // deployToK8s: needs imageTag + environment from pipeline input
    parallel(image.then(pick("imageTag")), pick("environment"))
      .then(merge())
      .then(deployToK8s),

    // TODO: this is awkward. Having to merge image output with pipeline
    // input manually is not much better than augment. Consider whether
    // declare should support multiple references more ergonomically.
  ),
)
```

Actually, a more natural use of `declare` — bind the derived values individually:

```ts
declare({
  image: pipe(pick("repo", "sha"), buildImage),
  input: identity(),  // capture the pipeline input itself as a variable
}, ({ image, input }) =>
  pipe(
    // runTests: just needs imageTag from image
    image.then(pick("imageTag")).then(runTests).dropOutput(),

    // deployToK8s: needs imageTag from image + environment from input
    parallel(
      image.then(pick("imageTag")),
      input.then(pick("environment")),
    ).then(merge()).then(deployToK8s),

    // notifySlack: needs repo+environment from input + podName from deploy
    // ... but we don't have the deploy result as a variable.
  ),
)
```

This reveals a design tension: `declare` binds values computed **before** the body runs. Values produced **during** the body (like `podName` from `deployToK8s`) need either nested `declare` or pipeline threading. The ergonomic sweet spot is binding things derived from the initial input — configuration, environment, derived identifiers — and letting the body pipeline handle step-to-step data flow.

The clearest win for `declare`:

```ts
// Bind all the "contextual" values once, reference them throughout.
declare({
  input: identity<DeployInput>(),
  image: pipe(pick("repo", "sha"), buildImage),
}, ({ input, image }) =>
  pipe(
    image.then(pick("imageTag")).then(runTests).dropOutput(),
    image.then(pick("imageTag"))
      .then(augment(input.then(pick("environment"))))
      .then(deployToK8s),
    // deployToK8s output: { podName }
    augment(input.then(pick("repo", "sha")))
      .then(augment(input.then(pick("environment"))))
      .then(notifySlack)
      .dropOutput(),
    augment(input.then(pick("repo", "sha")))
      .then(updateDashboard),
  ),
)
```

The key improvement: `input` and `image` are available at any point without threading. Steps that need `repo` grab it from `input`; steps that need `imageTag` grab it from `image`. No step is forced to accept fields it doesn't use just to pass them downstream.

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

### What RAII would mean for declare

`declare` as specified has no cleanup semantics. A binding that creates a git worktree has no way to delete it when the body exits. If the body fails, the worktree leaks.

```ts
// Without RAII — cleanup is manual and fragile
declare({
  worktree: createWorktree,
}, ({ worktree }) =>
  pipe(
    worktree.then(implement),
    worktree.then(commit),
    worktree.then(deleteWorktree),  // never runs if implement fails
  ),
)
```

RAII would add an optional `dispose` action to each binding. The scheduler guarantees: if a binding was successfully evaluated, its dispose action runs when the `declare` scope exits — regardless of whether the body succeeded or failed.

#### Surface API with RAII

The bindings object would accept either a bare action (no cleanup) or a `{ create, dispose }` pair:

```ts
declare({
  // Simple binding — no cleanup
  branch: deriveBranch,
  // Resource binding — with cleanup
  worktree: {
    create: createWorktree,
    dispose: deleteWorktree,
  },
}, ({ branch, worktree }) =>
  pipe(
    worktree.then(pick("worktreePath", "description")).then(implement),
    worktree.then(pick("worktreePath")).then(commit),
    branch.then(createPR),
  ),
)
```

The type system would distinguish these: a bare `Pipeable<TIn, T>` produces a `TypedAction<never, T>` VarRef. A `{ create: Pipeable<TIn, T>, dispose: Pipeable<T, unknown> }` also produces a `TypedAction<never, T>` VarRef — same from the body's perspective. The dispose action is invisible to the body callback. It's metadata for the scheduler.

#### Scheduler behavior

When a `Declare` node with disposable bindings is entered:

1. Evaluate all bindings in parallel (same as today).
2. Store results in the environment.
3. Execute the body.
4. **On body completion (success or failure)**: run dispose actions for all bindings that have them and were successfully evaluated.

The critical guarantee: dispose runs **even if the body fails**. This is the entire point of RAII — cleanup is not optional, not manual, not dependent on the happy path.

#### Error semantics

This is where it gets interesting. There are several failure modes:

**Body succeeds, dispose succeeds**: Normal case. Declare produces the body's output.

**Body succeeds, dispose fails**: The body's work completed, but cleanup failed. What should the declare node produce? Options:
- Return the body's output and log/ignore the dispose failure (pragmatic — the work is done)
- Fail the declare node with the dispose error (strict — resource leak is an error)
- Return both: `{ result: bodyOutput, disposeErrors: [...] }` (changes the output type — ugly)

**Body fails, dispose succeeds**: Cleanup ran correctly after a failure. The declare node propagates the body's error.

**Body fails, dispose fails**: Both failed. The declare node must propagate the body's error (that's the primary failure). The dispose error is secondary — log it, attach it as a suppressed error, or drop it.

**Binding fails**: If a binding itself fails during evaluation, the other bindings that completed need their dispose actions run. This means dispose must be possible even when not all bindings were evaluated — the scheduler needs to track which bindings completed successfully.

#### Dispose ordering

When multiple bindings have dispose actions, what order do they run?

**Reverse order of completion** (stack discipline): The last binding to complete is the first to be disposed. This matches C++ RAII semantics and is correct when resources have dependencies — a resource created later might depend on one created earlier.

**Parallel**: Run all dispose actions concurrently. Faster, but incorrect if resources depend on each other (e.g., a file handle in a worktree — must close file before deleting worktree).

**Recommendation**: Parallel by default (bindings are independent unless proven otherwise — they were evaluated in parallel, after all). If ordering matters, nest the declares:

```ts
// Outer disposes after inner
declare({
  worktree: { create: createWorktree, dispose: deleteWorktree },
}, ({ worktree }) =>
  declare({
    file: { create: worktree.then(openFile), dispose: closeFile },
  }, ({ file }) =>
    file.then(write),
  ),
)
```

The inner `declare`'s dispose (closeFile) runs when its body completes. The outer `declare`'s dispose (deleteWorktree) runs after the inner `declare` — including its dispose — completes. Nesting gives you ordering control without special syntax.

#### AST representation

Two options for encoding dispose in the AST:

**Option 1: Inline in DeclareAction.** Each binding in the `bindings` map becomes `{ create: Action, dispose?: Action }` instead of a bare `Action`.

```ts
interface DeclareAction {
  kind: "Declare";
  bindings: Record<string, DeclareBinding>;
  body: Action;
}

type DeclareBinding =
  | { kind: "Simple"; action: Action }
  | { kind: "Resource"; create: Action; dispose: Action };
```

**Option 2: Separate dispose map.** Keep `bindings` as `Record<string, Action>` and add a `disposers: Record<string, Action>` map (same keys as bindings, only for bindings that have cleanup).

```ts
interface DeclareAction {
  kind: "Declare";
  bindings: Record<string, Action>;
  disposers: Record<string, Action>;  // subset of binding IDs
  body: Action;
}
```

Option 2 is simpler for the flattener and engine — the bindings map stays uniform. The disposers map is a parallel structure the scheduler consults during scope exit. Option 1 is more self-describing. Either works; option 2 is probably easier to implement.

#### This replaces withResource

`declare` with RAII subsumes `withResource` entirely:

```ts
// withResource today:
withResource({
  create: createWorktree,
  action: pipe(implement, commit),
  dispose: deleteWorktree,
})

// declare with RAII:
declare({
  worktree: { create: createWorktree, dispose: deleteWorktree },
}, ({ worktree }) =>
  pipe(
    worktree.then(implement),
    worktree.then(commit),
  ),
)
```

The `declare` version is strictly more powerful:
- Multiple resources in one scope (withResource handles one)
- Resources can be referenced from any point in the body (withResource merges into pipeline input)
- Other bindings can coexist alongside resources (withResource is resources only)
- Nested declares give dispose ordering control

Once `declare` has RAII, `withResource` becomes a convenience wrapper — or is deleted entirely.

#### Implementation order

RAII is not part of the initial `declare` implementation. The plan:

1. Ship `declare` with simple bindings (no dispose). This solves prop drilling.
2. Add RAII to `declare` as a follow-up. This replaces `withResource`.
3. Deprecate `withResource`.

Step 1 is valuable on its own. Step 2 is valuable on its own. Neither blocks the other. The `DeclareAction` AST node is designed so that adding `disposers` later is a backward-compatible extension (add a new field with a default of empty map).

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

## Open questions

1. **Body input type**: Should the body's pipeline input be `TIn` (same as declare's input — variables are supplementary) or `never` (all data must come through variables)? `TIn` is more flexible.

2. **Interaction with steps**: Can a step body contain `declare`? Can a binding reference a named step? No fundamental obstacle, but the interaction with step registration needs thought.

3. **Implicit input binding**: Should `declare` automatically bind the pipeline input as a variable? Or is `identity()` in the bindings sufficient for cases where you need it?
