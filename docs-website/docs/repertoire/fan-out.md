# Fan-Out

Fan-out splits one task into multiple parallel tasks.

## Example: Parallel File Processing

```jsonc
{
  "entrypoint": "ListFiles",
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
        "instructions": { "inline": "Analyze this file. Return `[]` when done." }
      },
      "next": []
    }
  ]
}
```

## Running

```bash
barnum run --config config.json --pool agents
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

```jsonc
{
  "entrypoint": "Analyze",
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
        "instructions": { "inline": "Find all functions that need refactoring. Return one task per function: `[{\"kind\": \"Refactor\", \"value\": {\"function\": \"parse_config\"}}, {\"kind\": \"Refactor\", \"value\": {\"function\": \"validate_input\"}}]`" }
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
        "instructions": { "inline": "Refactor this function. Return `[]`." }
      },
      "next": []
    }
  ]
}
```

## Running

```bash
barnum run --config config.json --pool agents --entrypoint-value '{"file": "src/main.rs"}'
```

## Key Points

- Return an array with multiple tasks to fan out
- All fanned-out tasks run in parallel (up to `max_concurrency`)
- Each task is independent - failures don't affect siblings
