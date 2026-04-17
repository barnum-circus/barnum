# Parallel Execution

Barnum has three ways to run work concurrently: `all` for a fixed set of tasks, `forEach` for dynamic fan-out over arrays, and `forEach` + `.then()` for fan-out with a follow-up aggregation step.

## all — fixed parallel tasks

`all` runs multiple actions concurrently with the same input and collects their results as a tuple.

From [`demos/analyze-file/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/analyze-file/run.ts):

```ts
runPipeline(
  all(analyzeClassComponents, analyzeImpossibleStates, analyzeErrorHandling),
  "source/index.ts",
);
```

All three analyzers receive `"source/index.ts"` as input and run in parallel. The output is a tuple `[ResultA, ResultB, ResultC]`.

## forEach — dynamic fan-out

`forEach` maps an action over each element of an array, processing all elements concurrently.

From [`demos/simple-workflow/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/simple-workflow/run.ts):

```ts
runPipeline(
  listFiles.forEach(
    implementRefactor
      .then(typeCheckFiles)
      .then(fixTypeErrors)
      .then(commitChanges)
      .then(createPullRequest),
  ),
);
```

`listFiles` returns `string[]`. Each filename flows through the full pipeline independently and concurrently.

## Fan-out with aggregation

Chain `forEach` into a follow-up step to aggregate results after parallel work completes.

From [`demos/convert-folder-to-ts/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/convert-folder-to-ts/run.ts):

```ts
runPipeline(
  setup
    .then(listFiles.forEach(migrate({ to: "Typescript" })).drop())
    .then(typeCheckFix),
);
```

All files are migrated in parallel. After every migration finishes, `.drop()` clears the array and `typeCheckFix` runs once — a single type-check pass over the entire project, not per file.
