# Declare Bindings

## Motivation

Barnum pipelines thread data through each step as a single value. When a step produces a result that later steps need, the intervening steps must accept and pass through fields they don't use — classic prop drilling. The current workarounds (`tap`, `augment`, `withResource`) keep the data flowing but add ceremony and obscure intent.

A `declare` combinator would introduce named variable bindings: evaluate a set of expressions, bind the results to names, and make those names available as AST references inside a callback. The callback constructs the body of the `declare` block using those references.

This is the AST-level equivalent of JavaScript's `const { a, b } = ...`:

```ts
// JavaScript:
const { branch, worktreePath } = await createWorktree(input);
await implement({ worktreePath, description: input.description });

// Barnum today — manual threading via augment + tap + pick:
pipe(
  deriveBranch,
  createWorktree,
  tap(pipe(pick("worktreePath", "description"), implement)),
  tap(pipe(pick("worktreePath"), commit)),
)

// Barnum with declare bindings:
declare({
  worktree: pipe(deriveBranch, createWorktree),
}, ({ worktree }) =>
  pipe(
    worktree.then(pick("worktreePath", "description")).then(implement).dropOutput(),
    worktree.then(pick("worktreePath")).then(commit),
  ),
)
```

The key property: `worktree` in the callback is not the evaluated value — it's an AST node that, when placed in a pipeline, evaluates to the bound value. This keeps everything in AST-land. The callback runs at definition time (in JavaScript), producing an AST that the Rust scheduler later executes.

## Design

### Surface API

```ts
function declare<
  TIn,
  TBindings extends Record<string, TypedAction<TIn, unknown>>,
  TOut,
>(
  bindings: TBindings,
  body: (vars: {
    [K in keyof TBindings]: TypedAction<never, ExtractOutput<TBindings[K]>>
  }) => TypedAction<???, TOut>,
): TypedAction<TIn, TOut>
```

- `bindings`: An object mapping variable names to ASTs. Each AST receives the pipeline input (`TIn`) and produces its bound value.
- `body`: A JavaScript callback that receives named AST references and returns the body AST. The callback runs at definition time — it's a metaprogramming construct, not a runtime function.
- Each variable reference is a `TypedAction<never, T>` — it takes no pipeline input (the value is already captured), and produces the bound type.

### Example: avoiding prop drilling

```ts
declare({
  branch: pipe(extractField("description"), deriveBranch),
}, ({ branch }) =>
  pipe(
    implement,       // doesn't need to pass branch through
    commit,          // doesn't need branch either
    // only the step that needs branch references it
    branch.then(createPR),
  ),
)
```

## Implementation approaches

Two approaches, each with different tradeoffs. **Approach A is the clear winner** — Approach B is documented here to record why it was rejected.

### Approach A: New AST nodes (Declare + VarRef) — chosen

A new `Declare` action node and a `VarRef` builtin:

```ts
// New Action variant
| DeclareAction

interface DeclareAction {
  kind: "Declare";
  bindings: Record<string, Action>;  // each binding is an AST
  body: Action;                       // may contain VarRef nodes
}

// New BuiltinKind variant
| { kind: "VarRef"; id: string }  // unique ID, not a name
```

`VarRef` resolves to the bound value at runtime. The scheduler maintains a flat `Map<String, Value>` environment alongside each execution frame. Every binding gets a unique ID generated at definition time in JavaScript (e.g., a counter or UUID), so there is no shadowing — just unique keys.

Variables are an **environment**, not pipeline data. Pipeline data flows linearly through each step. Variables need to be accessible from any point in the body, regardless of what the current pipeline value is. `VarRef` reaches into a side channel (the environment) to get its value — it ignores the pipeline input entirely.

**Pros**: The scheduler has full visibility into variable lifetimes. It can optimize (evaluate once, cache), enforce scoping, and integrate with error handling / RAII. The AST is self-contained — the Rust scheduler can execute it without calling back into JavaScript. Error messages reference user-written variable names. Future features (RAII, lazy evaluation, dependent bindings) are localized scheduler changes.

