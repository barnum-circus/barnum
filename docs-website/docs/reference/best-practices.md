# Best Practices

Three principles drive every decision below:

1. **Use the builtins.** The framework provides typed combinators for structuring data (`wrapInField`, `getField`, `pick`, `allObject`). Use them instead of reimplementing the same logic inside handlers.
2. **Move logic into the pipeline.** Handlers do work — external calls, computation, side effects. Everything else (routing, merging, retrying, threading context) belongs in the pipeline definition where it's visible, composable, and reusable. It's tempting to put "just a little extra logic" in a handler — a retry loop, an early bail-out, a conditional branch. But that logic grows, and once it's buried in a handler it's invisible to the framework: you can't change the retry strategy without editing the handler, you can't reuse the handler without its hardcoded control flow, and you can't see the workflow's structure by reading the pipeline. Keep handlers dumb and the pipeline smart.
3. **Compose.** Small, focused handlers with narrow inputs and scalar outputs combine freely. A handler that returns a flat value works with `wrapInField`, `allObject`, `fold`, and every other combinator. A handler that returns a bespoke object works only with itself.

---

## Handler design

Handlers are the leaf nodes — they do work. Everything else is plumbing. Keep them minimal and let the pipeline layer handle composition.

### Handlers must be exported, and the export name must match the string identifier

The framework resolves handlers by importing their module and looking up the named export matching the string identifier (the second argument to `createHandler`). The worker does `mod[exportName]` — if the export is missing or isn't a handler, it fails with `worker: <module>:<export> is not a barnum handler` and the workflow terminates.

```ts
// Broken: not exported
const analyze = createHandler({ ... }, "analyze");

// Broken: export name doesn't match identifier
export const analyzeFile = createHandler({ ... }, "analyze");

// Correct: exported and names match
export const analyze = createHandler({ ... }, "analyze");
```

### Handlers cannot call other handlers

Handlers run in isolated subprocesses. You cannot call `.handle()` from inside one handler to invoke another. All composition happens in the pipeline definition via combinators (`pipe`, `.then()`, `bindInput`, etc.). If you need the output of one handler as input to another, chain them in the pipeline.

### Do not call `.run()` from inside a handler

A handler should never spawn a nested pipeline via `.run()` (or `.compile().run()`). If you think you need to, you're wrong — restructure the pipeline instead. The framework owns orchestration; handlers do work. A nested `.run()` bypasses scheduling, resource management, and error propagation. Whatever you're trying to express as a nested pipeline is expressible as pipeline-level composition (`pipe`, `withResource`, `loop`, `bindInput`).

### `createHandler` must be called at module top-level

`createHandler` uses V8 stack introspection to locate the handler's source module at definition time. If you define a handler inside a function, class method, or dynamic scope, the stack trace doesn't resolve to the correct module path and the handler fails to load at runtime.

```ts
// Broken: handler defined inside a function — stack introspection fails
function makeHandlers() {
  return {
    analyze: createHandler({ ... }, "analyze"),
  };
}

// Broken: handler defined inside a conditional
if (process.env.MODE === "production") {
  export const analyze = createHandler({ ... }, "analyze");
}

// Correct: top-level module scope
export const analyze = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.array(issueSchema),
  handle: async ({ value }) => { ... },
}, "analyze");
```

### Define handlers in separate files from the pipeline `.run()` call

**This will fork bomb your machine.** The framework executes handlers by importing their module in a subprocess. If that module also contains a top-level `.run()` call, importing the handler re-triggers the entire pipeline — which invokes handlers — which imports the module — which triggers the pipeline again. Each invocation spawns subprocesses exponentially until the system runs out of file descriptors or memory.

Never define handlers in the same file that calls `.run()`.

```ts
// run.ts — ONLY the pipeline definition and the .run() call
import { pipe } from "@barnum/barnum/pipeline";
import { analyze } from "./handlers/analyze";
pipe(constant({ file: "main.ts" }), analyze).run();

// handlers/analyze.ts — ONLY the handler definition
import { createHandler } from "@barnum/barnum/runtime";
export const analyze = createHandler({ ... }, "analyze");
```

### All data flows through the pipeline

Each handler invocation runs in its own subprocess. Global variables, module-level caches, and in-memory state do not persist between handler calls — mutating a global inside one handler is invisible to every other handler. The pipeline is the only data channel between steps.

```ts
// Broken: global state doesn't survive across subprocess boundaries
let lastResult: string | null = null;

export const step1 = createHandler(
  {
    handle: async ({ value }) => {
      lastResult = await compute(value); // this dies with the subprocess
    },
  },
  "step1",
);

export const step2 = createHandler(
  {
    handle: async () => {
      return lastResult; // always null — different subprocess
    },
  },
  "step2",
);

// Fixed: return the value and let the pipeline carry it
export const step1 = createHandler(
  {
    inputValidator: z.object({ input: z.string() }),
    outputValidator: z.string(),
    handle: async ({ value }) => {
      return await compute(value.input);
    },
  },
  "step1",
);

// Pipeline connects them:
step1.then(step2);
```

### One job per handler

A handler does one thing: transform data, call an external service, read a file, invoke an LLM. All plumbing — splitting fields, merging objects, routing to different paths — belongs in the pipeline layer using `bindInput`, `getField`, `wrapInField`, `allObject`, `pick`, and `branch`.

```ts
// Avoid: handler does plumbing + work
export const analyzeAndRoute = createHandler({
  handle: async ({ value }) => {
    const result = await invokeLanguageModel({ prompt: `Analyze ${value.file}` });
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
    return await invokeLanguageModel({ prompt: `Analyze ${value.file}` });
  },
}, "analyze");

// Pipeline routes and merges:
analyze.branch({ Critical: escalate, Low: log })
```

### Use `z.any()` for handlers that ignore their input

A handler that doesn't use its input is a thunk — like a `VarRef` or `constant`, it produces a value regardless of what arrives. Use `z.any()` (or omit `inputValidator`) so it composes freely in any pipeline position without requiring `.drop()` at call sites.

```ts
// Correct: handler ignores input, uses z.any() — composes like a VarRef
export const judgeRefactor = createHandler(
  {
    inputValidator: z.any(),
    outputValidator: JudgmentResultValidator,
    handle: async (): Promise<JudgmentResult> => {
      // doesn't use input — it's a thunk
      return await reviewChanges();
    },
  },
  "judgeRefactor",
);

// Can appear anywhere without .drop() ceremony:
someAction.then(judgeRefactor);  // works — input ignored
```

`z.null()` would force every caller to explicitly discard before calling, which is inconsistent with how VarRefs and constants behave. "Ignores its input" means "accepts anything" — narrowness applies to inputs the handler *uses*.

