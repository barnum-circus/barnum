# Best Practices

## Handler design

Handlers are the leaf nodes — they do work. Everything else is plumbing. Keep them minimal and let the pipeline layer handle composition.

### Handlers cannot call other handlers

Handlers run in isolated subprocesses. You cannot call `.handle()` from inside one handler to invoke another. All composition happens in the pipeline definition via combinators (`pipe`, `.then()`, `bindInput`, etc.). If you need the output of one handler as input to another, chain them in the pipeline.

### One job per handler

A handler does one thing: transform data, call an external service, read a file, invoke an LLM. All plumbing — splitting fields, merging objects, routing to different paths — belongs in the pipeline layer using `bindInput`, `getField`, `wrapInField`, `augment`, `pick`, and `branch`.

```ts
// Avoid: handler does plumbing + work
export const analyzeAndRoute = createHandler({
  handle: async ({ value }) => {
    const result = await callClaude({ prompt: `Analyze ${value.file}` });
    // Don't route inside the handler — that's pipeline work
    if (result.severity === "critical") { ... }
    return { ...value, result };  // Don't merge input back in — pipeline does that
  },
}, "analyzeAndRoute");

// Prefer: handler does one thing, pipeline handles the rest
export const analyze = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: analysisSchema,
  handle: async ({ value }) => {
    return await callClaude({ prompt: `Analyze ${value.file}` });
  },
}, "analyze");

// Pipeline routes and merges:
analyze.branch({ Critical: escalate, Low: log })
```

### Retry and loop logic belongs in the pipeline

A handler makes exactly one attempt and returns a `Result` on failure. The pipeline uses `loop`, `tryCatch`, and `unwrapOr` to handle retries, back-off, and fallback paths.

```ts
// Avoid: retry inside the handler
export const callApi = createHandler({
  handle: async ({ value }) => {
    for (let i = 0; i < 3; i++) {
      try { return await fetch(value.url); }
      catch { await sleep(1000 * i); }
    }
    throw new Error("failed after retries");
  },
}, "callApi");

// Prefer: handler does one attempt, pipeline handles retry
export const callApi = createHandler({
  outputValidator: Result.schema(responseSchema, z.string()),
  handle: async ({ value }) => {
    try { return { kind: "Result.Ok", value: await fetch(value.url) }; }
    catch (e) { return { kind: "Result.Err", value: e.message }; }
  },
}, "callApi");

// Pipeline retries:
loop((recur, done) =>
  callApi.branch({ Ok: done, Err: logAndWait.then(recur) })
)
```

### Don't return data the pipeline already knows

If the file path was passed in as input, don't make the handler echo it back. The pipeline can merge it back via `augment`, `wrapInField`, or `bindInput`.

```ts
// Avoid: handler parrots its input back
export const countLines = createHandler({
  handle: async ({ value }) => {
    const content = readFileSync(value.file, "utf-8");
    return { file: value.file, lineCount: content.split("\n").length };
    //       ^^^^^^^^^^^^^^^ pipeline already has this
  },
}, "countLines");

// Prefer: handler returns only what it computed
export const countLines = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.number(),
  handle: async ({ value }) => {
    const content = readFileSync(value.file, "utf-8");
    return content.split("\n").length;
  },
}, "countLines");

// Pipeline merges if needed:
augment(countLines)  // { file } → { file, lineCount }
```

---

## Pipeline composition

### Use `bindInput` when multiple steps need the same value

If a handler's output is consumed by one step but also needed later (e.g., a worktree path used for type-check, commit, and PR creation), wrap the section in `bindInput` rather than threading the value through every handler's input/output.

```ts
// Avoid: every handler accepts and returns worktreePath
implement.then(typeCheck).then(commit).then(createPR)
// Each handler must include worktreePath in its input AND output — coupling city

// Prefer: bindInput captures the shared context
bindInput<Params>((params) =>
  params.pick("worktreePath", "description").then(implement).drop()
    .then(params.pick("worktreePath").then(typeCheckFix).drop())
    .then(params.pick("worktreePath").then(commit).drop())
    .then(params.pick("branch", "description").then(createPR))
)
```

### Use `augment` to carry context forward

When you need both a handler's input and output downstream, `augment(handler)` merges them. Avoids handlers returning their own input.

```ts
// augment(countLines): { file: string } → { file: string, lineCount: number }
listFiles.iterate().map(augment(countLines)).collect()
```

### Prefer `.iterate().map()` over `forEach`

`forEach` is a low-level AST node. The Iterator API is the user-facing equivalent with better composability — you can chain `.filter()`, `.take()`, `.flatMap()` before collecting.

```ts
// Avoid: raw forEach, no ability to filter/take/transform
forEach(processFile)

// Prefer: full Iterator API
listFiles.iterate().filter(isRelevant).take(10).map(processFile).collect()
```