**Cons**: New AST node, new Rust scheduler concept (variable environment). Every consumer of the AST (flattener, serializer, Rust engine) needs to handle `Declare` and `VarRef`.

### Approach B: JS-side AST rewriting (no new AST nodes) — rejected

Instead of adding `Declare` and `VarRef` to the AST, `declare` could work entirely in JavaScript by rewriting the body AST to thread the variable environment through every step as an explicit pipeline value.

#### The technique: closure conversion

This is a well-known compiler technique called **closure conversion** (or closure elimination). When a language has variables that are accessible across lexical scope — reachable from arbitrary positions in the AST, not just from the immediately downstream step — and the target runtime only supports linear data flow, the compiler eliminates the free variables by threading them as an explicit environment parameter.

Concretely: every step `f: A → B` in the body gets wrapped in `parallel(pipe(extractIndex(0), f), extractIndex(1))`, transforming it to `[A, Env] → [B, Env]`. The environment tuple is carried alongside the pipeline value through every step. VarRef sentinels are replaced with the appropriate `extractIndex` into the environment.

```ts
function declare(bindings, body) {
  const keys = Object.keys(bindings);
  const bindingActions = Object.values(bindings);

  // Create sentinel placeholders for the callback
  const vars = {};
  const sentinels = new Map();
  keys.forEach((key, i) => {
    const sentinel = typedAction({
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Identity" } },
    });
    Object.defineProperty(sentinel, "__varRef", { value: { name: key, index: i } });
    vars[key] = sentinel;
    sentinels.set(sentinel, i);
  });

  // Body callback runs, producing an AST with sentinel nodes
  const rawBody = body(vars);

  // Closure-convert: rewrite entire body to thread env through every node
  const rewrittenBody = closureConvert(rawBody, sentinels);

  return pipe(
    parallel(...bindingActions),  // evaluate bindings → env tuple
    rewrittenBody,                // body with env threaded through
  );
}
```

#### AST size is not the problem

The raw AST blowup is a constant factor — roughly 3x (a 5-step body becomes ~15 nodes). This is linear, not exponential, and manageable in isolation.

#### The real problems

**1. Every AST node type needs a transform case, and several are subtle.**

Flat pipes are trivial. The hard cases:

- **Loop**: The body produces `Continue(nextVal)` / `Break(result)` signals. The transform must restructure these to carry env: `Continue([nextVal, env])` / `Break([result, env])`, then strip env from the Break result after the loop exits. The loop machinery needs to understand that the env portion of Continue is threaded through, not accumulated.

- **ForEach**: Input is `[items[], env]`. You can't forEach over the tuple — you need to extract items, map each to `[item, env]`, run the transformed body on each pair, then extract just the results. Requires non-trivial restructuring around the forEach.

- **Parallel**: Each branch receives `[pipeVal, env]` and produces `[branchOut, env]`. Output is `[[out1, env], [out2, env], ...]` but you want `[[out1, out2, ...], env]`. Requires a restructuring step after the parallel to transpose the result.

- **Branch**: Match predicates need the pipeline value, not the `[pipeVal, env]` tuple. Extract before matching, re-pair with env for the action arm.

None are unsolvable, but each node type needs its own correct transform case. Every future AST node type also needs one.

**2. Feature interactions compound quadratically.**

Every feature desugared into existing nodes must account for every other desugared feature. If feature B's desugaring encounters feature A's desugared output, B's transform must handle A's synthetic nodes correctly. With N desugared features, there are O(N²) interaction cases. Native AST nodes avoid this entirely — each feature is a self-contained scheduler concern.

**3. Scheduler opacity destroys debuggability.**

The scheduler sees an explosion of `parallel`/`extractIndex` nodes with no semantic meaning. Error messages reference synthetic nodes ("parallel at position 3 failed") instead of user-written variable names ("binding `worktree` failed"). Visualization tools show the mechanical expansion. There's no way to reconstruct "this parallel is just env-threading, not real concurrency" from the desugared AST.

**4. Future features require re-doing the desugaring, not extending the scheduler.**

