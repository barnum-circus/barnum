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
        "instructions": { "kind": "Inline", "value": "Analyze this code for potential issues. Return `[{\"kind\": \"Review\", \"value\": {\"issues\": [\"unused variable\", \"missing error handling\"]}}]`" }
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
        "instructions": { "kind": "Inline", "value": "Review these issues and suggest fixes. Return `[{\"kind\": \"Implement\", \"value\": {\"fixes\": [\"remove unused var x\", \"add try-catch\"]}}]`" }
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
        "instructions": { "kind": "Inline", "value": "Implement these fixes. Return `[]` when done." }
      },
      "next": []
    }
  ]
}
```

## Running

```js
import { BarnumConfig } from "@barnum/barnum";

BarnumConfig.fromConfig({
  entrypoint: "Analyze",
  steps: [
    {
      name: "Analyze",
      value_schema: {
        type: "object",
        required: ["file", "contents"],
        properties: {
          file: { type: "string" },
          contents: { type: "string" },
        },
      },
      action: {
        kind: "Pool",
        instructions: {
          kind: "Inline",
          value:
            'Analyze this code for potential issues. Return `[{"kind": "Review", "value": {"issues": ["unused variable", "missing error handling"]}}]`',
        },
      },
      next: ["Review"],
    },
    {
      name: "Review",
      value_schema: {
        type: "object",
        required: ["issues"],
        properties: {
          issues: { type: "array" },
        },
      },
      action: {
        kind: "Pool",
        instructions: {
          kind: "Inline",
          value:
            'Review these issues and suggest fixes. Return `[{"kind": "Implement", "value": {"fixes": ["remove unused var x", "add try-catch"]}}]`',
        },
      },
      next: ["Implement"],
    },
    {
      name: "Implement",
      value_schema: {
        type: "object",
        required: ["fixes"],
        properties: {
          fixes: { type: "array" },
        },
      },
      action: {
        kind: "Pool",
        instructions: {
          kind: "Inline",
          value: "Implement these fixes. Return `[]` when done.",
        },
      },
      next: [],
    },
  ],
})
  .run({
    entrypointValue:
      '{"file": "src/main.rs", "contents": "fn main() { println!(\\"hello\\"); }"}',
  })
  .on("exit", (code) => process.exit(code ?? 1));
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
