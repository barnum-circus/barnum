# Pipeline Exports

**Blocked by:** `OPTIONAL_HANDLER_TYPES.md`, `HANDLER_VALIDATION.md`

## TL;DR

Export pipelines alongside handlers. A pipeline is a composed `TypedAction` that wires multiple handlers together — it's what you'd write in `run.ts` today, but lifted into the handler module so it can be reused across workflows. Handlers still need to be exported (the worker imports them by name), but the pipeline is the public API that workflows consume.

After `HANDLER_VALIDATION.md` lands, handlers that call Claude get runtime input/output validation. A pipeline that wraps a Claude-calling handler with validation produces a unit that won't panic on bad data — it validates before invoking.

---

## Motivation

Right now, `run.ts` files do two things: define pipelines and define workflows. The pipeline composition (e.g., `typeCheckFix`, `implementAndReview` in the refactor demo) lives in `run.ts` even though it's reusable. Meanwhile, handler files export atomic handlers with no composition.

This creates three problems:

1. **Reusable pipelines are trapped in workflow files.** The `typeCheckFix` loop in `identify-and-address-refactors/run.ts` is identical to the loop in `convert-folder-to-ts/run.ts`. Both inline it.

2. **Atomic handlers are too granular for most callers.** A workflow that wants "implement a refactor and get it passing" has to compose `implement`, `typeCheck`, `classifyErrors`, `fix`, `commit` with the right control flow every time. The composition is the abstraction, not the individual handlers.

3. **No natural place for validation boundaries.** After `HANDLER_VALIDATION.md`, handlers validate their own inputs. But a pipeline that composes three handlers has an overall input/output contract that's separate from the individual handler contracts. Pipeline exports are where you'd express "this whole operation takes a Refactor and produces a PR URL."

---

## Design

### A pipeline export is a `TypedAction`

There's no new concept. A pipeline is a `TypedAction` produced by `pipe`, `loop`, `bindInput`, etc. The export is just a module-level binding:

```ts
// handlers/type-check-fix.ts

// Handlers are still exported (worker needs them by name).
export const typeCheck = createHandler({ ... }, "typeCheck");
export const classifyErrors = createHandler({ ... }, "classifyErrors");
export const fix = createHandler({ ... }, "fix");

// Pipeline export: the composed operation.
export const typeCheckFix = bindInput<{ worktreePath: string }>((params) =>
  loop<void>((recur, done) =>
    params.then(pipe(typeCheck, classifyErrors)).branch({
      HasErrors: forEach(fix).drop().then(recur),
      Clean: done,
    }),
  ),
);
```

Callers import `typeCheckFix` and use it directly. They never touch `typeCheck`, `classifyErrors`, or `fix` unless they need custom composition.

### Handlers remain exported

The Rust worker imports handler modules and calls exports by name. A handler's `module` + `func` in the AST must resolve to an actual export. This is a hard constraint — handlers must be exported regardless of whether the pipeline is the intended public API.

In a future with a compile step (babel transform, build plugin), handlers could be non-exported and registered by the compiler. That's not this refactor.

### Pipeline typing

A pipeline is a `TypedAction<TIn, TOut>`. It composes with everything else: `pipe`, `.then()`, `forEach`, `withResource`, etc. No special treatment needed.

With output validators from `OPTIONAL_HANDLER_TYPES.md` and runtime validation from `HANDLER_VALIDATION.md`, a pipeline's boundary handlers validate at runtime. The pipeline as a whole gets validation for free from its constituent parts.

---

## Concrete examples from demos

### Example 1: `typeCheckFix` — shared between two demos

**Current state:** Both `convert-folder-to-ts/run.ts` and `identify-and-address-refactors/run.ts` inline the same type-check/fix loop.

convert-folder-to-ts:
```ts
loop((recur) =>
  pipe(typeCheck, classifyErrors).branch({
    HasErrors: pipe(forEach(fix).drop(), recur),
    Clean: drop,
  }),
)
```

identify-and-address-refactors:
```ts
const typeCheckFix = bindInput<{ worktreePath: string }>((typeCheckFixParams) =>
  loop<void>((recur, done) =>
    typeCheckFixParams.then(pipe(typeCheck, classifyErrors)).branch({
      HasErrors: forEach(fix).drop().then(recur),
      Clean: done,
    }),
  ),
);
```

These are almost the same — the refactor demo wraps it in `bindInput` because it needs to re-inject `{ worktreePath }` each iteration, while the convert demo's `typeCheck` handler has the output directory hardcoded.

**After:** Export `typeCheckFix` from `handlers/type-check-fix.ts`. Both demos import it. The convert demo may need the handler signatures adjusted so `typeCheck` accepts a path parameter instead of hardcoding `baseDir`, but that's a separate demo cleanup.

### Example 2: `implementAndReview` — reusable "implement, validate, iterate" pipeline

**Current state:** `identify-and-address-refactors/run.ts` defines:

```ts
const implementAndReview = bindInput<ImplementAndReviewParams>((params) => pipe(
  params.pick("worktreePath", "description").then(implement).drop(),
  params.pick("worktreePath").then(typeCheckFix).drop(),
  loop((recur) =>
    pipe(judgeRefactor, classifyJudgment).branch({
      NeedsWork: pipe(
        applyFeedback.drop(),
        params.pick("worktreePath").then(typeCheckFix),
      ).drop().then(recur),
      Approved: drop,
    }),
  ).drop(),
  params.pick("worktreePath").then(commit).drop(),
  pipe(params.pick("branch", "description"), preparePRInput, createPR),
));
```

