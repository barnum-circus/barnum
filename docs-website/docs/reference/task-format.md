# Task Format

Barnum steps have two action types: **Pool** (dispatched to an agent) and **Command** (executed locally as a shell script). This document describes the JSON each receives and what it must return.

For how agents interact with the pool (get_task, heartbeat, response files), see [Agent Protocol](agent-protocol.md). For how tasks get submitted to the pool, see [Submission Protocol](submission-protocol.md).

## Response Format (Both Protocols)

Both agents and commands must produce a JSON array of next tasks on stdout. Each element has `kind` (the next step name) and `value` (the payload for that step):

```jsonc
[
  { "kind": "ProcessFile", "value": { "file": "src/main.rs" } },
  { "kind": "ProcessFile", "value": { "file": "src/lib.rs" } }
]
```

Return `[]` to end the chain (terminal step, no further work).

---

## Agent Protocol

When a step uses `"kind": "Pool"`, Barnum submits a task to the agent pool. The agent receives a JSON payload with three fields:

```jsonc
{
  // The task to process.
  "task": {
    "kind": "AnalyzeFile",
    "value": { "file": "src/main.rs" }
  },

  // Markdown instructions generated from the step's config.
  // Includes the step's instructions, valid next steps, and schemas.
  "instructions": "**IMPORTANT: This task is completely isolated...",

  // Optional timeout in seconds.
  "timeout_seconds": 86400
}
```

### What the instructions contain

Barnum auto-generates the `instructions` field from the config. It includes:

1. **Isolation preamble** — tells the agent it has no memory of prior tasks
2. **Step name header** — `# Current Step: AnalyzeFile`
3. **Your instructions** — the text from the step's `instructions` field
4. **Valid responses** — lists each valid next step with its schema

For a non-terminal step, the generated instructions look like:

```markdown
**IMPORTANT: This task is completely isolated. You have no memory
of previous tasks. Even if this task seems related to prior work,
you must complete it from scratch using only the information
provided here.**

# Current Step: AnalyzeFile

Analyze this file for issues. Return findings as a list of tasks.

## Valid Responses

You must return a JSON array of tasks. Each task has `kind` and
`value` fields.

Valid next steps:

### Categorize

Value must match schema:

```json
{
  "type": "object",
  "required": ["findings"],
  "properties": {
    "findings": { "type": "array" }
  }
}
```

Example:
```json
{"kind": "Categorize", "value": {...}}
```
```

For a terminal step, instead of the "Valid Responses" section, the agent sees:

```markdown
## Terminal Step

This is a terminal step. Return an empty array: `[]`
```

### Agent response

The agent writes its response as a JSON array string:

```jsonc
[
  { "kind": "Categorize", "value": { "findings": ["unused import", "dead code"] } }
]
```

Barnum validates the response:
- Must be a valid JSON array
- Each task's `kind` must be in the step's `next` list
- Each task's `value` must match the target step's `value_schema` (if defined)

Invalid responses trigger the retry policy.

---

## Command Protocol

When a step uses `"kind": "Command"`, Barnum executes the script locally via `sh -c`.

### Stdin

The command receives a JSON object on stdin:

```jsonc
{
  "kind": "ListFiles",
  "value": { "directory": "src" }
}
```

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
    "kind": "Command",
    // Fan out: take an array of items and emit one task per item.
    "script": "jq -c '.value.items[] | {kind: \"Process\", value: .}' | jq -s"
  },
  "next": ["Process"]
}
```

The `jq -c` produces one compact JSON object per line, and `| jq -s` collects them into an array.

### Stdout

The command must print a JSON array of next tasks to stdout:

```jsonc
[
  { "kind": "ProcessFile", "value": { "file": "src/main.rs" } },
  { "kind": "ProcessFile", "value": { "file": "src/lib.rs" } }
]
```

### Exit codes

- **exit 0** — success, stdout is parsed as the response
- **exit non-zero** — error, triggers the retry policy

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

## Comparison

| | Agent (Pool) | Command |
|---|---|---|
| **Receives** | JSON payload with `task`, `instructions`, `timeout_seconds` | Task JSON on stdin (`kind` + `value`) |
| **Instructions** | Auto-generated markdown with schemas and valid transitions | N/A (script is the logic) |
| **Returns** | JSON array string | JSON array on stdout |
| **Validation** | `kind` checked against `next`, `value` checked against `value_schema` | Same |
| **On failure** | Retry policy applies | Retry on non-zero exit |
| **Runs** | In agent pool (remote worker) | Locally via `sh -c` |
