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

At definition time, `declare` assigns each binding a **globally unique ID** via a monotonic counter: `__declare_0`, `__declare_1`, etc. Each single-binding `DeclareAction` node carries one ID and one binding AST. The `VarRef` nodes in the body carry these IDs. That's it.

```ts
// User writes (array form — sequential):
declare([
  { a: foo },
  ({ a }) => ({ b: bar_using_a }),
], ({ a, b }) => pipe(a, b))

// AST produced — nested single-binding Declares:
{
  kind: "Declare",
  id: "__declare_0",
  binding: /* foo's AST */,
  body: {
    kind: "Declare",
    id: "__declare_1",
    binding: /* bar_using_a's AST (may contain VarRef("__declare_0")) */,
    body: /* pipe(VarRef("__declare_0"), VarRef("__declare_1")) */,
  },
}
```

The object form (concurrent) compiles similarly but wraps bindings in a Parallel node first. See "Compilation of surface forms" below for details. The identity model is the same either way: every binding gets a unique ID, VarRefs carry those IDs.

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

The scheduler maintains an environment for variable bindings. When entering a single-binding `Declare` node:

1. Evaluate the binding (receives the pipeline input).
2. Extend the environment with `(id, value)`.
3. Execute the body (receives the same pipeline input, with the extended environment).

When encountering a `VarRef { id }`: look up `id` in the environment. Return the value. Done.

No name resolution. No shadowing. Just unique keys from the monotonic counter.

#### Environment data structure

The environment should be a **persistent immutable data structure** (cons list of `(id, value)` pairs, or a persistent hash map), not a flat mutable `HashMap`. Reasons:

- **Automatic scope cleanup**: When a Declare scope exits, the extended environment is simply no longer referenced. No explicit removal needed.
- **Parallel safety**: Parallel branches each receive the same environment reference. Immutable data requires no cloning or locking.
- **Future-proofing**: If the scheduler ever goes multi-threaded, persistent data structures work without changes.

Under the Handle/Perform model (see `ALGEBRAIC_EFFECTS.md`), the environment is even simpler: each Handle frame for ReadVar holds exactly one `(id, value)` pair. Lookup walks up the handler chain via parent pointers. The frame tree itself IS the cons list.

For the current cooperative single-threaded scheduler, a flat HashMap with explicit cleanup on scope exit would also work. The persistent structure is preferred for correctness, not performance.

## Surface API

Two forms: object (concurrent bindings) and array (sequential bindings).

### Object form — concurrent, independent bindings

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

All bindings evaluate concurrently. Each receives the pipeline input. Bindings cannot reference each other. The object literal signals independence — key order doesn't matter.

```ts
declare({
  branch: pipe(extractField("description"), deriveBranch),
  image: pipe(pick("repo", "sha"), buildImage),
}, ({ branch, image }) =>
  pipe(
    image.then(pick("imageTag")).then(deploy),
    branch.then(createPR),
  ),
)
```

### Array form — sequential, dependent bindings

```ts
function declare<TIn, TOut, TRefs extends string = never>(
  bindings: DeclareBinding<TIn>[],
  body: (vars: /* accumulated VarRefs */) => Pipeable<TIn, TOut, TRefs>,
): TypedAction<TIn, TOut, TRefs>
```

Each entry is a single-key object or a function that receives all previous bindings and returns a single-key object. Entries evaluate sequentially. Each can reference bindings from earlier entries.

```ts
declare([
  { image: pipe(pick("repo", "sha"), buildImage) },
  ({ image }) => ({
    deploy: pipe(
      image.then(pick("imageTag")),
      pick("environment"),
      merge(),
      deployToK8s,
    ),
  }),
], ({ image, deploy }) =>
  pipe(
    parallel(
      input.then(pick("repo", "environment")),
      deploy.then(pick("podName")),
    ).then(merge()).then(notifySlack),
  ),
)
```

The array form is do-notation. Each line is a monadic bind. The function parameter gives access to all bindings defined so far.

### Common to both forms

- **`body`**: Callback that receives VarRef AST nodes (one per binding) and returns the body AST. Each VarRef is `TypedAction<never, T>` — takes no pipeline input, produces the bound type. The body receives the original pipeline input as its pipeline value.
- **Return type**: `TypedAction<TIn, TOut>`. The declare block's input is the pipeline input; its output is the body's output.

### Compilation of surface forms

Both forms compile to nested single-binding `DeclareAction` nodes. The difference is whether a `Parallel` node wraps the concurrent bindings.

**Object form** `declare({ a: exprA, b: exprB }, body)`:

```
Parallel(exprA, exprB)
  → Declare(id_a, ExtractIndex(0, parallel_result),
      Declare(id_b, ExtractIndex(1, parallel_result),
        body))
```

The bindings are evaluated concurrently via Parallel (the existing AST node). The Parallel result is a tuple. Each nested Declare extracts its value from the tuple by index and binds it.