### Don't handle errors inside handlers

A handler that can fail should return a `Result` — not catch the error internally and swallow it. Error handling is a pipeline concern: the pipeline decides whether to retry, fall back, escalate, or abort. A handler that silently swallows errors (returning a default, logging and continuing, or catching and ignoring) hides failures from the framework and makes the workflow lie about its state.

```ts
// Avoid: handler swallows the error and returns a fallback
export const parseConfig = createHandler(
  {
    inputValidator: z.object({ path: z.string() }),
    outputValidator: configSchema,
    handle: async ({ value }) => {
      try {
        return JSON.parse(readFileSync(value.path, "utf-8"));
      } catch {
        console.error("Failed to parse, using defaults");
        return DEFAULT_CONFIG; // silent failure — pipeline never knows
      }
    },
  },
  "parseConfig",
);

// Prefer: handler surfaces the error, pipeline decides what to do
export const parseConfig = createHandler(
  {
    inputValidator: z.object({ path: z.string() }),
    outputValidator: Result.schema(configSchema, z.string()),
    handle: async ({ value }) => {
      try {
        return { kind: "Result.Ok", value: JSON.parse(readFileSync(value.path, "utf-8")) };
      } catch (e) {
        return { kind: "Result.Err", value: e.message };
      }
    },
  },
  "parseConfig",
);

// Pipeline handles the error explicitly:
parseConfig.unwrapOr(useDefaults);
// or: parseConfig.branch({ Ok: proceed, Err: escalate });
```

If an error is truly unrecoverable, let the handler throw — the framework will surface it. If it's recoverable, return `Result` and let the pipeline choose the recovery strategy. The one thing a handler should never do is hide that something went wrong.

### Retries, timeouts, and error recovery belong in the pipeline

A handler makes exactly one attempt and returns a `Result` on failure. Retries, timeouts, back-off, and fallback paths are all pipeline-level concerns — they compose around handlers via `loop`, `tryCatch`, `unwrapOr`, and `withTimeout`.

```ts
// Avoid: retry and timeout inside the handler
export const callApi = createHandler(
  {
    handle: async ({ value }) => {
      for (let i = 0; i < 3; i++) {
        try {
          return await fetch(value.url, { signal: AbortSignal.timeout(5000) });
        } catch {
          await sleep(1000 * i);
        }
      }
      throw new Error("failed after retries");
    },
  },
  "callApi",
);

// Prefer: handler does one attempt, pipeline handles retry and timeout
export const callApi = createHandler(
  {
    outputValidator: Result.schema(responseSchema, z.string()),
    handle: async ({ value }) => {
      try {
        return { kind: "Result.Ok", value: await fetch(value.url) };
      } catch (e) {
        return { kind: "Result.Err", value: e.message };
      }
    },
  },
  "callApi",
);

// Pipeline adds timeout and retries:
withRetry(withTimeout(constant(5_000), callApi), 3);
```

`withRetry(action, n)` retries a `Result`-returning action up to `n` times. On the first `Ok`, it short-circuits and returns the value. If all attempts produce `Err`, it returns the final `Err`. The action receives the same input on every attempt.

For more complex retry logic (back-off, logging between attempts, conditional retry based on error type), use `loop` directly:

```ts
loop((recur, done) =>
  withTimeout(constant(5_000), callApi).branch({
    Ok: done,
    Err: logAndWait.then(recur),
  }),
);
```

This separation means you can reuse `callApi` in contexts that don't want retries, or change the retry strategy without touching the handler.

### Return scalar values, not wrapper objects

A handler that computes one thing should return that thing directly — not an object wrapping it. Wrapping is the pipeline's job (`wrapInField`, `allObject`). Scalar outputs compose with every combinator; bespoke objects only work in one pipeline.

```ts
// Avoid: handler wraps its result in an object
export const countLines = createHandler(
  {
    inputValidator: z.object({ file: z.string() }),
    outputValidator: z.object({ lineCount: z.number() }),
    handle: async ({ value }) => {
      const content = readFileSync(value.file, "utf-8");
      return { lineCount: content.split("\n").length };
    },
  },
  "countLines",
);

// Prefer: return the value directly, let the pipeline structure it
export const countLines = createHandler(
  {
    inputValidator: z.object({ file: z.string() }),
    outputValidator: z.number(),
    handle: async ({ value }) => {
      const content = readFileSync(value.file, "utf-8");
      return content.split("\n").length;
    },
  },
  "countLines",
);

// Pipeline wraps/merges as needed:
countLines.wrapInField("lineCount"); // → { lineCount: number }
allObject({ lines: countLines, size: getFileSize }); // combine multiple
```

### Don't return data the pipeline already knows

If the file path was passed in as input, don't make the handler echo it back. The pipeline can merge it back via `wrapInField`, `allObject`, or `bindInput`.

```ts
// Avoid: handler parrots its input back
export const countLines = createHandler(
  {
    handle: async ({ value }) => {
      const content = readFileSync(value.file, "utf-8");
      return { file: value.file, lineCount: content.split("\n").length };
      //       ^^^^^^^^^^^^^^^ pipeline already has this
    },
  },
  "countLines",
);

// Prefer: handler returns only what it computed
export const countLines = createHandler(
  {
    inputValidator: z.object({ file: z.string() }),
    outputValidator: z.number(),
    handle: async ({ value }) => {
      const content = readFileSync(value.file, "utf-8");
      return content.split("\n").length;
    },
  },
  "countLines",
);

// Pipeline structures the result:
allObject({ file: identity(), lineCount: countLines }); // { file } → { file, lineCount }
```

### Don't accept pass-through fields in handler inputs

Handlers should only accept the fields they actually use. A handler that accepts data and returns it unchanged is an anti-pattern — it means the handler is doing the pipeline's job of threading context. The telltale sign is `return { ...value, result }` or any handler whose output is a superset of its input with one new field tacked on.

If a handler needs `file` but downstream steps also need `branch` and `worktreePath`, don't widen the handler's input to include all three. Use `bindInput` or `pick` in the pipeline to narrow the input before the handler and restore the full context after.

```ts
// Avoid: handler accepts fields it doesn't use, just to pass them through
export const analyze = createHandler(
  {
    inputValidator: z.object({
      file: z.string(),
      branch: z.string(), // not used by analyze
      worktreePath: z.string(), // not used by analyze
    }),
    outputValidator: z.object({
      file: z.string(), // echoed back unchanged
      branch: z.string(), // echoed back unchanged
      worktreePath: z.string(), // echoed back unchanged
      issues: z.array(issueSchema),
    }),
    handle: async ({ value }) => {
      const issues = await findIssues(value.file);
      return { ...value, issues }; // spreading input into output = anti-pattern
    },
  },
  "analyze",
);
```