This is a complete "implement → typecheck → review → fix → commit → PR" pipeline. It could live in the handler module.

**After:** Export from `handlers/refactor.ts`:

```ts
export const implementAndReview = bindInput<ImplementAndReviewParams>((params) => pipe(
  params.pick("worktreePath", "description").then(implement).drop(),
  params.pick("worktreePath").then(typeCheckFix).drop(),
  loop((recur) =>
    pipe(judgeRefactor, classifyJudgment).branch({
      NeedsWork: pipe(
        applyFeedback.drop(),
        params.pick("worktreePath").then(typeCheckFix),
      ).drop().then(recur),
      Approved: drop,
    }),
  ).drop(),
  params.pick("worktreePath").then(commit).drop(),
  pipe(params.pick("branch", "description"), preparePRInput, createPR),
));
```

`run.ts` becomes:

```ts
await workflowBuilder()
  .workflow(() =>
    pipe(
      constant({ folder: srcDir }),
      listTargetFiles,
      forEach(analyze).flatten(),
      forEach(assessWorthiness).then(Option.collect()),
      forEach(
        withResource({
          create: pipe(pick<Refactor, ["description"]>("description"), deriveBranch, createWorktree),
          action: implementAndReview,
          dispose: deleteWorktree,
        }),
      ),
    ),
  )
  .run();
```

The workflow file is now purely about orchestration — what runs in what order, resource management, fan-out. The how of implementation+review lives in the handler module.

### Example 3: `classifyJudgment` is a pipeline candidate, not a handler

`classifyJudgment` in `identify-and-address-refactors/handlers/refactor.ts` is:

```ts
export const classifyJudgment = createHandler({
  inputValidator: z.union([
    z.object({ approved: z.literal(true) }),
    z.object({ approved: z.literal(false), instructions: z.string() }),
  ]),
  handle: async ({ value: judgment }): Promise<ClassifyJudgmentResult> => {
    if (judgment.approved) {
      return { kind: "Approved", value: undefined };
    }
    return { kind: "NeedsWork", value: judgment.instructions };
  },
}, "classifyJudgment");
```

This is a pure data transformation — it converts a `{ approved: boolean, instructions?: string }` into a `TaggedUnion`. There's no I/O, no side effects. It's a handler only because the framework didn't have a way to express pure transformations as pipelines. With pipeline exports, this could be expressed using builtins (a `classify` or `tag` combinator) rather than a handler that requires worker dispatch. That's a future optimization — for now, the point is that this handler is logically a pipeline step, and its natural home is composed inline rather than as a standalone export.

Similarly, `classifyErrors` in `type-check-fix.ts` is the same pattern: pure classification of `TypeError[] → TaggedUnion<{ HasErrors, Clean }>`.

### Example 4: `deriveBranch` + `preparePRInput` — pure data shaping

```ts
export const deriveBranch = createHandler({
  inputValidator: z.object({ description: z.string() }),
  handle: async ({ value }) => ({
    branch: `refactor/${value.description.toLowerCase().replace(/[^a-z0-9]+/g, "-").slice(0, 40)}`,
  }),
}, "deriveBranch");

export const preparePRInput = createHandler({
  inputValidator: z.object({ branch: z.string(), description: z.string() }),
  handle: async ({ value }) => ({
    branch: value.branch,
    title: `Refactor: ${value.description.slice(0, 60)}`,
    body: `Automated refactor:\n\n${value.description}`,
  }),
}, "preparePRInput");
```

Both are pure transformations. In the workflow, they're already composed as a pipeline: `pipe(pick("description"), deriveBranch, createWorktree)`. They could be a single pipeline export that does the whole "Refactor → git branch → worktree" setup, but they're also useful individually.

The observation here isn't "turn these into pipelines" — it's that the natural composition `pipe(deriveBranch, createWorktree)` is a pipeline that callers would want to import as a unit. The handler module could export both the atoms and the composed pipeline.

---

## Validation boundary

After `HANDLER_VALIDATION.md`, handlers with validators check inputs/outputs at runtime. A pipeline inherits this: if `implement` has an `inputValidator`, calling `implement` through a pipeline still validates.

The interesting case is Claude-calling handlers. These can produce malformed output (bad JSON, missing fields, wrong types). With `outputValidator`, the handler validates Claude's response before returning. A pipeline that wraps a Claude handler inherits this protection.

Handlers that call Claude and currently parse JSON manually (analyze, assessWorthiness, judgeRefactor) could instead:
1. Have an `outputValidator` that enforces the expected shape
2. Let the validation framework reject bad responses
3. The pipeline wrapping them handles the error (via `tryCatch` or `Result`)

This separates "what shape do I expect" (declarative, in the validator) from "what do I do when it's wrong" (pipeline-level error handling).

---

## What this does NOT include

- **No new runtime concepts.** Pipelines are `TypedAction` — already exist.
- **No changes to the worker.** Handlers are still exported, still dispatched by name.
- **No babel transform.** Handlers remain exported. The pipeline is an additional export, not a replacement.
- **No new builtins for pure classification.** `classifyJudgment` and `classifyErrors` stay as handlers. A `tag`/`classify` builtin is a future optimization.

This is a demo-level change: move pipeline composition from `run.ts` into handler modules, identify at least one shared pipeline (`typeCheckFix`), and demonstrate the pattern.
