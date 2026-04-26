# Serial Execution

`.then()` chains actions sequentially. The output of each step becomes the input of the next. Each step completes before the next starts.

## Linear pipeline

From [`demos/simple-workflow/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/simple-workflow/run.ts):

```ts
runPipeline(
  listFiles
    .iterate()
    .map(
      implementRefactor
        .then(typeCheckFiles)
        .then(fixTypeErrors)
        .then(commitChanges)
        .then(createPullRequest),
    )
    .collect(),
);
```

Five steps in strict order: refactor → type-check → fix → commit → PR. Each handler receives the previous handler's output. `.iterate().map()` runs this pipeline per file in parallel, but within each file the steps are serial.

## Data transformation chain

From [`demos/identify-and-address-refactors/handlers/refactor.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/identify-and-address-refactors/handlers/refactor.ts):

```ts
export const createBranchWorktree = pick<Refactor, ["description"]>("description")
  .then(deriveBranch)
  .then(createWorktree);
```

Pure sequential data flow: extract a field → derive a branch name → create a worktree. No concurrency, no branching — just step-by-step transformation.

## Multi-phase workflow

From [`demos/convert-folder-to-ts/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/convert-folder-to-ts/run.ts):

```ts
runPipeline(
  setup
    .then(
      listFiles.iterate().map(migrate({ to: "Typescript" })).collect().drop(),
    )
    .then(typeCheckFix),
);
```

Three sequential phases: setup → migrate all files (parallel within this step) → type-check and fix. The `.then()` chain guarantees setup finishes before migration starts, and all migrations finish before type-checking.

## `pipe()` as an alternative

`pipe()` is equivalent to a `.then()` chain. It's available when you prefer a linear list of steps:

```ts
// These are equivalent:
a.then(b).then(c)
pipe(a, b, c)
```

## How it works

`a.then(b).then(c)` compiles to nested `Chain` nodes: `Chain(a, Chain(b, c))`. At runtime, `advance()` creates a Chain frame, runs `first`, and when it completes, trampolines to `rest` — no recursion, no stack growth.
