# Speculation: Eliminating Pipe

What happens if pipe goes away and every combinator explicitly takes its data as a positional argument?

## The observation

Pipe is linear: `pipe(a, b, c)` threads one value through a chain. But real workflows are DAGs. Step C might need the output of A and B. Step D might need A's output and the original input. The linear model forces workarounds (augment, tap, pick) to route data sideways through the pipeline.

The current workarounds are all symptoms of the same mismatch: linear composition applied to non-linear data flow.

## The alternative

Every combinator takes its dependencies as explicit AST-node arguments. Combinators are functions from AST nodes to AST nodes. Composition is done by passing one node's output as another node's input.

### Current API

```ts
pipe(
  pipe(typeCheck, classifyErrors),
  branch({
    HasErrors: pipe(forEach(fix).drop(), stepRef("TypeCheck")),
    Clean: drop(),
  }),
)
```

`branch` doesn't say what it branches on. It's positional: whatever the pipeline value is. `forEach` doesn't say what it iterates. Same implicit pipeline value.

### Alternative API

```ts
branch(chain(typeCheck, classifyErrors), {
  HasErrors: chain(forEach(fix).drop(), stepRef("TypeCheck")),
  Clean: drop(),
})
```

Now `branch` explicitly takes the action that produces the tagged union. The cases are the second argument. No change to what happens at runtime; only the surface syntax changes.

Similarly for `option.map`:

```ts
// Current: postfix, implicit subject
action.mapOption(transform)

// Alternative: explicit subject
mapOption(action, transform)
```

And `forEach`:

```ts
// Current: implicit subject
pipe(listFiles, forEach(processFile))

// Alternative: explicit subject
forEach(listFiles, processFile)
```

This is a small change so far. `branch(subject, cases)` vs `pipe(subject, branch(cases))` is cosmetic. The interesting part is what happens when you follow the idea further.

## Following the thread: sequences as accumulation

If every combinator takes its data explicitly, what replaces `pipe` for sequencing?

In the pipe model, `pipe(a, b, c)` means: run a, feed to b, feed to c. The data flow is implicit and linear.

Without pipe, you'd write:

```ts
const aResult = a;
const bResult = chain(a, b);
const cResult = chain(chain(a, b), c);
```

But that's just pipe with extra syntax. The real question: what if sequencing accumulates instead of threading?

```ts
sequence(
  { image: chain(pick("repo", "sha"), buildImage) },
  { tests: runTests },                              // receives { image }
  { deploy: chain(pick("imageTag", "environment"), deployToK8s) },
  chain(pick("repo", "environment", "podName"), notifySlack),
)
```

Each step receives the merged outputs of all previous steps. Each step can name its output so downstream steps can reference it. Steps that don't name their output contribute nothing to the context (side effects only, like `tap` today).

Compare with `declare`:

```ts
declare({
  image: chain(pick("repo", "sha"), buildImage),
}, ({ image }) =>
  pipe(
    image.then(pick("imageTag")).then(runTests).dropOutput(),
    image.then(pick("imageTag", "environment"))
      .then(deployToK8s),
    // etc.
  ),
)
```

The `declare` version nests callbacks to introduce bindings. The `sequence` version flattens the same bindings into a list. Both make data available by name to downstream steps.

## Deployment pipeline comparison

The deployment example from LET_BINDINGS.md, rewritten with accumulative sequencing:

```ts
type DeployInput = {
  repo: string;
  sha: string;
  environment: string;
};

// Each handler takes only what it needs. Invariant types enforced.
const buildImage     = createHandler<{ repo: string; sha: string }, { imageTag: string }>(...);
const runTests       = createHandler<{ imageTag: string }, { passed: boolean }>(...);
const deployToK8s    = createHandler<{ imageTag: string; environment: string }, { podName: string }>(...);
const notifySlack    = createHandler<{ repo: string; environment: string; podName: string }, void>(...);
const updateDashboard = createHandler<{ repo: string; sha: string; podName: string }, void>(...);
```

### With pipe (current)

```ts
pipe(
  augment(pipe(pick("repo", "sha"), buildImage)),
  tap(pipe(pick("imageTag"), runTests)),
  augment(pipe(pick("imageTag", "environment"), deployToK8s)),
  tap(pipe(pick("repo", "environment", "podName"), notifySlack)),
  pipe(pick("repo", "sha", "podName"), updateDashboard),
)
```

The pipeline value grows: `DeployInput -> DeployInput & { imageTag } -> ... & { podName }`. Every step must accept and thread fields it doesn't use.

### With declare