**Array form** `declare([{ a: exprA }, ({ a }) => ({ b: exprB_using_a })], body)`:

```
Declare(id_a, exprA,
  Declare(id_b, exprB_using_a,
    body))
```

Nested single-binding Declares. Sequential evaluation. Each binding's expression can contain VarRefs to earlier bindings.

### Relationship between declare and parallel

The object form of declare IS `parallel` with named results instead of a positional tuple. `parallel(a, b, c)` evaluates actions concurrently and returns `[OutputA, OutputB, OutputC]`. `declare({ x: a, y: b, z: c }, body)` evaluates actions concurrently and makes the results available as named VarRefs in the body.

`parallel` is the special case where you don't name the results and collect into a tuple. `declare` is the general case where results are named and scoped.

At the AST level, `Parallel` remains as a separate node — it's used constantly and desugaring every instance into Declare + VarRef + tuple construction would bloat the tree. But conceptually, Parallel is declare-without-names.

## Implementation: new AST nodes (Declare + VarRef)

### AST additions

The AST primitive is a **single-binding Declare**. One binding, one body.

```ts
// New Action variant — single binding
interface DeclareAction {
  kind: "Declare";
  id: string;        // unique ID from the monotonic counter
  binding: Action;   // the expression to evaluate
  body: Action;      // may contain VarRef nodes referencing this id
}

// New BuiltinKind variant
{ kind: "VarRef"; id: string }  // the unique ID from the counter
```

Multi-binding surface forms compile down to combinations of this primitive. See "Surface API" above for both forms and their compilation.

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

### Single-binding Declare: always eager

A single-binding `DeclareAction` evaluates its binding expression eagerly when the node is entered. The binding receives the pipeline input, runs to completion, and the result is inserted into the environment. Then the body executes (receiving the same pipeline input).

```
Enter Declare → evaluate binding → store (id, value) in env → execute body
```

This is call-by-value. One binding, no ambiguity about ordering or concurrency.

### Object form: concurrent via Parallel

The object form compiles to `Parallel(bindings) → nested Declares`. The Parallel node handles concurrent evaluation — all bindings run simultaneously, each receiving the pipeline input. After all complete, the nested Declare chain extracts each result and inserts it into the environment. Then the body runs.

Concurrency comes from Parallel, not from Declare. Declare is sequential by nature (it has one binding and one body). The object form gets concurrency by wrapping the bindings in Parallel before the Declare chain.

### Array form: sequential, dependent

The array form compiles directly to nested Declares with no Parallel wrapper. Each binding evaluates after the previous one completes. Each binding's expression can reference earlier bindings via VarRef. This is monadic bind — `let a = exprA in let b = exprB(a) in body(a, b)`.

### Lazy (deferred — not implementing now)

Lazy evaluation (thunks, forced on first VarRef access, memoized) is deferred. The array form gives us dependent bindings without laziness — binding B can reference binding A because A evaluates first. Laziness would add:

