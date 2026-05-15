# Best Practices

Three principles drive every decision below:

1. **Use the builtins.** The framework provides typed combinators for structuring data (`wrapInField`, `getField`, `pick`, `allObject`). Use them instead of reimplementing the same logic inside handlers.
2. **Move logic into the pipeline.** Handlers do work — external calls, computation, side effects. Everything else (routing, merging, retrying, threading context) belongs in the pipeline definition where it's visible, composable, and reusable.
3. **Compose.** Small, focused handlers with narrow inputs and scalar outputs combine freely. A handler that returns a flat value works with `wrapInField`, `allObject`, `fold`, and every other combinator. A handler that returns a bespoke object works only with itself.

---

## Handler design

Handlers are the leaf nodes — they do work. Everything else is plumbing. Keep them minimal and let the pipeline layer handle composition.

### Handlers cannot call other handlers

Handlers run in isolated subprocesses. You cannot call `.handle()` from inside one handler to invoke another. All composition happens in the pipeline definition via combinators (`pipe`, `.then()`, `bindInput`, etc.). If you need the output of one handler as input to another, chain them in the pipeline.

### One job per handler

A handler does one thing: transform data, call an external service, read a file, invoke an LLM. All plumbing — splitting fields, merging objects, routing to different paths — belongs in the pipeline layer using `bindInput`, `getField`, `wrapInField`, `allObject`, `pick`, and `branch`.

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

### Retries, timeouts, and error recovery belong in the pipeline

A handler makes exactly one attempt and returns a `Result` on failure. Retries, timeouts, back-off, and fallback paths are all pipeline-level concerns — they compose around handlers via `loop`, `tryCatch`, `unwrapOr`, and `withTimeout`.

```ts
// Avoid: retry and timeout inside the handler
export const callApi = createHandler({
  handle: async ({ value }) => {
    for (let i = 0; i < 3; i++) {
      try { return await fetch(value.url, { signal: AbortSignal.timeout(5000) }); }
      catch { await sleep(1000 * i); }
    }
    throw new Error("failed after retries");
  },
}, "callApi");

// Prefer: handler does one attempt, pipeline handles retry and timeout
export const callApi = createHandler({
  outputValidator: Result.schema(responseSchema, z.string()),
  handle: async ({ value }) => {
    try { return { kind: "Result.Ok", value: await fetch(value.url) }; }
    catch (e) { return { kind: "Result.Err", value: e.message }; }
  },
}, "callApi");

// Pipeline adds timeout and retries:
loop((recur, done) =>
  withTimeout(constant(5_000), callApi)
    .branch({ Ok: done, Err: logAndWait.then(recur) })
)
```

This separation means you can reuse `callApi` in contexts that don't want retries, or change the retry strategy without touching the handler.

### Return scalar values, not wrapper objects

A handler that computes one thing should return that thing directly — not an object wrapping it. Wrapping is the pipeline's job (`wrapInField`, `allObject`). Scalar outputs compose with every combinator; bespoke objects only work in one pipeline.

```ts
// Avoid: handler wraps its result in an object
export const countLines = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.object({ lineCount: z.number() }),
  handle: async ({ value }) => {
    const content = readFileSync(value.file, "utf-8");
    return { lineCount: content.split("\n").length };
  },
}, "countLines");

// Prefer: return the value directly, let the pipeline structure it
export const countLines = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.number(),
  handle: async ({ value }) => {
    const content = readFileSync(value.file, "utf-8");
    return content.split("\n").length;
  },
}, "countLines");

// Pipeline wraps/merges as needed:
countLines.wrapInField("lineCount")    // → { lineCount: number }
allObject({ lines: countLines, size: getFileSize })  // combine multiple
```

### Don't return data the pipeline already knows

If the file path was passed in as input, don't make the handler echo it back. The pipeline can merge it back via `wrapInField`, `allObject`, or `bindInput`.

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

