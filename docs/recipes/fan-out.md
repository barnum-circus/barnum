# Fan-Out

Fan-out splits one task into multiple parallel tasks.

## Example: Parallel File Processing

```json
{
  "steps": [
    {
      "name": "ListFiles",
      "value_schema": {
        "type": "object",
        "properties": {
          "directory": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        "script": "find src -name '*.rs' | jq -R -s 'split(\"\\n\") | map(select(length > 0)) | map({kind: \"ProcessFile\", value: {path: .}})'"
      },
      "next": ["ProcessFile"]
    },
    {
      "name": "ProcessFile",
      "value_schema": {
        "type": "object",
        "required": ["path"],
        "properties": {
          "path": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Analyze this file. Return `[]` when done."
      },
      "next": []
    }
  ]
}
```

## Initial Tasks

```bash
gsd run config.json --pool agents --initial '[{"kind": "ListFiles", "value": {}}]'
```

## Flow

```
              ┌─→ ProcessFile (file1.rs)
              │
ListFiles ────┼─→ ProcessFile (file2.rs)
              │
              └─→ ProcessFile (file3.rs)
```

## Agent Fan-Out

Agents can also fan out by returning multiple tasks:

```json
{
  "steps": [
    {
      "name": "Analyze",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Find all functions that need refactoring. Return one task per function: `[{\"kind\": \"Refactor\", \"value\": {\"function\": \"...\"}}, ...]`"
      },
      "next": ["Refactor"]
    },
    {
      "name": "Refactor",
      "value_schema": {
        "type": "object",
        "required": ["function"],
        "properties": {
          "function": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Refactor this function. Return `[]`."
      },
      "next": []
    }
  ]
}
```

## Initial Tasks

```bash
gsd run config.json --pool agents --initial '[{"kind": "Analyze", "value": {"file": "src/main.rs"}}]'
```

## Key Points

- Return an array with multiple tasks to fan out
- All fanned-out tasks run in parallel (up to `max_concurrency`)
- Each task is independent - failures don't affect siblings
