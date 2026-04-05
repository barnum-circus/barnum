# Pipeline Exports

**Blocked by:** `OPTIONAL_HANDLER_TYPES.md`

## TL;DR

Export pipelines alongside handlers. A pipeline is a composed `TypedAction` that wires multiple handlers together — it's what you'd write in `run.ts` today, but lifted into the handler module so it can be reused across workflows. Handlers still need to be exported (the worker imports them by name), but the pipeline is the public API that workflows consume.

---

## Motivation

Right now, `run.ts` files do two things: define pipelines and define workflows. The pipeline composition (e.g., `typeCheckFix`, `implementAndReview` in the refactor demo) lives in `run.ts` even though it's reusable. Meanwhile, handler files export atomic handlers with no composition.

This creates two problems:

1. **Reusable pipelines are trapped in workflow files.** The `typeCheckFix` loop in `identify-and-address-refactors/run.ts` is identical to the loop in `convert-folder-to-ts/run.ts`. Both inline it.

2. **Atomic handlers are too granular for most callers.** A workflow that wants "implement a refactor and get it passing" has to compose `implement`, `typeCheck`, `classifyErrors`, `fix`, `commit` with the right control flow every time. The composition is the abstraction, not the individual handlers.

---

## Design

### A pipeline export is a `TypedAction`

There's no new concept. A pipeline is a `TypedAction` produced by `pipe`, `loop`, `bindInput`, etc. The export is just a module-level binding.

### Handlers remain exported

The Rust worker imports handler modules and calls exports by name. A handler's `module` + `func` in the AST must resolve to an actual export. This is a hard constraint — handlers must be exported regardless of whether the pipeline is the intended public API.

In a future with a compile step (babel transform, build plugin), handlers could be non-exported and registered by the compiler. That's not this refactor.

---

## Concrete examples from demos

### Example 1: `typeCheckFix` — shared between two demos

Both `convert-folder-to-ts/run.ts` and `identify-and-address-refactors/run.ts` inline the same type-check/fix loop.

**Before** — `convert-folder-to-ts/run.ts`:

```ts
import { typeCheck, classifyErrors, fix } from "./handlers/type-check-fix.js";

await workflowBuilder()
  .workflow(() =>
    pipe(
      setup,
      listFiles
        .forEach(migrate({ to: "Typescript" }))
        .drop(),
      loop((recur) =>
        pipe(typeCheck, classifyErrors).branch({
          HasErrors: pipe(forEach(fix).drop(), recur),
          Clean: drop,
        }),
      ),
    ),
  )
  .run();
```

**Before** — `identify-and-address-refactors/run.ts`:

```ts
import { typeCheck, classifyErrors, fix } from "./handlers/type-check-fix.js";

const typeCheckFix = bindInput<{ worktreePath: string }>((typeCheckFixParams) =>
  loop<void>((recur, done) =>
    typeCheckFixParams.then(pipe(typeCheck, classifyErrors)).branch({
      HasErrors: forEach(fix).drop().then(recur),
      Clean: done,
    }),
  ),
);
```

These are almost the same loop — the refactor demo wraps it in `bindInput` because it needs to re-inject `{ worktreePath }` each iteration, while the convert demo's `typeCheck` handler has the output directory hardcoded.

**After** — `handlers/type-check-fix.ts`:

```ts
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

**After** — `convert-folder-to-ts/run.ts`:

```ts
import { typeCheckFix } from "./handlers/type-check-fix.js";

await workflowBuilder()
  .workflow(() =>
    pipe(
      setup,
      listFiles
        .forEach(migrate({ to: "Typescript" }))
        .drop(),
      typeCheckFix,
    ),
  )
  .run();
```

**After** — `identify-and-address-refactors/run.ts`:

```ts
import { typeCheckFix } from "./handlers/type-check-fix.js";

