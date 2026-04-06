# Onboarding Automation

Analyze a repository, generate setup guides, and verify the instructions actually work by running them in a clean environment.

## Workflow

```ts
runPipeline(
  pipe(
    all(
      analyzeRepoStructure,
      analyzeDevDependencies,
      analyzeBuildSystem,
    ),
    merge(),
    generateSetupGuide,
    loop((recur) =>
      pipe(testInCleanEnv, classifyResult).branch({
        Works: drop,
        Broken: pipe(fixGuide, recur),
      })
    ),
  ),
);
```

## Stages

1. **Parallel analysis** — examine the repo structure, dev dependencies, and build system concurrently.
2. **Generate guide** — write step-by-step setup instructions from the analysis.
3. **Verify** — run the guide's instructions in a clean environment. If they fail, fix the guide and try again.

## Key points

- The verification loop catches stale or incorrect instructions before a new hire encounters them.
- Each analysis agent has focused scope — the dependency analyzer doesn't look at the build system.
- Consider running the verification step inside `withResource` with a disposable container or VM.
