---
image: /img/og/repertoire-linear-pipeline.png
---

# Linear Pipeline

A linear pipeline processes data through a sequence of steps.

## Example: Code Review Pipeline

```jsonc
{
  "entrypoint": "Analyze",
  "steps": [
    {
      "name": "Analyze",
      "value_schema": {
        "type": "object",
        "required": ["file", "contents"],
        "properties": {
          "file": { "type": "string" },
          "contents": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Analyze this code for potential issues. Return `[{\"kind\": \"Review\", \"value\": {\"issues\": [\"unused variable\", \"missing error handling\"]}}]`" }
      },
      "next": ["Review"]
    },
    {
      "name": "Review",
      "value_schema": {
        "type": "object",
        "required": ["issues"],
        "properties": {
          "issues": { "type": "array" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Review these issues and suggest fixes. Return `[{\"kind\": \"Implement\", \"value\": {\"fixes\": [\"remove unused var x\", \"add try-catch\"]}}]`" }
      },
      "next": ["Implement"]
    },
    {
      "name": "Implement",
      "value_schema": {
        "type": "object",
        "required": ["fixes"],
        "properties": {
          "fixes": { "type": "array" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Implement these fixes. Return `[]` when done." }
      },
      "next": []
    }
  ]
}
```

## Running

```bash
barnum run --config config.json --entrypoint-value '{"file": "src/main.rs", "contents": "fn main() { println!(\"hello\"); }"}'
```

## Flow

```
Analyze → Review → Implement → (done)
```

Each step receives the output from the previous step as its input value.

## Key Points

- Terminal steps have `"next": []`
- Each agent response is an array of next tasks
- Return `[]` to end the workflow
