# Let Bindings

## Motivation

Barnum pipelines thread data through each step as a single value. When a step produces a result that later steps need, the intervening steps must accept and pass through fields they don't use — classic prop drilling. The current workarounds (`tap`, `augment`, `merge`) keep the data flowing but add ceremony and obscure intent.

A `let` combinator would introduce named variable bindings: evaluate a set of expressions in parallel, bind the results to names, and make those names available as AST references inside a callback. The callback constructs the body of the `let` block using those references.

This is the AST-level equivalent of JavaScript's `const { a, b } = ...`:

```ts
// JavaScript:
const { branch, worktreePath } = await createWorktree(input);
await implement({ ...input, worktreePath, branch });

// Barnum today — manual threading:
pipe(
  augment(pipe(deriveBranch, createWorktree)),
  // now { ...input, worktreePath, branch } flows
  tap(implement),
  tap(commit),
)

// Barnum with let bindings:
let_({
  resource: pipe(deriveBranch, createWorktree),
}, ({ resource }) =>
  pipe(
    resource,            // evaluates to the createWorktree result
    tap(implement),
    tap(commit),
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
  body: (vars: { [K in keyof TBindings]: TypedAction<never, ExtractOutput<TBindings[K]>> }) => TypedAction<???, TOut>,
): TypedAction<TIn, TOut>
```

- `bindings`: An object mapping variable names to ASTs. Each AST receives the pipeline input (`TIn`) and produces its bound value. All bindings evaluate in parallel.
- `body`: A JavaScript callback that receives named AST references and returns the body AST. The callback runs at definition time — it's a metaprogramming construct, not a runtime function.
- Each variable reference is a `TypedAction<never, T>` — it takes no pipeline input (the value is already captured), and produces the bound type.

### Example: withResource replacement

```ts
let_({
  resource: pipe(deriveBranch, createWorktree),
  input: identity(),  // capture the original input
}, ({ resource, input }) =>
  pipe(
    // merge resource and input into flat context
    parallel(resource, input).then(merge()),
    tap(implement),
    tap(commit),
    augment(pipe(preparePRInput, createPR)),
  ),
)
```

### Example: avoiding prop drilling

```ts
let_({
  branch: pipe(extractField("description"), deriveBranch),
}, ({ branch }) =>
  pipe(
    // branch is available anywhere without threading
    implement,      // doesn't need to pass branch through
    commit,         // doesn't need branch either
    // only the step that needs branch references it
    parallel(branch, preparePRInput).then(merge()).then(createPR),
  ),
)
```

## AST representation

Two options for how variables work at the AST level.

### Option A: Variable references are AST nodes

Add a new AST variant:

```ts
// New Action variant
| LetAction

interface LetAction {
  kind: "Let";
  bindings: Record<string, Action>;  // evaluated in parallel
  body: Action;                       // may contain VarRef nodes
}

// New BuiltinKind variant
| { kind: "VarRef"; name: string }
```

The `VarRef` builtin resolves to the bound value at runtime. The scheduler maintains a variable environment (scope stack) alongside each execution frame.

**Pros**: Clean separation — bindings are declared once, references appear wherever needed. The scheduler can optimize (evaluate once, reference many times).

**Cons**: New AST node, new scheduler concept (variable environment). VarRef is a new kind of node that doesn't take pipeline input — it reaches into a side channel.

### Option B: Inline substitution (desugar to parallel + extractIndex)

No new AST nodes. `let_` is pure sugar:

```ts
function let_(bindings, body) {
  // 1. Evaluate all bindings in parallel → tuple
  const bindingActions = Object.values(bindings);
  const keys = Object.keys(bindings);

  // 2. Create variable references as extractIndex ASTs
  const vars = {};
  keys.forEach((key, i) => {
    vars[key] = extractIndex(i);  // extracts from the parallel result tuple
  });

  // 3. Call body with the references
  const bodyAst = body(vars);

  // 4. Wrap: parallel(bindings...) → body
  return pipe(
    parallel(...bindingActions),  // → [val0, val1, ...]
    bodyAst,                      // body uses extractIndex(i) to access vars
  );
}
```

Each variable reference is `extractIndex(i)` — when placed in the body, it extracts the i-th element from the parallel results tuple.

**Pros**: No new AST nodes. Purely a TypeScript-side convenience. Works with the existing scheduler.

**Cons**: Each variable reference expects the full tuple as input. If the body is `pipe(resource, implement)`, then `resource` is `extractIndex(0)` which takes the tuple, but `implement` takes the extracted value. This works — pipe chains them. But if you want to reference a variable *inside* a pipeline that's already processing something else, you need to restructure. The variable references are positional — they only work when the pipeline value is the tuple from the parallel node.

### Option B's limitation

Consider:

```ts
let_({
  branch: deriveBranch,
  worktree: pipe(deriveBranch, createWorktree),
}, ({ branch, worktree }) =>
  pipe(
    worktree,        // extractIndex(1) — extracts from tuple. OK.
    implement,       // takes worktree output. OK.
    // But now I want to reference `branch` again here.
    // The pipeline value is implement's output, not the tuple.
    // `branch` (extractIndex(0)) expects the tuple as input.
    // This doesn't work.
  ),
)
```