The handler's signature is now coupled to its caller's context. It can't be reused in a pipeline that doesn't have `branch` or `worktreePath`. Instead, keep the handler's input narrow and let the pipeline manage context:

```ts
// Prefer: handler only accepts what it needs, returns only what it computed
export const analyze = createHandler(
  {
    inputValidator: z.object({ file: z.string() }),
    outputValidator: z.array(issueSchema),
    handle: async ({ value }) => {
      return await findIssues(value.file);
    },
  },
  "analyze",
);

// Pipeline narrows input and restores context:
bindInput<{ file: string; branch: string; worktreePath: string }>(
  (params) => params.pick("file").then(analyze).then(wrapInField("issues")),
  // params still has branch and worktreePath available for later steps
);

// Or use allObject to combine input fields with handler output:
allObject({ file: identity(), issues: analyze });
```

This keeps handlers reusable, testable in isolation, and decoupled from the specific pipeline they appear in. The pipeline is the right place for context management — handlers are the right place for doing work.

### Pass data through the pipeline, not the file system

The pipeline is the data channel between handlers. If handler A produces a result that handler B needs, return it from A and pass it to B through the pipeline — don't write it to a temp file and have B read it back.

```ts
// Avoid: using the file system as a data bus
export const generateReport = createHandler(
  {
    handle: async ({ value }) => {
      const report = await analyze(value.file);
      writeFileSync("/tmp/report.json", JSON.stringify(report));
    },
  },
  "generateReport",
);

export const publishReport = createHandler(
  {
    handle: async () => {
      const report = JSON.parse(readFileSync("/tmp/report.json", "utf-8"));
      await upload(report);
    },
  },
  "publishReport",
);

// Prefer: data flows through the pipeline
export const generateReport = createHandler(
  {
    inputValidator: z.object({ file: z.string() }),
    outputValidator: reportSchema,
    handle: async ({ value }) => {
      return await analyze(value.file);
    },
  },
  "generateReport",
);

export const publishReport = createHandler(
  {
    inputValidator: reportSchema,
    handle: async ({ value }) => {
      await upload(value);
    },
  },
  "publishReport",
);

// Pipeline connects them:
generateReport.then(publishReport);
```

File system writes are appropriate for **durable side effects** — checkpointing progress, writing final output artifacts, persisting state that survives process crashes. They are not appropriate for passing intermediate data between pipeline steps. Pipeline data is typed, validated, and visible to the framework for debugging and replay. File system state is opaque, fragile, and couples handlers to specific paths.

---

## Pipeline composition

### Always annotate type parameters on `loop`, `earlyReturn`, `bindInput`, and `defineRecursiveFunctions`

These combinators cannot infer their type parameters from usage. If you omit them, the output type silently degrades to `any` and every downstream step loses type checking.

**Why TypeScript can't infer these.** TypeScript infers generic type parameters from _arguments_ (outside-in). But these combinators' type parameters only manifest as the _types of callback parameters_ — the `recur`/`done` actions in `loop`, the `ret` action in `earlyReturn`, the `VarRef` in `bindInput`. TypeScript doesn't do bidirectional inference from "how the callback body uses its parameters" back to "what the enclosing function's type parameters must be." The type parameter determines what gets _passed into_ the callback, not what gets _returned from_ it — so TS has nothing to infer from.

```ts
// loop<TIn, TOut>: TIn determines recur's type, TOut determines done's type
// TS can't look at "done is used in the Empty branch" and work backwards

// Broken: type params omitted — entire pipeline is untyped from here on
const process = loop((recur, done) =>
  fetchNext.branch({
    HasItem: pipe(handle, recur),
    Empty: done,
  }),
);
// process: TypedAction<any, any> — no type safety

// Fixed: annotate TIn and TOut explicitly
const process = loop<void, ProcessResult>((recur, done) =>
  fetchNext.branch({
    HasItem: pipe(handle, recur),
    Empty: done,
  }),
);
// process: TypedAction<null, ProcessResult> — fully typed
```

The same applies to `earlyReturn`:

```ts
// earlyReturn: TEarlyReturn determines ret's type
// TS can't infer it from "ret is passed to .unwrapOr()"

// Broken: TEarlyReturn is any
earlyReturn((ret) => step1.unwrapOr(ret).then(step2));

// Fixed
earlyReturn<ErrorReport>((ret) => step1.unwrapOr(ret).then(step2));
```

And `defineRecursiveFunctions` — every function's input and output types must be annotated in the definition tuple, or all call sites produce `any`.

### `bindInput`: always specify both `TIn` and `TOut`

`bindInput<TIn, TOut>` requires both type parameters (there are no defaults). TypeScript can't infer either from callback usage — the lint rule `barnum/require-type-params` enforces this.

```ts
// Both TIn and TOut must be specified
bindInput<{ artifact: string }, { verified: boolean }>((input) =>
  input.then(verify),
);
```

**Inside loop/recursion bodies:** When `bindInput` is used inside `loop` where every branch ends in `recur` or `done` (both typed as `TypedAction<..., never>`), `TOut` must be `never`:

```ts
// Inside a loop body: TOut must be `never` because all paths end in recur/done
loop<State, Result>((recur, done) =>
  bindInput<State, never>((state) => {
    const { batch } = state.split();
    return batch.iterate().map(process).collect().branch({
      Continue: recur,
      Break: done,
    });
  }),
);
```

Without the explicit `never`, you get: `Type '() => any' is not assignable to type '() => never'`.

**Summary of when `TOut` is `never`:**

| Context                                      | `TOut`        | Why                                        |
| -------------------------------------------- | ------------- | ------------------------------------------ |
| Normal pipeline position                     | Concrete type | The body produces a value                  |
| Inside `loop` body (all paths → recur/done)  | `never`       | All paths exit via actions typed `→ never` |
| Inside `earlyReturn` where all paths → `ret` | `never`       | Same reason                                |

### Write Barnum like you write regular JavaScript

Barnum pipelines are just expressions. Apply the same judgment about when to extract a local variable as you would in normal code — if an expression is meaningful on its own, name it; if it's just argument assembly, inline it.