```ts
declare({
  input: identity<DeployInput>(),
  image: pipe(pick("repo", "sha"), buildImage),
}, ({ input, image }) =>
  pipe(
    image.then(pick("imageTag")).then(runTests).dropOutput(),
    parallel(
      image.then(pick("imageTag")),
      input.then(pick("environment")),
    ).then(merge()).then(deployToK8s),
    declare({
      deploy: identity<{ podName: string }>(),
    }, ({ deploy }) =>
      pipe(
        parallel(
          input.then(pick("repo", "environment")),
          deploy.then(pick("podName")),
        ).then(merge()).then(notifySlack).dropOutput(),
        parallel(
          input.then(pick("repo", "sha")),
          deploy.then(pick("podName")),
        ).then(merge()).then(updateDashboard),
      ),
    ),
  ),
)
```

Precise, but verbose. Nested declares for mid-pipeline bindings. Manual parallel + merge to assemble inputs from multiple sources.

### With accumulative sequence

```ts
sequence<DeployInput>(
  { image: pipe(pick("repo", "sha"), buildImage) },
  pipe(select("image", pick("imageTag")), runTests).discard(),
  { deploy: pipe(
      select("image", pick("imageTag")),
      select("input", pick("environment")),
      merge(),
      deployToK8s,
    ),
  },
  pipe(
    select("input", pick("repo", "environment")),
    select("deploy", pick("podName")),
    merge(),
    notifySlack,
  ).discard(),
  pipe(
    select("input", pick("repo", "sha")),
    select("deploy", pick("podName")),
    merge(),
    updateDashboard,
  ),
)
```

`select("image", pick("imageTag"))` reads from the accumulated context by name, then narrows the value. Flat, no nesting. But we've reintroduced `pipe` inside each step, and `select` is just VarRef by another name.

## What this actually is

Following the idea to its conclusion, accumulative sequencing converges on one of two things:

**Option A: Flat declare.** The sequence is declare without callback nesting. Named bindings, flat list, each step gets the full context. This is `declare` with syntactic sugar that avoids the callback pyramid.

```ts
// This:
sequence(
  { x: actionA },
  { y: actionB },
  actionC,
)

// Is sugar for:
declare({ x: actionA }, ({ x }) =>
  declare({ y: actionB }, ({ y }) =>
    actionC,
  ),
)
```

The semantics are identical. The difference is ergonomic: flat vs nested. But the flat form loses something. In `declare`, the binding expression runs against the pipeline input. In the flat sequence, what is the "input" to step 2? The original pipeline input? The output of step 1? The merged context? This ambiguity doesn't exist in `declare` because the callback signature makes it explicit.

**Option B: Dataflow graph.** Each step explicitly names its dependencies, and the scheduler resolves execution order. This is a different computation model entirely.

```ts
const image = node(pipe(pick("repo", "sha"), buildImage));
const tests = node(pipe(from(image, pick("imageTag")), runTests));
const deploy = node(pipe(
  merge(
    from(image, pick("imageTag")),
    from(input, pick("environment")),
  ),
  deployToK8s,
));
```

This is a genuine DAG, not a linear sequence. The scheduler topologically sorts the nodes and parallelizes where dependencies allow. `image` and `tests` can start simultaneously if they don't depend on each other.

This is more expressive than pipe, but it's a fundamentally different system. Pipe gives you a total order on execution. A DAG gives you a partial order. The scheduler must determine execution order from dependency analysis rather than source order.

## Branch and option.map under each model

### Current (pipe-based, implicit subject)

```ts
// branch: dispatches on pipeline value
pipe(typeCheck, classifyErrors).branch({
  HasErrors: fixStep,
  Clean: drop(),
})

// option.map: maps over pipeline value (which must be Option<T>)
pipe(lookupUser).mapOption(enrichProfile)
```

The subject is always "whatever the pipeline is currently carrying." Branch and mapOption don't name their subject because there's only one candidate.

### With explicit subject (no pipe)

```ts
// branch: first arg is the thing being dispatched on
branch(chain(typeCheck, classifyErrors), {
  HasErrors: fixStep,
  Clean: drop(),
})

// option.map: first arg is the option
mapOption(lookupUser, enrichProfile)
```

The subject is the first argument. This is more like standard function call syntax. When there's only one data source (linear flow), this is equivalent to the postfix form with more punctuation. When there are multiple data sources (DAG), the explicit argument lets you point at a specific upstream node.

### The interesting case: branch inside an accumulative sequence

```ts
sequence<DeployInput>(
  { errors: pipe(typeCheck, classifyErrors) },
  branch(select("errors"), {
    HasErrors: fixStep,
    Clean: drop(),
  }),
)
```