// typeCheckFix used directly — no longer defined here.
```

The convert demo may need `typeCheck` adjusted to accept a path parameter instead of hardcoding `baseDir`, but that's a separate demo cleanup.

### Example 2: `implementAndReview` — reusable "implement, validate, iterate" pipeline

**Before** — `identify-and-address-refactors/run.ts`:

```ts
import {
  listTargetFiles, analyze, assessWorthiness,
  deriveBranch, preparePRInput,
  implement, commit,
  judgeRefactor, classifyJudgment, applyFeedback,
  type Refactor,
} from "./handlers/refactor.js";
import { createWorktree, deleteWorktree, createPR } from "./handlers/git.js";
import { typeCheck, classifyErrors, fix } from "./handlers/type-check-fix.js";

type ImplementAndReviewParams = Refactor & { worktreePath: string; branch: string };

const typeCheckFix = bindInput<{ worktreePath: string }>((typeCheckFixParams) =>
  loop<void>((recur, done) =>
    typeCheckFixParams.then(pipe(typeCheck, classifyErrors)).branch({
      HasErrors: forEach(fix).drop().then(recur),
      Clean: done,
    }),
  ),
);

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

**After** — `handlers/refactor.ts` (new exports at bottom of file):

```ts
// ... existing handler exports (listTargetFiles, analyze, etc.) ...

import { typeCheckFix } from "./type-check-fix.js";
import { createWorktree, deleteWorktree, createPR } from "./git.js";

export type ImplementAndReviewParams = Refactor & { worktreePath: string; branch: string };

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

**After** — `identify-and-address-refactors/run.ts`:

```ts
import {
  listTargetFiles, analyze, assessWorthiness,
  deriveBranch,
  implementAndReview,
  type Refactor,
} from "./handlers/refactor.js";
import { createWorktree, deleteWorktree } from "./handlers/git.js";

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

### Example 3: `classifyJudgment` — pure classification, logically a pipeline step

**Before** — `handlers/refactor.ts`:

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

This is a pure data transformation — it converts `{ approved: boolean, instructions?: string }` into a `TaggedUnion`. No I/O, no side effects. It's a handler only because the framework doesn't have a builtin for tagging. A future `tag`/`classify` builtin would let this be expressed as a pipeline, eliminating the worker dispatch overhead. For now, no change — just noting it as a pattern that pipeline exports make visible.

`classifyErrors` in `type-check-fix.ts` is the same pattern: `TypeError[] → TaggedUnion<{ HasErrors, Clean }>`.

### Example 4: `deriveBranch` + `preparePRInput` — composable atoms

**Before** — used in `run.ts` as:

```ts
pipe(pick<Refactor, ["description"]>("description"), deriveBranch, createWorktree)
```

and separately:

```ts
pipe(params.pick("branch", "description"), preparePRInput, createPR)
```

Both `deriveBranch` and `preparePRInput` are pure transformations. They're already composed inline with I/O handlers (`createWorktree`, `createPR`). The handler module could export these composed pipelines:

**After** — `handlers/refactor.ts`:

```ts
// Atoms (still exported for the worker).
export const deriveBranch = createHandler({ ... }, "deriveBranch");
export const preparePRInput = createHandler({ ... }, "preparePRInput");

// Composed pipelines.
export const createBranchWorktree = pipe(
  pick<Refactor, ["description"]>("description"),
  deriveBranch,
  createWorktree,
);

export const openPR = pipe(preparePRInput, createPR);
```

---

## What this does NOT include

- **No new runtime concepts.** Pipelines are `TypedAction` — already exist.
- **No changes to the worker.** Handlers are still exported, still dispatched by name.
- **No babel transform.** Handlers remain exported. The pipeline is an additional export, not a replacement.
- **No new builtins for pure classification.** `classifyJudgment` and `classifyErrors` stay as handlers. A `tag`/`classify` builtin is a future optimization.

This is a demo-level change: move pipeline composition from `run.ts` into handler modules, identify at least one shared pipeline (`typeCheckFix`), and demonstrate the pattern.