With native AST nodes, adding RAII cleanup to `declare` is a localized scheduler change: "when a Declare scope exits, run dispose on bindings that have disposers." With closure conversion, you'd need to restructure the entire desugaring to wrap the body in try/finally-equivalent nodes, threading dispose actions alongside the env. The `withResource` implementation (`builtins.ts:213-244`) is already 30 lines of careful `parallel`/`chain`/`extractIndex` assembly and still doesn't handle cleanup-on-failure — a preview of where this approach leads.

**5. TypeScript type inference degrades.**

Every intermediate value becomes a `[PipelineValue, Env]` tuple. The type-level transform mirrors the AST transform. TypeScript's inference is less reliable with deeply nested conditional tuple types, and error messages become inscrutable tuple-index complaints instead of named-variable complaints.

#### When closure conversion is the right choice

Closure conversion is standard practice when the target runtime is fixed (CPU instruction set, VM bytecode, WASM). You can't add new concepts to a CPU. But the Barnum scheduler is ours to extend. Adding a node is a one-time cost. Maintaining a closure conversion pass over a growing AST grammar is an ongoing tax on every future feature.

**Verdict**: Approach B is viable as a proof-of-concept prototype. For production, Approach A is unambiguously better.

## Scheduler changes (Approach A)

### Environment model

The scheduler maintains a `Map<String, Value>` environment alongside the frame tree. When a `Declare` node is entered:

1. Evaluate all bindings (each receives the pipeline input, produces a value).
2. Push a new scope onto the environment with the bound names.
3. Execute the body with this extended environment.
4. Pop the scope on exit.

When a `VarRef` is encountered, look up the name in the current environment. This is O(n) in scope depth, but scopes are shallow in practice (typically 1-3 levels).

### Scope rules

- **Lexical scoping**: Variables are scoped to the `declare` body. Not visible outside.
- **Unique IDs, no shadowing**: Every binding has a unique ID (generated at definition time in JS). There is no name-based shadowing — each binding is globally unique within the config. The scheduler environment is a flat map from ID to value.
- **Shared environment**: `forEach` iterations and `parallel` branches all share the same environment (read-only, since variables are immutable). No copying needed. A VarRef in any branch or iteration resolves to the same cached value.
- **Step boundaries**: The environment does NOT leak across step jumps. When execution transfers to a named step (via `stepRef` or `steps.X`), the step runs with an empty environment. Variables are a structuring mechanism within a step's body, not a cross-step data channel.
- **No mutation**: Variables are bound once and never reassigned. The environment is append-only within a scope.

### Implicit scoping via handler boundaries

Handlers (TypeScript subprocesses) can't access declared variables. The only way to get data into a handler is through its input. Variables are a structuring mechanism for the workflow graph, not a way to smuggle state into handlers. Handlers remain pure functions of their declared input.

## Evaluation strategy

### Eager (current design)

All bindings evaluate when the `Declare` node is entered. Each binding receives the pipeline input and runs to completion. Results are stored in the environment. If a variable is never referenced, its binding still executes.

```
Enter Declare → evaluate all bindings → store results → execute body
```

This is call-by-value. Simple, predictable, matches most languages. Side effects (handler invocations) happen in a known order: bindings first, body second.

### Lazy (speculative)

Bindings are not evaluated when `Declare` is entered. Instead, the scheduler records the binding ASTs in the environment as **thunks** — unevaluated computations. A `VarRef` forces its thunk on first access; the result is memoized for subsequent references. If a variable is never referenced, its binding never executes.

```
Enter Declare → record thunks → execute body → force thunks on VarRef access
```

