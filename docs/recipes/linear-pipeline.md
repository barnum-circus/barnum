# Linear Pipeline

A linear pipeline processes data through a sequence of steps.

## Example: Code Review Pipeline

```json
{
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
        "instructions": "Analyze this code for potential issues. Return `[{\"kind\": \"Review\", \"value\": {\"issues\": [...]}}]`"
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
        "instructions": "Review these issues and suggest fixes. Return `[{\"kind\": \"Implement\", \"value\": {\"fixes\": [...]}}]`"
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
        "instructions": "Implement these fixes. Return `[]` when done."
      },
      "next": []
    }
  ]
}
```

## Initial Tasks

To start the pipeline:

```bash
gsd run config.json --pool agents --initial '[{"kind": "Analyze", "value": {"file": "src/main.rs", "contents": "fn main() { ... }"}}]'
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
