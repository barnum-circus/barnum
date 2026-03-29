# Missing Language Features

Speculative analysis of programming features Barnum lacks and how they might be addressed. Organized by impact and feasibility.

## Variables (Context / Environment)

**The problem**: Every value must be explicitly threaded through the pipeline. If step 1 produces `{ branch }` and step 5 needs it, every step 2–4 must accept and pass through `{ branch }` — even if they don't use it. This is the "prop drilling" problem.

**How it might work**: A context/environment that flows implicitly alongside the pipeline value. Each step can read from context without declaring it as input. Steps can write to context, and the writes are visible to downstream steps.

**Implementation sketch**: The scheduler maintains a `context: Map<string, Value>` alongside each frame's value. Builtins like `setContext("key", extractField("key"))` and `getContext("key")` read/write it. The AST has `WithContext` nodes that scope context additions.

```ts
pipe(
  withContext({ branch: extractField("branch") },
    pipe(
      implement,   // can read "branch" from context
      commit,      // can read "branch" from context
      createPR,    // can read "branch" from context
    ),
  ),
)
```

**The monoid observation**: Context writes must be commutative and associative if they can happen in parallel. Two parallel steps writing to the same context key is a race condition. Options:
1. Disallow parallel writes to the same key (static analysis or runtime error)
2. Require writes to be monoidal (append-only arrays, merge-only objects)
3. Scope context writes — each parallel branch gets its own context copy

Option 3 is cleanest: parallel branches can write to context, but their writes are local. The join point collects results as values (as today), and context changes don't leak across branches.

**Priority**: HIGH. This is the single most impactful missing feature. It eliminates `tap`, `augment`, and `merge` as workarounds for threading data through side-effectful steps.

## Caching / Memoization

**The problem**: If the same handler is called with the same input multiple times (e.g., `typeCheck` in a fix loop), we redo the work.

**How it might work**: A `cached(action)` combinator that memoizes results by input hash. The scheduler maintains a cache keyed on `(handler_id, hash(input))`.

```ts
pipe(
  cached(typeCheck),   // second call with same input returns cached result
  classifyErrors,
  ...
)
```

**Context-based alternative**: The scheduler could maintain a `cache: Map<string, Value>` in context. Handlers opt in to caching via metadata. No new AST node needed — it's a scheduler optimization.

**Priority**: LOW. Workflows rarely repeat the exact same computation. More useful for production than for the language itself.

## Panicking / Error Handling

**The problem**: If a handler throws, the entire workflow fails. There's no try/catch, no recovery, no partial failure handling.

**How it might work**: A `tryAction` combinator that catches handler errors and returns a Result-like discriminated union:

```ts
pipe(
  tryAction(riskyHandler),
  // produces { kind: "Ok", value: result } | { kind: "Err", value: error }
  branch({
    Ok: processResult,
    Err: handleError,
  }),
)
```

**Rust side**: The scheduler wraps handler execution in a try/catch. On error, instead of propagating the error up, it produces a tagged `Err` value and continues the pipeline.

**Relation to scope/continuations**: `tryAction` is another delimiter. If a handler panics, the scope catches it and produces an Err tag. This is exactly exception handling as a delimited continuation.

**Priority**: HIGH. Without this, any handler failure is fatal. Real workflows need graceful degradation.

## Deferring / Finally

**The problem**: `withResource` handles the RAII case (create → use → cleanup), but there's no general "run this at the end regardless of success/failure" mechanism. If the action inside `withResource` fails, does dispose still run? Currently no — a handler error kills the workflow.

**How it might work**: `defer(cleanup)` registers an action to run when the enclosing scope exits, regardless of how it exits. Combined with `tryAction`:

```ts
scope(({ exit, defer }) =>
  pipe(
    createResource,
    defer(deleteResource),  // runs on scope exit, even on error
    useResource,
    exit(),
  ),
)
```

**Context-based alternative**: The scheduler maintains a defer stack per scope. When a scope exits (normally or via error), it pops and executes deferred actions in reverse order.

**Priority**: MEDIUM. Needed for robust resource management, but `withResource` covers the common case.

## Pausing / Waiting

**The problem**: Some workflows need to wait for an external event (human approval, CI completion, webhook). The current model requires keeping a subprocess alive during the wait, which is wasteful and fragile.

**How it might work**: A `pause(resumeCondition)` builtin that serializes the workflow state to disk and exits. An external trigger (webhook, CLI command) resumes execution by loading the state and continuing.