```ts
// Good: inline argument assembly, extract meaningful steps
bindInput<[WorktreeResource, Refactor], { prUrl: string }>((state) => {
  const [resource, refactor] = state.split();
  const worktreePath = resource.getField("worktreePath");
  const description = refactor.getField("description");

  const implemented = implement
    .call(allObject({ worktreePath, description }))
    .drop();
  const typeChecked = typeCheckFix
    .call(resource.pick("worktreePath"))
    .drop()
    .call(implemented);
  const committed = commit
    .call(resource.pick("worktreePath"))
    .drop()
    .call(typeChecked);
  return createPR
    .call(
      preparePRInput.call(
        allObject({ branch: resource.getField("branch"), description }),
      ),
    )
    .call(committed);
});
```

Notice that `allObject(...)` is inlined — you wouldn't write `const args = { name, age }; return createUser(args);` in JS, so don't write `const combined = allObject({ a, b }); return action.call(combined);` in Barnum. Inline argument assembly, extract meaningful computation.

```ts
// Avoid: extracting trivial argument assembly
const combined = allObject({ batchResults, state });
return advanceOrFinish.call(combined);

// Prefer: inline it — the verb and its arguments belong together
return advanceOrFinish.call(allObject({ batchResults, state }));
```

**`.call(x)` means data flow, never sequencing.** `action.call(x)` means "pass x's output as input to action." It must never be used to mean "x happens first" when the output is ignored. If you need ordering without data flow, use `pipe`.

```ts
// BAD: .call() used for sequencing — resetQueues output is ignored
all(consumer1, consumer2).call(resetQueues(dir))

// GOOD: pipe expresses ordering, .call() expresses data flow
pipe(resetQueues(dir).drop(), all(consumer1, consumer2))
```

**`pipe` for sequences:** `pipe` is natural when you have a sequence of steps — whether data flows between them or they're just ordered side effects. Each step occupies its own line and the sequence reads top-to-bottom:

```ts
// Data flows linearly through each step
listFiles
  .iterate()
  .map(pipe(implementRefactor, typeCheckFiles, fixTypeErrors, commitChanges));

// Steps don't share data — pipe expresses ordering
pipe(foo.drop(), bar.call(varRef));
```

**Prefer `const` + `.call()` when:**

- A step assembles its input from multiple sources (`allObject`, VarRefs)
- An intermediate result is meaningful enough to name
- The pipeline involves branching, loops, or error handling

**Non-caching note:** `.call()` results are not memoized. Each reference to a local variable re-evaluates its entire pipeline from scratch. If you reference the same `const` in two downstream actions, the handler runs twice:

```ts
// BUG: expensiveComputation runs TWICE — once for each reference to result
const result = expensiveComputation.call(input);
return allObject({ summary: summarize.call(result), report: format.call(result) });

// Fixed: bindInput caches the value as a VarRef — computation runs once
return expensiveComputation.call(input).bindInput<{ summary: string; report: string }>((result) =>
  allObject({ summary: summarize.call(result), report: format.call(result) }),
);
```

For single-use intermediates (the common case), local `const` bindings are pure win with no overhead. Only reach for `bindInput` when the same value is consumed by multiple downstream actions.

### Don't use `constant()` to produce a value the pipeline already carries

`constant(x)` discards its input and produces `x`. If the input is already `x`, `constant(x)` is a no-op — it throws away a value only to reconstruct the identical value. This most commonly appears at the top of a `loop` body:

```ts
// Avoid: loop<null, null> already passes null as input to the body
loop<null, null>((recur, done) =>
  dequeueEvent.call(constant(null)).branch({ ... }),
);

// Prefer: dequeueEvent accepts null, which is already the loop's input
loop<null, null>((recur, done) =>
  dequeueEvent.branch({ ... }),
);
```

`loop<TIn, TOut>` passes `TIn` as the body's input on every iteration. If the loop's input type is `null` and the first handler accepts `null`, just use the handler directly — the pipeline already provides the correct value.

`constant()` is appropriate when you need to introduce a value that doesn't exist in the current pipeline context — e.g., `constant({ producerId })` to inject a captured closure variable.

### Use `.drop()` instead of `constant(null)` to discard a value

When you need to discard the current pipeline value and produce `null`, use the postfix `.drop()` method — not `constant(null)`. Both are `any → null`, but `.drop()` communicates intent: "I'm done with this value." `constant(null)` reads as "I'm introducing a null," which obscures that a discard is happening.

```ts
// Avoid: constant(null) hides the discard
processEvent.then(constant(null)).then(dequeueEvent);

// Prefer: .drop() postfix — reads as "process, discard result, then dequeue"
processEvent.drop().then(dequeueEvent);
```

The freestanding `drop` is for positions where there's no preceding action to call `.drop()` on — e.g., as the body of a branch case that discards its input:

```ts
classify.branch({
  Relevant: processItem,
  Irrelevant: drop,
});
```

### Prefer `allObject` over `all`

`all` returns a positional tuple — callers access results by index, which is fragile and unreadable. `allObject` returns a named object:

```ts
// Avoid: positional tuple — what's [0]? what's [1]?
all(listFiles, loadConfig, readManifest);
// → [string[], Config, Manifest]

// Prefer: named fields — self-documenting, refactor-safe
allObject({
  files: listFiles,
  config: loadConfig,
  manifest: readManifest,
});
// → { files: string[], config: Config, manifest: Manifest }
```

