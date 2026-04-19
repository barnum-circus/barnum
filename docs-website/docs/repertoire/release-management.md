# Release Management

Automate the release process: aggregate changelogs from commits, determine the version bump, update version files, tag, and publish.

## Workflow

```ts
runPipeline(
  listCommitsSinceLastRelease
    .then(allObject({
      versionBump: iterate().map(classifyCommit).collect().then(determineVersionBump),
      changelog: generateChangelog,
    }))
    .then(applyVersionBump)
    .then(tryCatch(
      (throwError) => buildAndTest.unwrapOr(throwError).drop()
        .then(tagRelease)
        .then(publish),
      rollbackVersion,
    )),
);
```

## Stages

1. **List commits** — get all commits since the last release tag.
2. **Parallel analysis**:
   - **Classify commits** — each commit is categorized (feature, fix, breaking, chore). The results determine the version bump (major/minor/patch).
   - **Generate changelog** — an LLM writes human-readable release notes from the commit list.
3. **Apply version bump** — update `package.json`, `Cargo.toml`, or whatever version files exist.
4. **Build, test, tag, publish** — wrapped in `tryCatch` so failures roll back the version bump.

## Key points

- `all` runs version analysis and changelog generation concurrently — they don't depend on each other.
- Commit classification is per-commit via `.iterate().map()`, so the agent sees one commit at a time.
- The changelog agent sees all commits but doesn't classify them — it writes prose.
- `tryCatch` ensures a failed publish doesn't leave the repo in a half-released state.