```ts
pipe(
  createPR,
  pause({ event: "pr-approved", key: extractField("prUrl") }),
  // execution resumes here when the event fires
  deploy,
)
```

**Rust side**: `pause` serializes the `WorkflowState` (current frame stack + all intermediate values) to a file. A `barnum resume --state <file> --event <json>` command rehydrates and continues.

**Prerequisite**: Pre-compilation (PRECOMPILATION.md) — the AST must be serializable separately from the execution state. Also requires that handler file paths are stable across resume.

**Priority**: HIGH for production use. This is what makes Barnum a durable workflow engine rather than a batch orchestrator.

## Reading Parameters / Standard Input

**The problem**: Workflows currently can only start with `constant()` or `never` (no input). There's no way to accept external parameters at launch time.

**How it might work**: The entry point could accept initial input from the CLI:

```bash
barnum run --config workflow.json --input '{"folder": "/path"}'
```

The `workflow` action's input type would be the parameter type instead of `never`:

```ts
workflowBuilder()
  .workflow(() =>
    // input is now { folder: string } instead of never
    pipe(listFiles, forEach(processFile)),
  )
  .run({ folder: "/path/to/project" });
```

**Priority**: MEDIUM. Currently worked around with `constant()`, but parameterized workflows are more composable and testable.

## Capabilities / Permissions

**The problem**: Any handler can do anything — read files, make network requests, delete databases. There's no way to restrict what a handler can access.

**Reality check**: We can't meaningfully restrict a TypeScript subprocess. It has full OS access. Capabilities would be metadata/documentation, not enforcement. Not worth the complexity.

**Priority**: SKIP. Not enforceable at the TypeScript handler level.

## Mutation / Accumulation

**The problem**: How do you accumulate results across iterations or parallel branches? Currently, `forEach` collects results into an array and `merge` combines objects. But there's no general "accumulate" operation (fold/reduce).

**The monoid insight**: As long as the accumulation operation is an associative monoid (identity element + associative binary operation), the order of parallel contributions doesn't matter. Examples:
- Array append: identity = `[]`, operation = `concat`
- Object merge: identity = `{}`, operation = `Object.assign`
- Counter: identity = `0`, operation = `+`
- Set union: identity = `∅`, operation = `∪`

**How it might work**: A `reduce(monoid, action)` combinator that runs action on each element and folds results:

```ts
reduce({
  identity: 0,
  combine: (a, b) => a + b,
}, countErrors)
```

But wait — `combine` is a JavaScript function, and Barnum AST is data. The combine function would need to be a builtin or a handler. This gets complicated.

**Simpler approach**: Don't add reduce. Instead, provide more builtins for common monoidal operations: `sum`, `count`, `collect` (array), `merge` (object). The set of monoids is fixed and known.

**Priority**: LOW. `forEach` + `flatten` + `merge` cover most cases.

## Missing Control Flow

### Pattern matching (deep branch)

`branch` dispatches on a single `kind` field. What about matching on nested structure or multiple fields?

```ts
match({
  { status: "ok", retries: 0 }: handleFirstSuccess,
  { status: "ok" }: handleRetrySuccess,
  { status: "error" }: handleError,
})
```

This is complex to implement in the AST. Simpler: chain of `branch` + `extractField` operations. Not worth a new primitive.

### Conditional (if/else without tagging)

`branch` requires input to be a tagged union. What about simple boolean conditions?

```ts
when(
  extractField("count").greaterThan(10),
  truncate,
  identity(),
)
```

This requires expression evaluation in the AST — a slippery slope toward a full expression language. Better: have the handler return a tagged union and use `branch`.

**Priority**: LOW. `branch` with explicit tagging is more principled.

### Parallel with different inputs (fan-out)

`parallel` sends the same input to all branches. What about sending different data to each?

Already solved: `parallel(pipe(extractField("a"), actionA), pipe(extractField("b"), actionB))`.

## Feature Priority Summary

1. **Error handling (tryAction)** — fatal errors are unacceptable in production
2. **Context / variables** — eliminates the biggest source of boilerplate
3. **Scope + continuations** — generalizes loop, enables early return (see LOOP_WITH_CLOSURE.md)
4. **Pausing / durable execution** — transforms Barnum from batch to workflow engine
5. **Parameterized workflows** — more composable entry points
6. **Deferring / finally** — robust resource cleanup
7. Everything else — nice to have, not blocking