- Efficiency for unused bindings (minor — don't bind what you don't use)
- Unpredictable side-effect ordering (Haskell's IO monad problem)
- Concurrent memo tables for parallel branches referencing the same lazy var
- Unpredictable error locations (failure surfaces at VarRef, not Declare)

The costs outweigh the benefits. The array form handles the "binding B depends on binding A" case that was previously the strongest argument for laziness.

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

### RAII: separate concern from variable binding

**Important caveat**: Tying resource disposal to variable scope exit conflates two type-theoretic concepts. Variables are unrestricted (read zero or more times). Resources are affine (used at most once, must be disposed). See `ALGEBRAIC_EFFECTS.md` for the full analysis.

Under the current execution model (strict, sequential, cooperative, single-threaded), lexical scope perfectly matches temporal lifetime. Tying disposal to Declare scope exit is safe. It breaks if the execution model evolves to include lazy evaluation, detached forks, or continuation capture.

The surface API can still present a unified `declare` that handles both variables and resources. The compiler would generate two separate mechanisms: one for binding (ReadVar effect) and one for lifecycle management (Bracket effect). The user writes one thing; the concerns separate at the semantic level.

#### The model (surface API — unchanged)

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

The user writes:

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

#### Scheduler implementation: depends on architecture choice

Under the current proposal (specific frame kinds), disposal runs at Declare scope exit. The scheduler inspects the binding's handler metadata and calls dispose if present. This works today but assumes lexical scope = temporal lifetime.

Under the Handle/Perform model (see `ALGEBRAIC_EFFECTS.md`), the compiler generates two nested handlers:

```
Handle(Bracket, {          // resource lifecycle
  Handle(ReadVar, {        // variable binding
    body
  })
})
```

The Bracket handler tracks acquired resources and runs dispose on scope exit (success or error). The ReadVar handler manages variable lookup. The two are composed but independent: the ReadVar handler could be lazy without affecting the Bracket handler's disposal guarantees.

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

Nesting determines disposal order. Inner resources are disposed before outer resources. This holds under both the specific-frame-kind and Handle/Perform architectures.

#### Error semantics

**Body succeeds, dispose succeeds**: Normal case. Declare produces the body's output.

**Body fails, dispose succeeds**: Cleanup ran correctly. Declare propagates the body's error.

**Body succeeds, dispose fails**: Recommend strict — propagate dispose error. A leaked resource is an error.

**Body fails, dispose fails**: Propagate body's error (primary). Attach dispose error as suppressed/secondary.

#### This replaces withResource

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

## The unifying abstraction: algebraic effects

`declare`, RAII, `tryCatch`, `withTimeout`, `loop` are all instances of algebraic effect handling. See `ALGEBRAIC_EFFECTS.md` for the full theoretical treatment, including the Handle/Perform primitive pair and implementation options.

### The pattern

Every scope-creating combinator follows the same structure: install a handler, execute a body, handle effects from the body, clean up on exit. What varies is the effect type (variable lookup, error interception, timeout, loop control) and what the handler does with the continuation (resume, discard, re-enter).

| Combinator | Effect | Handler behavior |
|---|---|---|
| `declare` | `ReadVar(id)` | Look up value, resume with it |
| RAII | `Acquire`/`Release` | Track resource, dispose on scope exit |
| `tryCatch` | `Error(err)` | Discard continuation, run recovery |
| `withTimeout` | `Timeout` | Discard continuation, propagate |
| `loop` | `Continue(val)` / `Break(val)` | Re-enter body / exit scope |

### Two architecture options

**Specific frame kinds** (current trajectory): Each scope type is a dedicated AST node and frame kind. The scheduler has specific enter/exit logic for each.

**Handle/Perform** (general mechanism): One AST node pair in the flat table. The scheduler implements one general frame kind. Specific scope types are TypeScript sugar that compiles down. See `ALGEBRAIC_EFFECTS.md` for details.

Both approaches produce the same user-facing API. The difference is in the scheduler's internal architecture. The Handle/Perform approach is more work upfront but pays off when adding the third or fourth scope type.

### Implementation considerations

`declare` exercises scope infrastructure that doesn't exist yet in the scheduler: environment management, scope-exit cleanup, error-path scope exit. These are real additions.

Whether to implement this as a specific Declare frame kind or as Handle/Perform is an architectural choice (see `ALGEBRAIC_EFFECTS.md`). Either way, the single-binding Declare AST node in the tree is the same. The question is what the Rust scheduler sees after flattening.

The single-binding Declare AST node is minimal — one id, one binding, one body. If the evaluation strategy or scheduler architecture changes, the surface API and tree AST remain stable. The flattening step absorbs the differences.

### Could Loop be expressed as a scope?

Yes. Loop is a hardcoded continuation — it captures a body ActionId and re-enters it on Continue. A general scope primitive could subsume it:

```ts
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

`scope` takes a callback. The callback receives `restart` — a `TypedAction` whose AST carries an auto-generated unique ID (same pattern as declare's VarRefs). The user never names the scope. Collisions are impossible: monotonic counter.

The scheduler implements `restart` by re-entering the scope's body ActionId with a new value — exactly what Loop's Continue already does.

But Loop should remain as a dedicated AST node even if we add `scope`. Loop has specific error messages ("loop body must return Continue/Break"), type-level enforcement of the Continue/Break protocol, and clear intent. `scope` would be the general escape hatch; Loop would be the ergonomic specialization. Same pattern as how languages keep `for` loops even though every `for` is expressible as a `while`.

### Declare captures data, scope captures control flow

They're both scoped effects. They both use the callback-receives-opaque-reference pattern with auto-generated IDs. But they capture fundamentally different things:

| | `declare` | `scope` (hypothetical) |
|---|---|---|
| **Captures** | Values (data) | Program point (control flow) |
| **Mechanism** | VarRef → environment lookup | Jump → re-enter body ActionId |
| **Body executes** | Exactly once | Zero or more times |
| **Solves** | Prop drilling | Loop, early return, restart |

They compose rather than conflict. A declare inside a scope gives you both data bindings and control flow. A scope inside a declare gives you looping with access to outer variables.

### Implementation order

Each step builds on the previous:

1. **`declare`** — the foundational scope type. Builds environment management, scope-exit protocol, error-path cleanup. All subsequent scope types reuse this infrastructure.
2. **Handler-level dispose (RAII)** — not a new scope type; adds exit behavior to existing Declare frames. Tests the scope-exit protocol with real cleanup logic.
3. **`tryCatch`** — new scope type for error handling. First scope type beyond declare, validates that the frame infrastructure generalizes.
4. **`withTimeout`** — new scope type with external triggers (timer). Tests the cancellation model.
5. **`scope`/continuations** — general control flow. Subsumes loop's re-entry pattern. Only if we need it.

Each is incremental. Each validates the infrastructure built by the previous step.

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
