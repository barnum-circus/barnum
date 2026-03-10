# ProcessRefactorList

Apply the FIRST refactor from the list to the file.

## Input

```json
{"file": "src/main.rs", "refactors": ["rename 'x' to 'user_count'", "rename 'tmp' to 'cached_result'"]}
```

## Rules

- **Never create new files.** Only modify the existing file.
- **No behavior changes.** The refactor should be purely cosmetic (renaming, readability).

## Output

After applying the first refactor:

- If more refactors remain:
  ```json
  [{"kind": "ProcessRefactorList", "value": {"file": "src/main.rs", "refactors": ["rename 'tmp' to 'cached_result'"]}}]
  ```

- If no refactors remain:
  ```json
  [{"kind": "CommitFile", "value": {"file": "src/main.rs"}}]
  ```

This ensures refactors for the same file are applied sequentially.
