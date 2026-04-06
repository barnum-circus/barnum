# Test Generation

Analyze source files, generate test suites, run them, and iterate on failures until the tests pass.

## Workflow

```ts
runPipeline(
  pipe(
    listSourceFiles,
    forEach(
      pipe(
        analyzeForTestability,
        generateTests,
        loop((recur) =>
          pipe(runTests, classifyResults).branch({
            Passing: drop,
            Failing: pipe(fixTests, recur),
          })
        ),
      )
    ),
  ).drop(),
);
```

## Stages

1. **List source files** — find files that need test coverage.
2. **For each file** (concurrently):
   - **Analyze** — identify functions, edge cases, and testable behaviors.
   - **Generate tests** — write a test file based on the analysis.
   - **Run and iterate** — run the tests. If any fail, fix them and re-run. Loop until all pass.

## Key points

- The analysis agent focuses purely on identifying what to test — it doesn't write tests.
- The test-writing agent receives the analysis output, not the analysis instructions. Focused context.
- The fix-and-retry loop is the same `loop` + `branch` pattern used in [type-check/fix](./codebase-migration.md).
- `forEach` processes all files concurrently, so test generation scales with the codebase.
- Consider adding `withTimeout` to cap how long the fix loop runs per file.
