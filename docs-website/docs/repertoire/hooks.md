---
image: /img/og/repertoire-hooks.png
---

# Finally Hooks

The `finally` hook runs after a task **and all its descendants** complete (not just direct children).

## When It Runs

```
Task A runs → spawns children B, C
  B completes
  C completes
  → A's finally hook runs
```

The finally hook waits for the entire subtree to finish — including grandchildren, retried tasks, and tasks spawned by other finally hooks.

## Example

```jsonc
{
  "entrypoint": "AnalyzeAll",
  "steps": [
    {
      "name": "AnalyzeAll",
      "value_schema": {
        "type": "object",
        "required": ["files"],
        "properties": {
          "files": { "type": "array", "items": { "type": "string" } }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "Fan out to analyze each file. Return `[{\"kind\": \"AnalyzeFile\", \"value\": {\"file\": \"src/main.rs\"}}]`" }
      },
      "next": ["AnalyzeFile"],
      // After all analyses complete, emit a summary task.
      "finally": { "kind": "Command", "script": "echo '[{\"kind\": \"Summarize\", \"value\": {\"status\": \"all files analyzed\"}}]'" }
    },
    {
      "name": "AnalyzeFile",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "Analyze this file. Return `[]`." }
      },
      "next": []
    },
    {
      "name": "Summarize",
      "value_schema": {
        "type": "object",
        "required": ["status"],
        "properties": {
          "status": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "Summarize the analysis results. Return `[]`." }
      },
      "next": []
    }
  ]
}
```

## Contract

- **stdin**: Task JSON (`{"kind": "StepName", "value": {...}}`) — same envelope format as command actions
- **stdout**: JSON array of follow-up tasks to spawn: `[{"kind": "StepName", "value": {...}}, ...]`
- Return `[]` to spawn no follow-ups
- Runs even if some descendants failed
- Failure is logged but doesn't prevent the workflow from continuing

## Use Cases

- Aggregate results after fan-out completes
- Cleanup temp directories created for a batch
- Trigger follow-up work (categorization, prioritization)
- Send completion notifications

See [fan-out-finally.md](fan-out-finally.md) for a complete pattern.

## Key Points

- `finally` runs after **all descendants** complete, not just direct children
- `finally` can spawn follow-up tasks (which themselves can have `finally` hooks)
- Tasks spawned by `finally` are tracked under the grandparent
- All hooks have access to environment variables
