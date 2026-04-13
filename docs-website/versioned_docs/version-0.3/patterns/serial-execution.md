# Serial Execution

`pipe` chains actions sequentially. The output of each step becomes the input of the next. Each step completes before the next starts.

## Linear pipeline

From [`demos/simple-workflow/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/simple-workflow/run.ts):

```ts
runPipeline(
  listFiles
    .forEach(
      pipe(
        implementRefactor,
        typeCheckFiles,
        fixTypeErrors,
        commitChanges,
        createPullRequest,
      ),
    ),
);
```

Five steps in strict order: refactor → type-check → fix → commit → PR. Each handler receives the previous handler's output. The `forEach` runs this pipeline per file in parallel, but within each file the steps are serial.

## Data transformation chain

From [`demos/identify-and-address-refactors/handlers/refactor.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/identify-and-address-refactors/handlers/refactor.ts):

```ts
export const createBranchWorktree = pipe(
  pick<Refactor, ["description"]>("description"),
  deriveBranch,
  createWorktree,
);
```

Pure sequential data flow: extract a field → derive a branch name → create a worktree. No concurrency, no branching — just step-by-step transformation.

## Multi-phase workflow

From [`demos/convert-folder-to-ts/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/convert-folder-to-ts/run.ts):

```ts
runPipeline(
  pipe(
    setup,
    listFiles
      .forEach(migrate({ to: "Typescript" }))
      .drop(),
    typeCheckFix,
  ),
);
```

Three sequential phases: setup → migrate all files (parallel within this step) → type-check and fix. The outer `pipe` guarantees setup finishes before migration starts, and all migrations finish before type-checking.

## Postfix `.then()` as an alternative

`.then()` is the postfix equivalent of `pipe` for chaining a single step:

```ts
// These are equivalent:
pipe(a, b, c)
a.then(b).then(c)
```

`.then()` is useful when mixing with other postfix methods:

```ts
listFiles
  .forEach(processFile)
  .drop()
  .then(commit)
```

## How it works

`pipe(a, b, c)` right-folds into nested `Chain` nodes: `Chain(a, Chain(b, c))`. At runtime, `advance()` creates a Chain frame, runs `first`, and when it completes, trampolines to `rest` — no recursion, no stack growth.
