# Branching

Branching allows agents to choose different paths based on their analysis.

## Example: Approval Workflow

```json
{
  "steps": [
    {
      "name": "Review",
      "value_schema": {
        "type": "object",
        "required": ["pr_number"],
        "properties": {
          "pr_number": { "type": "integer" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Review this PR. If it looks good, return `[{\"kind\": \"Approve\", \"value\": {\"pr_number\": 123}}]`. If changes are needed, return `[{\"kind\": \"RequestChanges\", \"value\": {\"pr_number\": 123, \"comments\": [\"fix typo\"]}}]`."
      },
      "next": ["Approve", "RequestChanges"]
    },
    {
      "name": "Approve",
      "value_schema": {
        "type": "object",
        "required": ["pr_number"],
        "properties": {
          "pr_number": { "type": "integer" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Merge the PR. Return `[]`."
      },
      "next": []
    },
    {
      "name": "RequestChanges",
      "value_schema": {
        "type": "object",
        "required": ["pr_number", "comments"],
        "properties": {
          "pr_number": { "type": "integer" },
          "comments": { "type": "array" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Comment on the PR with requested changes. Return `[]`."
      },
      "next": []
    }
  ]
}
```

## Initial Tasks

```bash
gsd run config.json --pool agents --initial-state '[{"kind": "Review", "value": {"pr_number": 123}}]'
```

## Flow

```
        ┌─→ Approve → (done)
Review ─┤
        └─→ RequestChanges → (done)
```

## Key Points

- The `next` array lists ALL valid transitions from a step
- The agent's response determines which path is taken
- Agents can only transition to steps listed in `next`
- Invalid transitions cause retries (configurable)
