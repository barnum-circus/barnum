# Let Bindings

## Motivation

Barnum pipelines thread data through each step as a single value. When a step produces a result that later steps need, the intervening steps must accept and pass through fields they don't use — classic prop drilling. The current workarounds (`tap`, `augment`, `withResource`) keep the data flowing but add ceremony and obscure intent.

A `let` combinator would introduce named variable bindings: evaluate a set of expressions, bind the results to names, and make those names available as AST references inside a callback. The callback constructs the body of the `let` block using those references.

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

// Barnum with let bindings:
let_({
  worktree: pipe(deriveBranch, createWorktree),
}, ({ worktree }) =>
  pipe(
    worktree.then(pick("worktreePath", "description")).then(implement).dropOutput(),
    worktree.then(pick("worktreePath")).then(commit),
  ),
)
```

The key property: `resource` in the callback is not the evaluated value — it's an AST node that, when placed in a pipeline, evaluates to the bound value. This keeps everything in AST-land. The callback runs at definition time (in JavaScript), producing an AST that the Rust scheduler later executes.

## Design

### Surface API

```ts
function let_<
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
let_({
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

Two approaches, each with different tradeoffs.

### Approach A: New AST nodes (Let + VarRef)

A new `Let` action node and a `VarRef` builtin:

```ts
// New Action variant
| LetAction

interface LetAction {
  kind: "Let";
  bindings: Record<string, Action>;  // each binding is an AST
  body: Action;                       // may contain VarRef nodes
}

// New BuiltinKind variant
| { kind: "VarRef"; id: string }  // unique ID, not a name
```

`VarRef` resolves to the bound value at runtime. The scheduler maintains a flat `Map<String, Value>` environment alongside each execution frame. Every binding gets a unique ID generated at definition time in JavaScript (e.g., a counter or UUID), so there is no shadowing — just unique keys.

Variables are an **environment**, not pipeline data. Pipeline data flows linearly through each step. Variables need to be accessible from any point in the body, regardless of what the current pipeline value is. `VarRef` reaches into a side channel (the environment) to get its value — it ignores the pipeline input entirely.

**Pros**: The scheduler has full visibility into variable lifetimes. It can optimize (evaluate once, cache), enforce scoping, and integrate with error handling / RAII. The AST is self-contained — the Rust scheduler can execute it without calling back into JavaScript.

**Cons**: New AST node, new Rust scheduler concept (variable environment). Every consumer of the AST (flattener, serializer, Rust engine) needs to handle `Let` and `VarRef`.

### Approach B: JS-side AST rewriting (no new AST nodes)

Instead of adding `Let` and `VarRef` to the AST, `let_` could work entirely in JavaScript by traversing and modifying the AST that the body callback produces.

The idea: the body callback produces an AST containing placeholder nodes (VarRef-like sentinels). Before serializing, `let_` walks the AST tree in JavaScript and replaces each placeholder with the appropriate `extractIndex(i)` from a wrapping `Parallel` node.

```ts
function let_(bindings, body) {
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
    // Mark this node so we can find it during traversal
    Object.defineProperty(sentinel, "__varRef", { value: { name: key, index: i } });
    vars[key] = sentinel;
    sentinels.set(sentinel, i);
  });

  // Body callback runs, producing an AST with sentinel nodes
  const rawBody = body(vars);

  // Walk the AST, replacing sentinels with the real extraction logic:
  // Each sentinel becomes: parallel(extractIndex(i_from_outer_tuple), identity())
  // where the outer tuple is the Parallel node wrapping all bindings.
  //
  // Actually — this is where it gets complicated. The sentinel needs to
  // "reach back" to the parallel result, but the pipeline value at the
  // sentinel's position is whatever the upstream step produced, not the
  // parallel tuple. Same fundamental problem as inline substitution.
  const rewrittenBody = rewriteAst(rawBody, sentinels);

  return pipe(
    parallel(...bindingActions),
    rewrittenBody,
  );
}
```

The problem is the same one that killed Options B and C from the earlier version of this doc: once the pipeline value moves past the parallel tuple, variable references can't reach back to extract from it. AST rewriting in JS-land doesn't change the runtime execution model — the Rust scheduler still only sees linear pipeline data flow.

**However**, JS-side rewriting could work if the rewrite is more aggressive. Instead of simple sentinel replacement, `let_` could restructure the entire body AST to thread the variable tuple through every step. Concretely: every step in the body gets wrapped in a `parallel(step, identity())` node that preserves the variable tuple alongside the pipeline value, and every VarRef sentinel is replaced with the appropriate `extractIndex`.

This is essentially CPS-transforming the body to carry the variable environment as an extra pipeline value. It's doable but produces an explosively larger AST (every step gets wrapped in parallel+identity).

**Pros**: No changes to the Rust scheduler or AST definition. The entire feature lives in TypeScript. The serialized AST uses only existing node types.

**Cons**:
- AST explosion: every step in the body gets wrapped in parallel+identity to carry the environment. A 5-step body becomes ~15 nodes.
- Fragile: the rewrite must handle every AST node type (Chain, Parallel, Branch, Loop, ForEach, Step). Missing a case is a bug.
- Opaque: the Rust scheduler sees an explosion of parallel+extractIndex nodes with no semantic meaning. Debugging, logging, and optimization are harder.
- Types: the TypeScript types become extremely complex because every intermediate step has a tuple type `[PipelineValue, VariableEnvironment]`.

**When it makes sense**: If we want to prototype `let` quickly without touching Rust, this works as a proof of concept. But for production, Approach A (scheduler-native) is cleaner.

## Scheduler changes (Approach A)

### Environment model

The scheduler maintains a `Map<String, Value>` environment alongside the frame tree. When a `Let` node is entered:

1. Evaluate all bindings (each receives the pipeline input, produces a value).
2. Push a new scope onto the environment with the bound names.
3. Execute the body with this extended environment.
4. Pop the scope on exit.

When a `VarRef` is encountered, look up the name in the current environment. This is O(n) in scope depth, but scopes are shallow in practice (typically 1-3 levels).

### Scope rules

- **Lexical scoping**: Variables are scoped to the `let` body. Not visible outside.
- **Unique IDs, no shadowing**: Every let binding has a unique ID (generated at definition time in JS). There is no name-based shadowing — each binding is globally unique within the config. The scheduler environment is a flat map from ID to value.
- **Shared environment**: `forEach` iterations and `parallel` branches all share the same environment (read-only, since variables are immutable). No copying needed. A VarRef in any branch or iteration resolves to the same cached value.
- **Step boundaries**: The environment does NOT leak across step jumps. When execution transfers to a named step (via `stepRef` or `steps.X`), the step runs with an empty environment. Variables are a structuring mechanism within a step's body, not a cross-step data channel.
- **No mutation**: Variables are bound once and never reassigned. This is `let`, not `var`. The environment is append-only within a scope.

### Implicit scoping via handler boundaries

Handlers (TypeScript subprocesses) can't access let-bound variables. The only way to get data into a handler is through its input. Variables are a structuring mechanism for the workflow graph, not a way to smuggle state into handlers. Handlers remain pure functions of their declared input.

## Evaluation strategy

### Eager (current design)

All bindings evaluate when the `Let` node is entered. Each binding receives the pipeline input and runs to completion. Results are stored in the environment. If a variable is never referenced, its binding still executes.

```
Enter Let → evaluate all bindings → store results → execute body
```

This is call-by-value. Simple, predictable, matches most languages. Side effects (handler invocations) happen in a known order: bindings first, body second.

### Lazy (speculative)

Bindings are not evaluated when `Let` is entered. Instead, the scheduler records the binding ASTs in the environment as **thunks** — unevaluated computations. A `VarRef` forces its thunk on first access; the result is memoized for subsequent references. If a variable is never referenced, its binding never executes.

```
Enter Let → record thunks → execute body → force thunks on VarRef access
```

This is call-by-need (Haskell's evaluation strategy).

#### What lazy gets you

1. **Efficiency**: Don't compute what you don't use. If a binding is expensive and only needed in one branch of a `branch`, it's wasteful to evaluate it eagerly when the other branch is taken.

2. **Dependent bindings**: Bindings could reference other bindings. `b` could depend on `a`'s result if `a` is forced first. This enables:

```ts
let_({
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

3. **Error semantics**: If a thunk fails, where does the error surface? At the `VarRef` site, not the `Let` site. This means error locations become less predictable. The same binding could fail at different points depending on which reference is evaluated first.

4. **Debugging**: Lazy evaluation makes execution traces harder to follow. The order of operations in the trace doesn't match the order of declarations in the source.

#### Are we reinventing Haskell?

Arguably yes. Consider what Barnum already has:

| Haskell | Barnum |
|---|---|
| `>>=` (monadic bind) | `pipe` / `.then()` |
| `do` notation | `pipe(a, b, c)` |
| `let` in `do` blocks | `let_` (this proposal) |
| `pure` / `return` | `constant` |
| `void` | `drop` |
| `fmap` | `forEach` (on arrays) |
| `<*>` (applicative) | `parallel` |
| Case analysis | `branch` |
| Recursion | `loop` / `stepRef` |
| Type classes | Handler schemas (structural) |

Adding lazy `let` bindings with memoization makes the parallel even tighter. Barnum would essentially be a lazy, first-order, effectful workflow DSL — which is a restricted Haskell with explicit effects.

The question is whether this is a good thing or a bad thing. Haskell's laziness is its most controversial feature. The benefits (composability, modularity) are real but so are the costs (space leaks, unpredictable performance, reasoning difficulty).

**Recommendation**: Start with eager evaluation. It's simpler, predictable, and covers the common case (bind a few values, use them all). If we find ourselves writing bindings that are conditionally needed, we can add a `lazy_let_` variant or a per-binding `lazy: true` flag. Don't default to lazy.

The deeper insight: Barnum is already a language runtime (see BARNUM_AS_LANGUAGE.md). Every feature we add makes it more of a language. `let` bindings are variables; lazy `let` is call-by-need; RAII is linear types. We should be deliberate about which language features we adopt and recognize that each one moves us further along the "accidentally designed a programming language" spectrum.

## Relation to existing combinators

If `let` exists, several patterns simplify:

| Current pattern | With `let` |
|---|---|
| `augment(pipe(extract, transform))` | `let_({ x: pipe(extract, transform) }, ({ x }) => ...)` |
| `tap(sideEffect)` | Side effect in a binding that's never referenced (eager) |
| `withResource({ create, action, dispose })` | See "Interaction with RAII" below |

### Interaction with RAII

`let` doesn't replace `withResource` because it has no cleanup semantics. A `let` binding that creates a worktree has no way to delete it when the body exits. You'd need:

```ts
let_({
  resource: createWorktree,
}, ({ resource }) =>
  pipe(
    resource.then(implement),
    resource.then(commit),
    resource.then(deleteWorktree),  // manual cleanup, not RAII
  ),
)
```

This is fragile — if `implement` fails, `deleteWorktree` never runs. `withResource` guarantees cleanup. `let` + RAII (see RAII.md if it exists) could replace `withResource`, but `let` alone cannot.

## Open questions

1. **Should the pipeline input be implicitly available as a variable?** In the `withResource` example, the original input is just as important as the resource. Should `let_` implicitly bind the input under a special name (e.g., `_` or `input`)? Or is `identity()` in the bindings sufficient?

2. **Naming**: `let_` (trailing underscore because `let` is a JS keyword), `bind`, `with`, `assign`, `declare`? The semantics are closest to `let` in ML/Haskell: evaluate bindings, make them available in a body.

3. **Interaction with steps**: Can a `let` binding reference a named step? Can a step body contain `let`? There's no fundamental obstacle, but the interaction with step registration and mutual recursion needs thought.

4. **Multiple references to the same variable**: A VarRef can appear multiple times in the body AST. The scheduler evaluates the binding once and caches the value. Multiple references return the same cached result.

5. **Body input type**: What is the body's pipeline input? Options:
   - `never` — the body has no pipeline input, only variable references. Forces all data through variables.
   - `TIn` — the body receives the same input as the let block. Variables are supplementary, not replacement.
   - The second option is more flexible and avoids forcing the user to bind the input explicitly.
