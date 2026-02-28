# AnalyzeFile

Analyze the file and identify refactors needed.

## Input

```json
{"file": "src/main.rs"}
```

## Refactor Guidelines

- **No behavior changes.** The code should do exactly the same thing before and after.
- **Focus on slight cleanups:** renaming variables to be more informatively named, improving readability.
- **Never create new files.** Only modify existing files.

## Output

If refactors are needed, return:

```json
[{"kind": "ProcessRefactorList", "value": {
  "file": "src/main.rs",
  "refactors": ["rename 'x' to 'user_count'", "rename 'tmp' to 'cached_result'"]
}}]
```

If no refactors needed, return `[]`.