This is call-by-need (Haskell's evaluation strategy).

#### What lazy gets you

1. **Efficiency**: Don't compute what you don't use. If a binding is expensive and only needed in one branch of a `branch`, it's wasteful to evaluate it eagerly when the other branch is taken.

2. **Dependent bindings**: Bindings could reference other bindings. `b` could depend on `a`'s result if `a` is forced first. This enables:

```ts
declare({
  branch: deriveBranch,
  worktree: ({ branch }) => branch.then(createWorktree),  // depends on branch
}, ({ worktree }) =>
  worktree.then(implement),
)
```

With eager evaluation, all bindings see the same pipeline input. With lazy, bindings form a dependency graph.

3. **Natural resource management**: An unreferenced binding that creates a resource never creates it — no cleanup needed. Combined with RAII (if we had it), this could replace `withResource` entirely.

#### What lazy costs you

1. **Side effect ordering**: Handler invocations have side effects (file I/O, network, subprocess spawning). Lazy evaluation means the order of side effects depends on which variables are referenced first in the body. This is the exact problem Haskell solves with the IO monad — and the exact problem we'd be introducing.

2. **Concurrency semantics**: If two parallel branches both reference the same lazy variable, the scheduler needs synchronization: evaluate the thunk exactly once, block the second reference until the first completes. This is a concurrent memo table — not trivial.

3. **Error semantics**: If a thunk fails, where does the error surface? At the `VarRef` site, not the `Declare` site. This means error locations become less predictable. The same binding could fail at different points depending on which reference is evaluated first.

4. **Debugging**: Lazy evaluation makes execution traces harder to follow. The order of operations in the trace doesn't match the order of declarations in the source.

#### Are we reinventing Haskell?

Arguably yes. Consider what Barnum already has:

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
| Type classes | Handler schemas (structural) |

Adding lazy bindings with memoization makes the parallel even tighter. Barnum would essentially be a lazy, first-order, effectful workflow DSL — which is a restricted Haskell with explicit effects.

The question is whether this is a good thing or a bad thing. Haskell's laziness is its most controversial feature. The benefits (composability, modularity) are real but so are the costs (space leaks, unpredictable performance, reasoning difficulty).

**Recommendation**: Start with eager evaluation. It's simpler, predictable, and covers the common case (bind a few values, use them all). If we find ourselves writing bindings that are conditionally needed, we can add a `lazy_declare` variant or a per-binding `lazy: true` flag. Don't default to lazy.

The deeper insight: Barnum is already a language runtime (see BARNUM_AS_LANGUAGE.md). Every feature we add makes it more of a language. `declare` bindings are variables; lazy `declare` is call-by-need; RAII is linear types. We should be deliberate about which language features we adopt and recognize that each one moves us further along the "accidentally designed a programming language" spectrum.

## Relation to existing combinators

If `declare` exists, several patterns simplify:

| Current pattern | With `declare` |
|---|---|
| `augment(pipe(extract, transform))` | `declare({ x: pipe(extract, transform) }, ({ x }) => ...)` |
| `tap(sideEffect)` | Side effect in a binding that's never referenced (eager) |
| `withResource({ create, action, dispose })` | See "Interaction with RAII" below |

### Interaction with RAII

`declare` doesn't replace `withResource` because it has no cleanup semantics. A binding that creates a worktree has no way to delete it when the body exits. You'd need:

```ts
declare({
  resource: createWorktree,
}, ({ resource }) =>
  pipe(
    resource.then(implement),
    resource.then(commit),
    resource.then(deleteWorktree),  // manual cleanup, not RAII
  ),
)
```

This is fragile — if `implement` fails, `deleteWorktree` never runs. `withResource` guarantees cleanup. `declare` + RAII (see RAII.md if it exists) could replace `withResource`, but `declare` alone cannot.

## Open questions

1. **Should the pipeline input be implicitly available as a variable?** In the `withResource` example, the original input is just as important as the resource. Should `declare` implicitly bind the input under a special name (e.g., `_` or `input`)? Or is `identity()` in the bindings sufficient?

2. **Interaction with steps**: Can a binding reference a named step? Can a step body contain `declare`? There's no fundamental obstacle, but the interaction with step registration and mutual recursion needs thought.

3. **Multiple references to the same variable**: A VarRef can appear multiple times in the body AST. The scheduler evaluates the binding once and caches the value. Multiple references return the same cached result.

4. **Body input type**: What is the body's pipeline input? Options:
   - `never` — the body has no pipeline input, only variable references. Forces all data through variables.
   - `TIn` — the body receives the same input as the declare block. Variables are supplementary, not replacement.
   - The second option is more flexible and avoids forcing the user to bind the input explicitly.