// Pipeline structures the result:
allObject({ file: identity(), lineCount: countLines })  // { file } → { file, lineCount }
```

### Don't accept pass-through fields in handler inputs

Handlers should only accept the fields they actually use. A handler that accepts data and returns it unchanged is an anti-pattern — it means the handler is doing the pipeline's job of threading context. The telltale sign is `return { ...value, result }` or any handler whose output is a superset of its input with one new field tacked on.

If a handler needs `file` but downstream steps also need `branch` and `worktreePath`, don't widen the handler's input to include all three. Use `bindInput` or `pick` in the pipeline to narrow the input before the handler and restore the full context after.

```ts
// Avoid: handler accepts fields it doesn't use, just to pass them through
export const analyze = createHandler({
  inputValidator: z.object({
    file: z.string(),
    branch: z.string(),        // not used by analyze
    worktreePath: z.string(),  // not used by analyze
  }),
  outputValidator: z.object({
    file: z.string(),          // echoed back unchanged
    branch: z.string(),        // echoed back unchanged
    worktreePath: z.string(),  // echoed back unchanged
    issues: z.array(issueSchema),
  }),
  handle: async ({ value }) => {
    const issues = await findIssues(value.file);
    return { ...value, issues };  // spreading input into output = anti-pattern
  },
}, "analyze");
```

The handler's signature is now coupled to its caller's context. It can't be reused in a pipeline that doesn't have `branch` or `worktreePath`. Instead, keep the handler's input narrow and let the pipeline manage context:

```ts
// Prefer: handler only accepts what it needs, returns only what it computed
export const analyze = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.array(issueSchema),
  handle: async ({ value }) => {
    return await findIssues(value.file);
  },
}, "analyze");

// Pipeline narrows input and restores context:
bindInput<{ file: string; branch: string; worktreePath: string }>((params) =>
  params.pick("file").then(analyze)
    .then(wrapInField("issues"))
    // params still has branch and worktreePath available for later steps
)

// Or use allObject to combine input fields with handler output:
allObject({ file: identity(), issues: analyze })
```

This keeps handlers reusable, testable in isolation, and decoupled from the specific pipeline they appear in. The pipeline is the right place for context management — handlers are the right place for doing work.

### Pass data through the pipeline, not the file system

The pipeline is the data channel between handlers. If handler A produces a result that handler B needs, return it from A and pass it to B through the pipeline — don't write it to a temp file and have B read it back.

```ts
// Avoid: using the file system as a data bus
export const generateReport = createHandler({
  handle: async ({ value }) => {
    const report = await analyze(value.file);
    writeFileSync("/tmp/report.json", JSON.stringify(report));
  },
}, "generateReport");

export const publishReport = createHandler({
  handle: async () => {
    const report = JSON.parse(readFileSync("/tmp/report.json", "utf-8"));
    await upload(report);
  },
}, "publishReport");

// Prefer: data flows through the pipeline
export const generateReport = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: reportSchema,
  handle: async ({ value }) => {
    return await analyze(value.file);
  },
}, "generateReport");

export const publishReport = createHandler({
  inputValidator: reportSchema,
  handle: async ({ value }) => {
    await upload(value);
  },
}, "publishReport");

// Pipeline connects them:
generateReport.then(publishReport)
```

File system writes are appropriate for **durable side effects** — checkpointing progress, writing final output artifacts, persisting state that survives process crashes. They are not appropriate for passing intermediate data between pipeline steps. Pipeline data is typed, validated, and visible to the framework for debugging and replay. File system state is opaque, fragile, and couples handlers to specific paths.

---

## Pipeline composition

### Prefer `allObject` over `all`

`all` returns a positional tuple — callers access results by index, which is fragile and unreadable. `allObject` returns a named object:

```ts
// Avoid: positional tuple — what's [0]? what's [1]?
all(listFiles, loadConfig, readManifest)
// → [string[], Config, Manifest]

// Prefer: named fields — self-documenting, refactor-safe
allObject({
  files: listFiles,
  config: loadConfig,
  manifest: readManifest,
})
// → { files: string[], config: Config, manifest: Manifest }
```

Named fields survive reordering and additions without breaking downstream `.getIndex()` calls. Use `all` only when feeding directly into something that expects a tuple (like `fold`'s `[acc, element]`).

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

### Use `allObject` to carry context forward

When you need both a handler's input and output downstream, use `allObject` to run the handler alongside `identity()` and collect the results into a named object.

```ts
// { file: string } → { file: string, lineCount: number }
listFiles.iterate().map(allObject({ file: getField("file"), lineCount: countLines })).collect()
```

### Iteration is parallel by default

`.iterate().map(action).collect()` dispatches all elements concurrently — like `Promise.all`, not a for-loop. If you need sequential processing (e.g., each step depends on the previous result, or you're rate-limited), use `.fold()`:

```ts
// Parallel: all files processed concurrently
listFiles.iterate().map(processFile).collect()

