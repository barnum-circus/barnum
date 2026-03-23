---
image: /img/og/repertoire-branching.png
---

# Branching

Branching allows agents to choose different paths based on their analysis.

## Example: Approval Workflow

```jsonc
{
  "entrypoint": "Review",
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
        "instructions": { "kind": "Inline", "value": "Review this PR. If it looks good, return `[{\"kind\": \"Approve\", \"value\": {\"pr_number\": 123}}]`. If changes are needed, return `[{\"kind\": \"RequestChanges\", \"value\": {\"pr_number\": 123, \"comments\": [\"fix typo\"]}}]`." }
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
        "instructions": { "kind": "Inline", "value": "Merge the PR. Return `[]`." }
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
        "instructions": { "kind": "Inline", "value": "Comment on the PR with requested changes. Return `[]`." }
      },
      "next": []
    }
  ]
}
```

## Running

```js
import { barnumRun } from "@barnum/barnum";

barnumRun({
  config: "config.json",
  entrypointValue: '{"pr_number": 123}',
}).on("exit", (code) => process.exit(code ?? 1));
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