`select("errors")` pulls a specific binding from the accumulated context. This is a VarRef. If every combinator takes explicit subjects, and subjects are selected from a named context, then the context IS declare's environment.

## The convergence

Every path leads to the same place: named bindings with explicit references. Whether you spell it as:

- `declare({ x: a }, ({ x }) => x.then(b))` (callback + VarRef)
- `sequence({ x: a }, pipe(select("x"), b))` (flat + select)
- `const x = node(a); node(from(x), b)` (dataflow + from)

The underlying mechanism is the same: evaluate an expression, bind the result to a name, reference the name later.

`pipe` is the degenerate case where every step references exactly one binding: the output of the immediately preceding step. It's anonymous, positional, and linear. Any deviation from that pattern (accessing an earlier result, skipping a result, accessing multiple results) requires escaping the linear model.

## What pipe elimination gets you

1. **No prop drilling.** Every intermediate result is in scope by name. No augment/tap/pick workarounds.

2. **Explicit data dependencies.** Each combinator declares what it reads. The scheduler can parallelize independent steps automatically. `notifySlack` depends on `input` and `deploy`; `updateDashboard` depends on `input` and `deploy`. They don't depend on each other, so the scheduler can run them in parallel without the user writing `parallel(...)`.

3. **Simpler mental model for complex workflows.** The deployment pipeline reads top to bottom. No nesting, no callback pyramid. Each step says where its data comes from.

## What pipe elimination costs you

1. **Simple pipelines get noisier.** `pipe(a, b, c)` is clean for linear flows. `chain(chain(a, b), c)` or `sequence({ _0: a }, { _1: b }, c)` is worse for the common case. Most pipelines ARE linear. Optimizing for the complex case penalizes the simple case.

2. **Two composition mechanisms.** Even in the accumulative model, you still need something like `chain` or `pipe` for combining actions within a single step: `pipe(pick("imageTag"), runTests)`. The flat sequence handles inter-step composition. Intra-step composition still needs chaining. You haven't eliminated pipe; you've pushed it down a level.

3. **Execution order ambiguity.** In `pipe(a, b, c)`, the order is total: a then b then c. In a named-binding model, the order is only partially constrained by dependencies. `{ x: a }, { y: b }` where y doesn't reference x: should they run sequentially or in parallel? `declare` answers this (bindings in parallel, body sequential). A flat sequence would need its own answer.

4. **Type inference burden.** `pipe` overloads chain types left to right, and TypeScript handles this well. An accumulative context is an intersection type that grows with each step. The type of the context after step N is `Step1Output & Step2Output & ... & StepNOutput`, which stresses the type checker and produces inscrutable error messages when something doesn't match.

5. **Loss of totality.** In `pipe(a, b)`, b MUST consume a's output. The connection is enforced by the type system. In a named-binding model, a step could silently ignore bindings. Unreferenced bindings are dead code that still executes (in the eager model). The type system doesn't force you to use what you bind.

## The relationship to declare

Declare is this idea, scoped. Rather than replacing the entire composition model, declare adds named bindings as an opt-in layer on top of pipe:

- **Linear flow**: use pipe. It's the right tool.
- **Non-linear flow**: use declare to name intermediate results and reference them.
- **Nesting**: declare scopes bindings. The environment doesn't grow unboundedly. Inner declares add to the scope; outer scopes are visible. Standard lexical scoping.

Eliminating pipe in favor of "everything is named" is saying: non-linear flow is the common case, so optimize for it. The evidence from the existing demos is mixed. `convert-folder-to-ts` is almost entirely linear. `identify-and-address-refactors` has significant non-linearity (worktree used in multiple places, context threaded through taps). Both exist in real workflows.

Pipe + declare gives you both modes. Eliminating pipe forces the verbose mode everywhere.

## Where this speculation is useful

Even if we don't eliminate pipe, the analysis clarifies what declare IS:

**Declare is the minimal addition that gives pipe-based composition access to non-linear data flow.** It doesn't replace pipe for the linear case. It doesn't introduce a fundamentally new computation model. It adds one thing (named bindings in scope) that lets the user escape the linear model when they need to.

If declare's implementation turns out to be wrong (evaluation strategy, scope semantics, scheduler integration), the fix is to change declare's implementation. The need for named bindings in non-linear data flow is real regardless of how the bindings are evaluated or stored.

The user's earlier concern ("DECLARE evaluates and stores things, and that's weird") maps to a specific axis of this design space: should bindings be eager or lazy? Should the values be cached or recomputed? These are evaluation strategy questions, not "should named bindings exist?" questions. The answer to the latter is yes, under any formulation.