To use `branch` after `implement`, you'd need to thread the tuple through, which defeats the purpose.

**This is why Option A exists.** VarRef reaches into a side channel (the variable environment) regardless of what the pipeline value currently is. It's context, not pipeline data.

### Option C: Desugar using `augment` internally

Another approach to make Option B work: `let_` wraps the body so the tuple is always available via the pipeline value. Each variable reference uses `extractField` on a known key.

```ts
function let_(bindings, body) {
  const keys = Object.keys(bindings);
  const bindingActions = Object.values(bindings);

  // Variable references extract from the object
  const vars = {};
  keys.forEach(key => {
    vars[key] = extractField(key);
  });

  const bodyAst = body(vars);

  // parallel(bindings...) → object { key0: val0, key1: val1, ... }
  // Then body runs with that object as the pipeline value
  return pipe(
    parallel(...bindingActions),
    // Convert tuple to named object (new builtin or use Tag+Merge pattern)
    tupleToObject(keys),  // → { branch: ..., worktree: ... }
    bodyAst,
  );
}
```

This has the same problem as Option B: once the body pipeline transforms the value, the variable names are gone from the pipeline.

The fundamental issue: **variables are an environment, not pipeline data.** Pipeline data flows linearly. Variables need to be accessible from any point, not just the head of the pipeline.

## The environment question

Option A (VarRef + scheduler environment) is the only option that gives true variable semantics. But it raises questions:

1. **Scope**: Variables are lexically scoped to the `let` body. The scheduler pushes a scope on entering `Let`, pops on exit. Nested `let` blocks shadow outer bindings.

2. **Evaluation**: Bindings evaluate in parallel when the `Let` node is entered. Each binding receives the pipeline input and produces its value. Values are stored in the environment.

3. **References**: `VarRef("x")` looks up `"x"` in the current environment. It ignores the pipeline input — the value comes from the environment, not the pipeline.

4. **Interaction with forEach**: If a `let` is inside a `forEach`, each iteration gets its own scope. Variables don't leak across iterations.

5. **Interaction with parallel**: If a `let` is inside a `parallel`, each branch gets a copy of the environment. Bindings in one branch don't affect the other.

6. **No mutation**: Variables are bound once and never reassigned. This is `let`, not `var`. The environment is append-only within a scope.

### Implicit scoping via functions

One nice property: handlers (TypeScript functions) can't access let-bound variables. The only way to get data into a handler is through its input. This means variables are implicitly scoped to the AST — they're a structuring mechanism for the workflow graph, not a way to smuggle state into handlers.

This is actually desirable. It means handlers remain pure functions of their input. Variables reduce prop drilling in the *workflow graph*, but handlers still declare exactly what data they need.

## Relation to existing combinators

If `let` exists, several patterns simplify:

| Current pattern | With `let` |
|---|---|
| `augment(pipe(extract, transform))` | `let_({ x: pipe(extract, transform) }, ({ x }) => ...)` |
| `tap(sideEffect)` | Side effect in a `let` binding that's never referenced |
| `withResource({ create, action, dispose })` | `let_({ resource: create }, ({ resource }) => pipe(resource.then(action), resource.then(dispose)))` — though RAII ordering (dispose after action) needs care |
| `parallel(a, identity()).then(merge())` | `let_({ result: a }, ({ result }) => ...)` |

`tap` and `augment` become less necessary but not obsolete — they're still convenient for simple cases where a full `let` block is overkill.

## Open questions

1. **Is Option A worth the scheduler complexity?** Adding a variable environment to the Rust scheduler is a meaningful change. The frame-based execution model currently has no concept of "ambient state" — everything flows through the pipeline value. Variables would be the first side channel.

2. **Can we prototype with Option B first?** Option B (desugar to parallel + extractIndex) works for the simple case where all variable references appear at the top of the body pipeline. We could ship this and see how far it gets before committing to scheduler changes.

3. **Should the input also be available as a variable?** In the `withResource` example, the original input is just as important as the resource. Should `let_` implicitly bind the input under a special name (e.g., `_` or `input`)? Or is `identity()` in the bindings sufficient?

4. **Naming**: `let_` (trailing underscore because `let` is a JS keyword), `bind`, `with`, `assign`, `declare`? The semantics are closest to `let` in ML/Haskell: evaluate bindings, make them available in a body.

5. **Interaction with steps**: Can a `let` binding reference a named step? Can a step body contain `let`? There's no fundamental obstacle, but the interaction with step registration and mutual recursion needs thought.

6. **Multiple references to the same variable**: With Option A, a VarRef can appear multiple times in the body AST. The scheduler evaluates the binding once and caches the value. With Option B, each extractIndex is a separate AST node but they all point at the same parallel result slot — also evaluated once. Both options handle this correctly.