// Sequential: one at a time, with accumulator
listFiles.iterate().fold(constant(initialState), processFileSequentially)
```

There is no sequential `.each()` or sequential `.map()`. If you want one-at-a-time execution, fold is the primitive.

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

### Annotate return types when returning tagged unions

When a handler returns a tagged union but only constructs one variant in a given code path, TypeScript narrows the return type to that single variant. The pipeline then fails to typecheck because the handler's output type is narrower than the full union expected by `.branch()` or `.unwrapOr()`.

Fix: add an explicit `Promise<FullUnionType>` return type annotation to the `handle` function.

```ts
type AnalysisResult = Result<string, string>;

// Avoid: TypeScript narrows to just the Ok variant
handle: async ({ value }) => {
  return { kind: "Result.Ok" as const, value: "done" };
  // Inferred return: { kind: "Result.Ok", value: string } — not Result<string, string>
}

// Prefer: explicit annotation preserves the full union
handle: async ({ value }): Promise<AnalysisResult> => {
  return { kind: "Result.Ok" as const, value: "done" };
}
```

### Use `null` not `undefined` for empty pipeline values

Pipeline definitions are serialized to JSON. `undefined` has no JSON representation — `JSON.stringify({ value: undefined })` produces `{}`, which causes the Rust deserializer to fail with a missing field error. TypeScript won't catch this because `void` accepts `undefined` as a valid value.

Use `null` for "no meaningful value" in pipeline data:

```ts
// Broken: undefined disappears during serialization
Skip: bindInput<null, void>(() => constant(undefined))

// Fixed: null serializes correctly
Skip: bindInput<null, null>(() => constant(null))
```

This applies anywhere you construct pipeline values — `constant()`, handler return values, branch cases. If you mean "nothing," use `null`.

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

## Minimize work inside LLM handlers

When a handler invokes an LLM agent (e.g., `callClaude`), the agent's effectiveness is bounded by the context it receives. Pre-read files in earlier pipeline steps and pass the content as input — don't make the agent spend tokens discovering information you already have.

### Pre-read the file being modified

If the agent's job is to modify a file, read it before the handler runs and pass the content in. The agent sees the full file immediately instead of burning a tool call to read it.

```ts
// Avoid: agent wastes a tool call reading the file
export const refactor = createHandler({
  inputValidator: z.object({ file: z.string() }),
  handle: async ({ value }) => {
    await callClaude({
      prompt: `Refactor ${value.file}`,
      allowedTools: ["Read", "Edit"],
    });
  },
}, "refactor");

// Prefer: pre-read in the pipeline, agent starts with full context
export const readFile = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.object({ file: z.string(), content: z.string() }),
  handle: async ({ value }) => ({
    file: value.file,
    content: readFileSync(value.file, "utf-8"),
  }),
}, "readFile");

export const refactor = createHandler({
  inputValidator: z.object({ file: z.string(), content: z.string() }),
  handle: async ({ value }) => {
    await callClaude({
      prompt: `Refactor this file (${value.file}):\n\n${value.content}`,
      allowedTools: ["Edit"],
    });
  },
}, "refactor");
```

### Pre-read imports and dependents

An agent modifying a file needs to understand its dependencies and its callers. Read these in the pipeline and include them:

- **Files it imports** — so the agent knows the shape of dependencies without guessing.
- **Files that import it** — so the agent understands downstream callers and avoids breaking changes.

```ts
// Pipeline reads context, agent receives it pre-loaded:
readTargetFile.bindInput((fileAndContent) =>
  allObject({
    file: fileAndContent.getField("file"),
    content: fileAndContent.getField("content"),
    imports: fileAndContent.then(resolveImports),
    dependents: fileAndContent.then(findDependents),
  }).then(refactorWithContext)
)
```

### Why this matters

Every tool call an LLM agent makes costs latency and tokens. A `Read` call the agent makes inside a handler is identical work the pipeline could have done deterministically in milliseconds. The agent should spend its budget on judgment and creativity — deciding *what* to change — not on mechanically gathering files it was always going to need.

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

## `.then()` vs `pipe()` for chaining

Use `.then()` for two-step chains. Use `pipe()` when chaining three or more steps in sequence — it's flatter and easier to scan:

```ts
// Two steps: .then() is fine
listFiles.then(commit)

// Three+ steps: use pipe()
pipe(
  listFiles,
  processFiles,
  commit,
)

// Avoid: long .then() chains
listFiles.then(processFiles).then(validate).then(commit)

// Avoid: mixing pipe and .then()
pipe(listFiles, processFiles).then(commit)
```

Postfix methods like `.iterate()`, `.map()`, `.collect()`, `.getField()`, `.branch()` are always preferred over their standalone equivalents regardless of chain length — they infer types from context and don't require explicit type parameters.

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
