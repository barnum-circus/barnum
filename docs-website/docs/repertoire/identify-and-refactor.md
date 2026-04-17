# Identify and Refactor

Scan a codebase for refactoring opportunities, then implement each refactor in an isolated git worktree with LLM-powered review.

## Workflow

From [`demos/identify-and-address-refactors/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/identify-and-address-refactors/run.ts):

```ts
runPipeline(
  constant({ folder: srcDir })
    .then(listTargetFiles)
    .then(forEach(analyze).flatten())
    .then(forEach(assessWorthiness).then(Option.collect()))
    .then(forEach(
      withResource({
        create: createBranchWorktree,
        action: implementAndReview,
        dispose: deleteWorktree,
      }),
    )),
);
```

## Stages

1. **List files** — find all files in the target directory.
2. **Analyze** — for each file, identify refactoring opportunities. `flatten()` merges the per-file arrays into a single list.
3. **Filter** — an LLM assesses whether each refactor is worth doing. `Option.collect()` filters out the `None` values.
4. **Implement in worktrees** — each surviving refactor gets its own git worktree via `withResource`. The worktree is created before work starts and cleaned up after, regardless of success or failure.

## The implementation loop

From [`demos/identify-and-address-refactors/handlers/refactor.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/identify-and-address-refactors/handlers/refactor.ts):

```ts
export const implementAndReview = bindInput<ImplementAndReviewParams>((params) =>
  params.pick("worktreePath", "description").then(implement).drop()
    .then(params.pick("worktreePath").then(typeCheckFix).drop())

    .then(loop((recur) =>
      judgeRefactor.then(classifyJudgment).branch({
        NeedsWork: applyFeedback.drop()
          .then(params.pick("worktreePath").then(typeCheckFix))
          .drop().then(recur),
        Approved: drop,
      }),
    ).drop())

    .then(params.pick("worktreePath").then(commit).drop())
    .then(params.pick("branch", "description").then(preparePRInput).then(createPR)),
);
```

Each refactor goes through: implement → type-check/fix → judge → revise if needed → commit → PR. The `bindInput` gives every step access to the original parameters (worktree path, branch name, description) without threading them through the pipeline.

## Key points

- **Worktree isolation**: each refactor runs in its own git worktree. Parallel refactors don't interfere with each other.
- **`withResource`**: guarantees worktree cleanup even if the refactor fails.
- **`bindInput`**: the worktree path is needed at multiple points (type-check, commit, etc.) — `bindInput` makes it available everywhere without threading.
- **Adversarial loop**: a judge LLM reviews the refactor and requests revisions. The implementing agent never sees the judge's criteria.
- **Type-check/fix loop**: after every change, the type-check/fix loop runs until the code compiles cleanly.