Named fields survive reordering and additions without breaking downstream `.getIndex()` calls. Use `all` only when feeding directly into something that expects a tuple (like `fold`'s `[acc, element]`).

### `bindInput` captures the input as a `VarRef` — the body starts fresh

`bindInput<TIn>(fn)` captures the current pipeline value as a `VarRef<TIn>` and passes it to the callback. **The body's pipeline input is `any` — it does NOT receive the captured value as its natural input.** You must explicitly inject values using the VarRef's methods (`.getField()`, `.pick()`, or using the ref directly as the first step in a `pipe`).

A `VarRef<T>` is just a `TypedAction<any, T>` — a pipeline step that always produces the captured value regardless of what's flowing through the pipeline. `constant(x)` creates one too. There's nothing magical about VarRefs — they're actions you can use anywhere in the body to "reach back" to the captured input.

```ts
// The body doesn't implicitly receive `params` as input — you must use the VarRef:
bindInput<Params>((params) =>
  pipe(
    params.pick("file"), // ← explicitly inject from the captured value
    analyze,
    params.pick("branch"), // ← reach back to captured value mid-pipeline
    commit,
  ),
);
```

### Prefer postfix `.bindInput()` over `bindInput(...).call()`

`bindInput` is available both as a standalone function and as a postfix method on `TypedAction`. When you already have the value (as a VarRef or action), use the postfix form — it reads naturally and avoids the awkward `.call()` at the end.

```ts
// Avoid: standalone bindInput with .call() — reads inside-out
bindInput<WorktreeResource, never>((resource) =>
  pipe(
    implement.call(resource.pick("worktreePath")).drop(),
    commit.call(resource.pick("worktreePath")),
  ),
).call(createWorktree)

// Prefer: postfix .bindInput() — reads left-to-right
createWorktree.bindInput<never>((resource) =>
  pipe(
    implement.call(resource.pick("worktreePath")).drop(),
    commit.call(resource.pick("worktreePath")),
  ),
)
```

The standalone `bindInput<TIn, TOut>(fn)` is for the top level of a pipeline definition (where there's no preceding action to call it on). Everywhere else, use the postfix form.

### Use `bindInput` when multiple steps need the same value

If a handler's output is consumed by one step but also needed later (e.g., a worktree path used for type-check, commit, and PR creation), wrap the section in `bindInput` rather than threading the value through every handler's input/output.

```ts
// Avoid: every handler accepts and returns worktreePath
implement.then(typeCheck).then(commit).then(createPR);
// Each handler must include worktreePath in its input AND output — coupling city

// Prefer: bindInput captures the shared context
bindInput<Params>((params) =>
  params
    .pick("worktreePath", "description")
    .then(implement)
    .drop()
    .then(params.pick("worktreePath").then(typeCheckFix).drop())
    .then(params.pick("worktreePath").then(commit).drop())
    .then(params.pick("branch", "description").then(createPR)),
);
```

### Don't use `bindInput` if you never reference the bound variable

`bindInput` captures the pipeline input as a reusable variable. If the body never references that variable, the `bindInput` is doing nothing — remove it.

```ts
// Pointless: param is never used
bindInput<{ file: string }>((param) => constant("hello").then(processFile));

// Just write the pipeline directly:
constant("hello").then(processFile);
```

Similarly, if the first thing the body does is feed the bound variable into the next step, you're just recreating the pipeline's natural data flow. `bindInput` is only useful when you need the input _again later_ — not when you're using it exactly once as the starting point.

```ts
// Pointless: param is used once, immediately, as input to the next step
bindInput<{ file: string }>((param) => param.then(analyze).then(report));

// The pipeline already passes its input forward:
analyze.then(report);
```

### Use `.split()` to destructure tuples and objects inside `bindInput`

When a `VarRef` holds a tuple or object and you need its individual components as separate pipeline values, call `.split()`. It returns component VarRefs via JavaScript destructuring — tuple positions for arrays, named properties for objects.

```ts
// Tuple destructuring: fold body receives [TAcc, TElement]
Iterator.fold(
  constant(0),
  bindInput<[number, Item]>((state) => {
    const [acc, item] = state.split();
    return item.then(getScore).then(add(acc));
  }),
);

// Object destructuring: withResource action receives [TResource, TIn]
withResource({
  create: createDb,
  action: bindInput<[Db, Config]>((state) => {
    const [db, config] = state.split();
    return db.then(query(config));
  }),
  dispose: closeDb,
});
```

Each destructured component is a full `VarRef` — a `TypedAction<any, T>` that produces its value regardless of the current pipeline context. You can use them anywhere in the body: as the start of a `pipe`, as an argument to `all`, or chained with `.then()`.

### Don't use `.split()` when you only need one component

If you only need one field from the tuple/object, use `.getIndex()` or `.getField()` directly on the VarRef. `.split()` is for when you need multiple components.

```ts
// Avoid: splitting just to use one element
bindInput<[string, number]>((state) => {
  const [name, _age] = state.split();
  return name.then(greet);
});

// Prefer: access the index directly
bindInput<[string, number]>((state) => {
  return state.getIndex(0).unwrap().then(greet);
});
```

### `.split()` works on any structured VarRef — not just tuples

Object VarRefs support named destructuring:

```ts
bindInput<{ host: string; port: number }>((config) => {
  const { host, port } = config.split();
  return host.then(connect(port));
});
```

But prefer `.getField()` for single-field access — same reasoning as above.

### Use `allObject` to carry context forward

When you need both a handler's input and output downstream, use `allObject` to run the handler alongside `identity()` and collect the results into a named object.

```ts
// { file: string } → { file: string, lineCount: number }
listFiles
  .iterate()
  .map(allObject({ file: getField("file"), lineCount: countLines }))
  .collect();
```

### Iteration is parallel by default

`.iterate().map(action).collect()` dispatches all elements concurrently — like `Promise.all`, not a for-loop. If you need sequential processing (e.g., each step depends on the previous result, or you're rate-limited), use `.fold()`:

```ts
// Parallel: all files processed concurrently
listFiles.iterate().map(processFile).collect();

// Sequential: one at a time, with accumulator
listFiles.iterate().fold(constant(initialState), processFileSequentially);
```

There is no sequential `.each()` or sequential `.map()`. If you want one-at-a-time execution, fold is the primitive.

### Bounded concurrency: process N items at a time

`.iterate().map()` runs ALL elements concurrently. For bounded concurrency (max N in flight at a time), see the [Bounded Concurrency pattern](../patterns/bounded-concurrency.md).

### Prefer `.iterate().map()` over `forEach`

`forEach` is a low-level AST node. The Iterator API is the user-facing equivalent with better composability — you can chain `.filter()`, `.take()`, `.flatMap()` before collecting.

```ts
// Avoid: raw forEach, no ability to filter/take/transform
forEach(processFile);

// Prefer: full Iterator API
listFiles.iterate().filter(isRelevant).take(10).map(processFile).collect();
```

### Use `withResource` for anything that needs cleanup

Git worktrees, temp directories, database connections — if it needs teardown regardless of success/failure, use `withResource` rather than manual try/finally logic inside a handler.

```ts
withResource({
  create: createBranchWorktree,
  action: implementAndReview,
  dispose: deleteWorktree,
});
```

The `dispose` step runs whether `action` succeeds or fails — guaranteed cleanup without polluting handler logic.

### Use `earlyReturn` + `.unwrapOr()` for Rust's `?` operator

When a pipeline has multiple fallible steps and you want to bail on the first failure, use `earlyReturn` with `.unwrapOr(ret)`. This mirrors Rust's `?` operator — propagate the error upward without nesting.

```ts
// Rust equivalent: let x = fallible_step()?;
// Barnum: unwrapOr(ret) bails early if the Result is Err

earlyReturn<FinalOutput>((ret) =>
  pipe(
    step1.unwrapOr(ret), // bail if step1 fails
    step2.unwrapOr(ret), // bail if step2 fails
    step3, // final step produces FinalOutput
  ),
);
```

This also works with `Option` — `.unwrapOr(ret)` bails on `None`:

```ts
earlyReturn<string>((ret) =>
  pipe(
    lookupUser.unwrapOr(ret), // bail on None
    extractEmail, // only runs if Some
  ),
);
```

Without `earlyReturn`, you'd need deeply nested `.branch({ Ok: ..., Err: ... })` at every step. `earlyReturn` + `.unwrapOr()` keeps the pipeline flat.

---

## Handler contracts

### Zod schemas are for serialization boundaries

Zod schemas exist to validate data that crosses the serialization boundary between the pipeline runtime and handler subprocesses. They are not general-purpose type enforcement — TypeScript's type system handles that. You need schemas on handler `inputValidator`/`outputValidator` because data is JSON-serialized over stdin/stdout between processes. You don't need schemas for internal helper functions, pipeline-level constants, or anything that stays within a single process.

### Supported Zod types in handler schemas

Handler schemas are converted to JSON Schema (Draft 7) for the Rust runtime. Only types that survive the TS → JSON → Rust boundary are allowed. The conversion uses `zod`'s `toJSONSchema()` with `unrepresentable: "throw"`.

**Primitives:** `z.string()`, `z.number()`, `z.boolean()`, `z.null()`, `z.unknown()`, `z.any()`

**Literals:** `z.literal("hello")`, `z.literal(42)`, `z.literal(true)`, `z.literal(null)`

**Enums:** `z.enum(["a", "b", "c"])`

**Containers:** `z.object()`, `z.array()`, `z.tuple()`, `z.record()`

**Composition:** `z.union()`, `z.nullable()`, `.optional()`

**Modifiers:**

- String: `.min()`, `.max()`, `.length()`, `.regex()`, `.email()`, `.url()`, `.startsWith()`, `.endsWith()`
- Number: `.min()`, `.max()`, `.gt()`, `.lt()`, `.int()`, `.multipleOf()`
- Array: `.min()`, `.max()`
- `.default()`, `.brand()`

**Rejected (throws at pipeline construction time):**

| Type                          | Reason                                                                                                        |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `z.undefined()`, `z.void()`   | No JSON representation                                                                                        |
| `z.bigint()`                  | No JSON representation                                                                                        |
| `z.symbol()`                  | No JSON representation                                                                                        |
| `z.date()`                    | No JSON representation                                                                                        |
| `z.function()`                | No JSON representation                                                                                        |
| `z.map()`, `z.set()`          | No JSON representation                                                                                        |
| `.transform()`                | Output type differs from input; not expressible in schema                                                     |
| `z.intersection()`            | Produces broken `allOf` with `additionalProperties: false` on Draft 7 — use `.extend()` or `.merge()` instead |
| `.refine()`, `.superRefine()` | Silently stripped from JSON Schema — validation would pass on the Rust side for values that should fail       |

`z.unknown()` and `z.any()` both produce `{}` (empty schema — accepts any JSON value). Use `z.unknown()` when the handler genuinely accepts arbitrary input.

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

### Return `Option` or `Result` — never separate the check from the use

If a handler might not produce a value, return `Option<T>`. If it might fail, return `Result<T, E>`. The pipeline then handles the absence structurally via `.branch()`, `.unwrapOr()`, or `earlyReturn`. The compiler enforces that every consumer handles the empty/error case.

The alternative — a handler that returns a boolean or status flag, followed by a second handler that assumes the value exists — separates the check from the use. The invariant ("I checked, so it's safe") lives in the textual proximity of two pipeline steps, not in the type system. Refactors can move them apart, concurrent branches can invalidate the check, and the compiler can't help.

```ts
// Avoid: check-then-act — the invariant is informal
export const isEmpty = createHandler(
  {
    inputValidator: z.object({ queueDir: z.string() }),
    outputValidator: z.boolean(),
    handle: async ({ value }) => {
      const files = readdirSync(value.queueDir);
      return files.length === 0;
    },
  },
  "isEmpty",
);

export const readFirst = createHandler(
  {
    inputValidator: z.object({ queueDir: z.string() }),
    outputValidator: itemSchema, // assumes non-empty — partial function disguised as total
    handle: async ({ value }) => {
      const files = readdirSync(value.queueDir);
      return JSON.parse(readFileSync(join(value.queueDir, files[0]), "utf-8"));
    },
  },
  "readFirst",
);

// Pipeline: isEmpty guards readFirst, but the compiler doesn't know they're related
isEmpty.branch({ true: readFirst, false: sleep(5000) });
```

```ts
// Prefer: one handler returns Option<T> — partiality is in the type
export const dequeue = createHandler(
  {
    inputValidator: z.object({ queueDir: z.string() }),
    outputValidator: Option.schema(itemSchema),
    handle: async ({ value }): Promise<Option<Item>> => {
      const files = readdirSync(value.queueDir).sort();
      if (files.length === 0) return { kind: "Option.None", value: null };
      const path = join(value.queueDir, files[0]);
      const item = JSON.parse(readFileSync(path, "utf-8"));
      unlinkSync(path);
      return { kind: "Option.Some", value: item };
    },
  },
  "dequeue",
);

// Pipeline: the compiler forces you to handle None
dequeue.branch({
  Some: processItem,
  None: sleep(5_000),
});
```

The typed version fuses the check and the use into a single atomic operation. There's no window where the invariant can be invalidated, no informal proof for the reader to re-verify, and no way to forget the empty case.

This generalizes: anywhere you'd write "check condition, then act assuming condition holds," ask whether a single operation can return a type that encodes both outcomes. `Option` for presence/absence, `Result` for success/failure, a tagged union for multi-way decisions. Push the invariant into the type so the compiler enforces it instead of you.

### Panic on impossible states — don't handle them "gracefully"

If your handler reaches a state that should be structurally impossible — a tagged union variant you didn't expect, a null where the type says non-null, a missing file that a previous step guaranteed exists — throw. Don't return a default, don't log and continue, don't return `None` when the contract says `Some`. A "graceful" response to an impossible state is a lie that propagates corruption downstream.

```ts
// Avoid: silently swallowing an impossible state
handle: async ({ value }) => {
  const item = lookupById(value.id);
  if (!item) {
    console.warn(`Item ${value.id} not found, skipping`);
    return { kind: "Option.None", value: null }; // this can't happen — upstream guarantees existence
  }
  return { kind: "Option.Some", value: item };
};

// Prefer: crash immediately — the bug is upstream, not here
handle: async ({ value }) => {
  const item = lookupById(value.id);
  if (!item) {
    throw new Error(
      `Invariant violation: item ${value.id} must exist (guaranteed by previous step)`,
    );
  }
  return item;
};
```

A panic is a signal to the developer that an assumption is broken. A graceful fallback hides the broken assumption and lets it compound. The earlier you crash, the closer the stack trace is to the actual bug. Use `Result`/`Option` for states that _can_ legitimately occur; use `throw` for states that _cannot_.

### Return tagged unions, not booleans

A boolean return is a closed, two-valued type with no payload and no extensibility. A tagged union with two variants carries the same information but is self-documenting at call sites, can carry data per variant, and extends to three or more cases without a breaking change.

```ts
// Avoid: boolean — opaque at the call site, can't carry data, can't extend
export const checkCapacity = createHandler(
  {
    outputValidator: z.boolean(),
    handle: async ({ value }) => {
      return readdirSync(value.queueDir).length < value.maxSize;
    },
  },
  "checkCapacity",
);

// Pipeline: what does true mean? What does false mean?
checkCapacity.branch({ true: produce, false: sleep(60_000) });
```

```ts
// Prefer: tagged union — self-documenting, extensible, can carry data
export const checkCapacity = createHandler(
  {
    outputValidator: taggedUnionSchema("Capacity", {
      HasCapacity: z.object({ remaining: z.number() }),
      Full: z.null(),
    }),
    handle: async ({ value }): Promise<Capacity> => {
      const count = readdirSync(value.queueDir).length;
      if (count >= value.maxSize) return { kind: "Capacity.Full", value: null };
      return {
        kind: "Capacity.HasCapacity",
        value: { remaining: value.maxSize - count },
      };
    },
  },
  "checkCapacity",
);

// Pipeline: variants are named, and Full can later become Degraded/Full/Overloaded without breakage
checkCapacity.branch({
  HasCapacity: produce,
  Full: sleep(60_000),
});
```

When a third state appears (and it will — "degraded," "rate-limited," "shutting down"), a boolean forces a breaking change everywhere. A tagged union just adds a variant. Start with the union.

### Factor shared data out of tagged union variants

If every variant of a tagged union contains the same field, that field doesn't belong inside the union — it belongs alongside it. Move it into a tuple or object wrapping the union. This avoids redundant extraction logic in every branch case and makes the shared data accessible without dispatching.

```ts
// Avoid: every variant repeats the same field
outputValidator: taggedUnionSchema("ReviewResult", {
  Approved: z.object({ file: z.string(), approver: z.string() }),
  Rejected: z.object({ file: z.string(), reason: z.string() }),
});
// Every branch case must extract `file` separately

// Prefer: shared data lives outside the union
outputValidator: z.object({
  file: z.string(),
  decision: taggedUnionSchema("ReviewDecision", {
    Approved: z.object({ approver: z.string() }),
    Rejected: z.object({ reason: z.string() }),
  }),
});
// Pipeline can access `file` without branching:
// result.getField("file") works regardless of decision
```

### Namespace tagged union variants

When a handler returns a decision (e.g., "needs work" vs "approved"), namespace the variants in `taggedUnionSchema`. This prevents collisions when multiple branch points exist in the same pipeline and makes branch dispatch unambiguous.

```ts
// Handler returns a namespaced decision:
outputValidator: taggedUnionSchema("Judgment", {
  NeedsWork: feedbackSchema,
  Approved: z.null(),
});

// Branch dispatches on the short names:
classifyJudgment.branch({
  NeedsWork: applyFeedback.then(recur),
  Approved: drop,
});
```

### Always annotate `handle` return types

Always add an explicit return type to `handle`. Two reasons:

1. **No `as const` needed.** Without a return type, TypeScript widens `"Result.Ok"` to `string` unless you write `as const`. With an explicit return type, TypeScript checks the literal against the annotation directly — no casting ceremony.

2. **No narrowing surprises.** When a handler returns a tagged union but only constructs one variant in a given code path, TypeScript narrows the inferred return to that single variant. The pipeline then fails to typecheck because the output is narrower than the full union expected by `.branch()` or `.unwrapOr()`.

```ts
type AnalysisResult = Result<string, string>;

// Avoid: needs `as const`, and if only one branch is returned, TypeScript narrows
handle: async ({ value }) => {
  return { kind: "Result.Ok" as const, value: "done" };
  // Inferred: { kind: "Result.Ok", value: string } — not Result<string, string>
};

// Prefer: explicit return type — no `as const`, no narrowing issues
handle: async ({ value }): Promise<AnalysisResult> => {
  return { kind: "Result.Ok", value: "done" };
};
```

### Use `null` not `undefined` for empty pipeline values

Pipeline definitions are serialized to JSON. `undefined` has no JSON representation — `JSON.stringify({ value: undefined })` produces `{}`, which causes the Rust deserializer to fail with a missing field error. TypeScript won't catch this because `void` accepts `undefined` as a valid value.

Use `null` for "no meaningful value" in pipeline data:

```ts
// Broken: undefined disappears during serialization
Skip: bindInput<null, void>(() => constant(undefined));

// Fixed: null serializes correctly
Skip: bindInput<null, null>(() => constant(null));
```

This applies anywhere you construct pipeline values — `constant()`, handler return values, branch cases. If you mean "nothing," use `null`.

### Use void returns for side-effect-only handlers

If a handler's purpose is a side effect (write a file, send a message, invoke an LLM with tools), return `void` from `handle`. The framework types it as `never` output — the next step starts fresh via `.drop()` or naturally from a new source. Don't return `null` and pass it along.

```ts
// Avoid: returning null as a meaningless value that gets threaded through
export const implement = createHandler(
  {
    outputValidator: z.null(),
    handle: async ({ value }) => {
      await invokeLanguageModel({ prompt: `Implement ${value.description}` });
      return null;
    },
  },
  "implement",
);

// Prefer: void return — framework knows there's no output
export const implement = createHandler(
  {
    inputValidator: z.object({ description: z.string() }),
    handle: async ({ value }) => {
      await invokeLanguageModel({ prompt: `Implement ${value.description}` });
    },
  },
  "implement",
);
```

---

## Minimize work inside LLM handlers

When a handler invokes an LLM agent (e.g., `invokeLanguageModel`), the agent's effectiveness is bounded by the context it receives. Pre-read files in earlier pipeline steps and pass the content as input — don't make the agent spend tokens discovering information you already have.

### Pre-read the file being modified

If the agent's job is to modify a file, read it before the handler runs and pass the content in. The agent sees the full file immediately instead of burning a tool call to read it.

```ts
// Avoid: agent wastes a tool call reading the file
export const refactor = createHandler(
  {
    inputValidator: z.object({ file: z.string() }),
    handle: async ({ value }) => {
      await invokeLanguageModel({
        prompt: `Refactor ${value.file}`,
        allowedTools: ["Read", "Edit"],
      });
    },
  },
  "refactor",
);

// Prefer: pre-read in the pipeline, agent starts with full context
export const readFile = createHandler(
  {
    inputValidator: z.object({ file: z.string() }),
    outputValidator: z.object({ file: z.string(), content: z.string() }),
    handle: async ({ value }) => ({
      file: value.file,
      content: readFileSync(value.file, "utf-8"),
    }),
  },
  "readFile",
);

export const refactor = createHandler(
  {
    inputValidator: z.object({ file: z.string(), content: z.string() }),
    handle: async ({ value }) => {
      await invokeLanguageModel({
        prompt: `Refactor this file (${value.file}):\n\n${value.content}`,
        allowedTools: ["Edit"],
      });
    },
  },
  "refactor",
);
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
  }).then(refactorWithContext),
);
```

### Why this matters

Every tool call an LLM agent makes costs latency and tokens. A `Read` call the agent makes inside a handler is identical work the pipeline could have done deterministically in milliseconds. The agent should spend its budget on judgment and creativity — deciding _what_ to change — not on mechanically gathering files it was always going to need.

---

## Prefer postfix methods over standalone functions

When a combinator is available as both a standalone function and a postfix method, **always prefer the postfix form.** Two reasons:

1. **No type parameters.** Standalone functions like `getField<TObj, TField>(field)` often require explicit generic arguments because TypeScript can't infer the input type without context. The postfix form `action.getField("name")` infers everything from the preceding action's output type — zero annotation needed.

2. **No wrapping in `pipe`.** Standalone functions used mid-pipeline need a `pipe(action, getField("name"))` wrapper. Postfix chains directly: `action.getField("name")`.

```ts
// Avoid: standalone requires type parameters and pipe wrapping
pipe(getUserProfile, getField<UserProfile, "email">("email"));

// Prefer: postfix infers types from context
getUserProfile.getField("email");
```

This applies to every combinator that has a postfix form: `.then()`, `.iterate()`, `.map()`, `.flatMap()`, `.filter()`, `.collect()`, `.branch()`, `.drop()`, `.tag()`, `.flatten()`, `.getField()`, `.getIndex()`, `.pick()`, `.wrapInField()`, `.splitFirst()`, `.splitLast()`, `.mapErr()`, `.unwrapOr()`.

### Extract postfix operations into named actions with `identity()`

Every postfix method can be turned into a standalone named action by calling it on `identity()`. This is useful when the same operation appears in multiple places, or when you want to name a step for readability:

```ts
// Inline postfix:
classify.branch({ Critical: escalate, Low: log });

// Extracted to a named action:
const route = identity<PrCategory>().branch({ Critical: escalate, Low: log });
classify.then(route);
```

`identity<T>()` creates a typed starting point, so the postfix method infers all its types. The resulting action is a normal `TypedAction<PrCategory, ...>` you can use anywhere — pass to `.then()`, store in a variable, reuse across pipelines.

## `.then()` vs `pipe()` for chaining

**Two steps: use `.then()`.** It reads left-to-right, stays on one line, and doesn't require an import.

**Three or more steps: use `pipe()`.** It's flatter than nested `.then()` chains and each step occupies its own line.

```ts
// Two steps: .then() — always preferred
listFiles.then(commit);

// Three+ steps: pipe() — one step per line, easy to scan
pipe(listFiles, processFiles, commit);

// Avoid: long .then() chains — hard to scan, awkward to add steps
listFiles.then(processFiles).then(validate).then(commit);

// Avoid: pipe for two items — .then() is simpler
pipe(listFiles, commit);

// Avoid: mixing pipe and .then() — pick one
pipe(listFiles, processFiles).then(commit);
```

Postfix methods like `.iterate()`, `.map()`, `.collect()`, `.getField()`, `.branch()` are always preferred over their standalone equivalents regardless of chain length — they infer types from context and don't require explicit type parameters.

## Use `taggedUnionSchema` for handler validators

When a handler returns a tagged union, use `taggedUnionSchema()`, `Option.schema()`, or `Result.schema()` instead of hand-rolling `z.discriminatedUnion()`:

```ts
// Avoid
outputValidator: z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("HasErrors"), value: z.array(errorSchema) }),
  z.object({ kind: z.literal("Clean"), value: z.null() }),
]);

// Prefer
outputValidator: taggedUnionSchema({
  HasErrors: z.array(errorSchema),
  Clean: z.null(),
});
```

For `Option` and `Result` specifically:

```ts
outputValidator: Option.schema(z.string()); // Option<string>
outputValidator: Result.schema(z.string(), z.number()); // Result<string, number>
```

**Critical:** `Result.schema(ok, err)` and `taggedUnionSchema("Result", { Ok: ok, Err: err })` are NOT the same. Only `Result.schema()` produces a type compatible with `.unwrapOr()`, `.mapErr()`, and other Result-aware postfix methods. `taggedUnionSchema("Result", ...)` produces a generic tagged union that requires `.branch()` dispatch — the Result-specific combinators won't recognize it.

### `Result.Err` and `Option.Some` both use `.value` — not `.error` or `.data`

All tagged union variants use the field name `value` for their payload, including error cases. This is consistent (every variant is `{ kind: string, value: T }`) but potentially surprising for `Result.Err`:

```ts
// The error payload is in .value, NOT .error:
{ kind: "Result.Ok", value: "success data" }
{ kind: "Result.Err", value: "error message" }  // ← .value, not .error

{ kind: "Option.Some", value: 42 }
{ kind: "Option.None", value: null }
```

This uniformity means all tagged unions share the same shape — `branch()`, `getField("value")`, and serialization work identically regardless of which variant is active.

### `.branch()` requires exhaustive variants

Every variant in the tagged union must be handled. If a handler returns `taggedUnionSchema({ Pass: ..., Fail: ..., Skip: ... })`, the `.branch()` call must include all three cases. Missing a variant is a type error (and a runtime panic if the missing variant is produced).

```ts
// Type error: missing "Skip" case
classify.branch({ Pass: deploy, Fail: rollback });

// Correct: all variants handled
classify.branch({ Pass: deploy, Fail: rollback, Skip: drop });
```

### Branded types work in handler schemas

`z.string().brand("BranchName")` produces a branded type that survives pipeline serialization. The brand is reapplied by zod at each validation boundary (handler input/output). Use brands to distinguish semantically different strings at the type level.

```ts
const branchNameSchema = z.string().brand("BranchName");
type BranchName = z.infer<typeof branchNameSchema>;

// Now BranchName is not assignable to/from plain string
inputValidator: z.object({ branch: branchNameSchema });
```
