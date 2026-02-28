# ListFiles

List all source files in the given folder.

## Input

```json
{"folder": "./src"}
```

## Output

Return an array of AnalyzeFile tasks, one per file:

```json
[{"kind": "AnalyzeFile", "value": {"file": "src/main.rs"}}, ...]
```
