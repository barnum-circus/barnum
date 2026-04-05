---
image: /img/og/reference-task-format.png
---

# Task Format

Barnum steps use **Bash** actions (executed locally as a shell script). This document describes the JSON each receives and what it must return.

## Response Format

Scripts must produce a JSON array of next tasks on stdout. Each element has `kind` (the next step name) and `value` (the payload for that step):

```jsonc
[
  { "kind": "ProcessFile", "value": { "file": "src/main.rs" } },
  { "kind": "ProcessFile", "value": { "file": "src/lib.rs" } }
]
```

Return `[]` to end the chain (terminal step, no further work).

---

## Bash Action Protocol

When a step uses `"kind": "Bash"`, Barnum executes the script locally via `sh -c`.

### Stdin

The script receives a JSON envelope on stdin:

```jsonc
{
  "value": { "directory": "src" },
  "config": { /* the full resolved config */ },
  "stepName": "ListFiles"
}
```

| Field | Description |
|---|---|
| `value` | The task's payload (from the parent step's output) |
| `config` | The full resolved barnum config (for introspection) |
| `stepName` | The name of the step being executed |

### Extracting parameters with jq

Use `jq` to pull fields out of the task JSON:

```bash
#!/bin/bash
set -e

# Read stdin once, extract fields
INPUT=$(cat)
DIR=$(echo "$INPUT" | jq -r '.value.directory')
VERBOSE=$(echo "$INPUT" | jq -r '.value.verbose // false')
```

### Inline jq scripts

For simple transformations, the script can be an inline jq pipeline directly in the config:

```jsonc
{
  "name": "Split",
  "action": {
    "kind": "Bash",
    // Fan out: take an array of items and emit one task per item.
    "script": "jq -c '.value.items[] | {kind: \"Process\", value: .}' | jq -s"
  },
  "next": ["Process"]
}
```

The `jq -c` produces one compact JSON object per line, and `| jq -s` collects them into an array.

### Stdout

The script must print a JSON array of next tasks to stdout:

```jsonc
[
  { "kind": "ProcessFile", "value": { "file": "src/main.rs" } },
  { "kind": "ProcessFile", "value": { "file": "src/lib.rs" } }
]
```

### Exit codes

- **exit 0**: success, stdout is parsed as the response
- **exit non-zero**: error, triggers the retry policy

### Full script example

```bash
#!/bin/bash
set -e

INPUT=$(cat)
DIR=$(echo "$INPUT" | jq -r '.value.directory')

# Find Rust files and emit one ProcessFile task per file
find "$DIR" -name '*.rs' | jq -R -s '
  split("\n") |
  map(select(length > 0)) |
  map({ kind: "ProcessFile", value: { file: . } })
'
```

---

## Summary

| | Bash |
|---|---|
| **Receives** | JSON envelope on stdin (`value`, `config`, `stepName`) |
| **Returns** | JSON array on stdout |
| **Validation** | `kind` checked against `next`, `value` checked against `value_schema` |
| **On failure** | Retry on non-zero exit |
| **Runs** | Locally via `sh -c` |