### Use `withResource` for anything that needs cleanup

Git worktrees, temp directories, database connections — if it needs teardown regardless of success/failure, use `withResource` rather than manual try/finally logic inside a handler.

```ts
withResource({
  create: createBranchWorktree,
  action: implementAndReview,
  dispose: deleteWorktree,
})
```

The `dispose` step runs whether `action` succeeds or fails — guaranteed cleanup without polluting handler logic.

---

## Handler contracts

### Always provide validators

Always provide `inputValidator` and `outputValidator` on handlers even though they're optional. They serve as machine-checked documentation of the handler's contract and catch shape mismatches at runtime boundaries.

```ts
// Avoid: no validators — silent failures when shapes don't match
export const analyze = createHandler({
  handle: async ({ value }) => { ... },
}, "analyze");

// Prefer: validators document and enforce the contract
export const analyze = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.array(refactorSchema),
  handle: async ({ value }) => { ... },
}, "analyze");
```

### Namespace tagged union variants

When a handler returns a decision (e.g., "needs work" vs "approved"), namespace the variants in `taggedUnionSchema`. This prevents collisions when multiple branch points exist in the same pipeline and makes branch dispatch unambiguous.

```ts
// Handler returns a namespaced decision:
outputValidator: taggedUnionSchema("Judgment", {
  NeedsWork: feedbackSchema,
  Approved: z.null(),
})

// Branch dispatches on the short names:
classifyJudgment.branch({
  NeedsWork: applyFeedback.then(recur),
  Approved: drop,
})
```

### Use void returns for side-effect-only handlers

If a handler's purpose is a side effect (write a file, send a message, invoke an LLM with tools), return `void` from `handle`. The framework types it as `never` output — the next step starts fresh via `.drop()` or naturally from a new source. Don't return `null` and pass it along.

```ts
// Avoid: returning null as a meaningless value that gets threaded through
export const implement = createHandler({
  outputValidator: z.null(),
  handle: async ({ value }) => {
    await callClaude({ prompt: `Implement ${value.description}` });
    return null;
  },
}, "implement");

// Prefer: void return — framework knows there's no output
export const implement = createHandler({
  inputValidator: z.object({ description: z.string() }),
  handle: async ({ value }) => {
    await callClaude({ prompt: `Implement ${value.description}` });
  },
}, "implement");
```

---

## Prefer postfix methods over standalone functions

When a combinator is available as both a standalone function and a postfix method, **always prefer the postfix form.** Two reasons:

1. **No type parameters.** Standalone functions like `getField<TObj, TField>(field)` often require explicit generic arguments because TypeScript can't infer the input type without context. The postfix form `action.getField("name")` infers everything from the preceding action's output type — zero annotation needed.

2. **No wrapping in `pipe`.** Standalone functions used mid-pipeline need a `pipe(action, getField("name"))` wrapper. Postfix chains directly: `action.getField("name")`.

```ts
// Avoid: standalone requires type parameters and pipe wrapping
pipe(getUserProfile, getField<UserProfile, "email">("email"))

// Prefer: postfix infers types from context
getUserProfile.getField("email")
```

This applies to every combinator that has a postfix form: `.then()`, `.iterate()`, `.map()`, `.flatMap()`, `.filter()`, `.collect()`, `.branch()`, `.drop()`, `.tag()`, `.flatten()`, `.getField()`, `.getIndex()`, `.pick()`, `.wrapInField()`, `.splitFirst()`, `.splitLast()`, `.mapErr()`, `.unwrapOr()`.

## Prefer `.then()` over `pipe()`

Postfix `.then()` is the primary way to chain steps. It reads naturally and infers types from context:

```ts
// Avoid
pipe(listFiles, Iterator.fromArray(), Iterator.map(processFile), Iterator.collect(), commit)

// Prefer
listFiles.iterate().map(processFile).collect().then(commit)
```

`pipe()` is available as an alternative but rarely needed — `.then()` chains work at any length.

## Use `taggedUnionSchema` for handler validators

When a handler returns a tagged union, use `taggedUnionSchema()`, `Option.schema()`, or `Result.schema()` instead of hand-rolling `z.discriminatedUnion()`:

```ts
// Avoid
outputValidator: z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("HasErrors"), value: z.array(errorSchema) }),
  z.object({ kind: z.literal("Clean"), value: z.null() }),
])

// Prefer
outputValidator: taggedUnionSchema({
  HasErrors: z.array(errorSchema),
  Clean: z.null(),
})
```

For `Option` and `Result` specifically:

```ts
outputValidator: Option.schema(z.string())     // Option<string>
outputValidator: Result.schema(z.string(), z.number())  // Result<string, number>
```
