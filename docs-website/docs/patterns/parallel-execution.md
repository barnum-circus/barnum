# Parallel Execution

Barnum has three ways to run work concurrently: `all` for a fixed set of tasks, `Iterator.map` for dynamic fan-out over arrays, and `Iterator.flatMap` for fan-out with flattening.

## all — fixed parallel tasks

`all` runs multiple actions concurrently with the same input and collects their results as a tuple.

From [`demos/analyze-file/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/analyze-file/run.ts):

```ts
all(analyzeClassComponents, analyzeImpossibleStates, analyzeErrorHandling)
  .call(constant("source/index.ts"))
  .run();
```

All three analyzers receive `"source/index.ts"` as input and run in parallel. The output is a tuple `[ResultA, ResultB, ResultC]`.

## Iterator.map — dynamic fan-out

`.iterate()` wraps an array as an Iterator. `.map(action)` applies an action to each element concurrently. `.collect()` gathers results back into an array.

From [`demos/simple-workflow/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/simple-workflow/run.ts):

```ts
listFiles
    .iterate()
    .map(
      implementRefactor
        .then(typeCheckFiles)
        .then(fixTypeErrors)
        .then(commitChanges)
        .then(createPullRequest),
    )
    .collect().run();
```

`listFiles` returns `string[]`. `.iterate()` enters Iterator, `.map()` fans out — each filename flows through the full pipeline independently and concurrently. `.collect()` gathers results.

## Fan-out with aggregation

Chain `.collect()` into a follow-up step to aggregate results after parallel work completes.

From [`demos/convert-folder-to-ts/run.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/convert-folder-to-ts/run.ts):

```ts
setup
    .then(
      listFiles.iterate().map(migrate({ to: "Typescript" })).collect().drop(),
    )
    .then(typeCheckFix).run();
```

All files are migrated in parallel. After every migration finishes, `.collect()` gathers results, `.drop()` discards them, and `typeCheckFix` runs once — a single type-check pass over the entire project, not per file.
