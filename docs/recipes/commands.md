# Local Commands

Use shell commands instead of agents for deterministic or system-level operations.

## Basic Command

```json
{
  "steps": [
    {
      "name": "ListFiles",
      "value_schema": { "type": "object" },
      "action": {
        "kind": "Command",
        "script": "find . -name '*.rs' | jq -R -s 'split(\"\\n\") | map(select(. != \"\")) | map({kind: \"Analyze\", value: {file: .}})'"
      },
      "next": ["Analyze"]
    },
    {
      "name": "Analyze",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string" }
        }
      },
      "action": { "kind": "Pool", "instructions": "Analyze this file. Return `[]`." },
      "next": []
    }
  ]
}
```

## Initial Tasks

```bash
gsd run config.json --pool agents --initial-state '[{"kind": "ListFiles", "value": {}}]'
```

## Command Contract

- **stdin**: Task JSON (`{"kind": "StepName", "value": {"key": "value"}}`)
- **stdout**: Response JSON (array of next tasks)
- **exit 0**: Success
- **exit non-zero**: Error, triggers retry

## Use Cases

**File operations:**
```json
{
  "kind": "Command",
  "script": "jq -r '.value.path' | xargs cat | jq -Rs '{kind: \"Process\", value: {contents: .}}'"
}
```

**API calls:**
```json
{
  "kind": "Command",
  "script": "jq -r '.value.url' | xargs curl -s | jq '{kind: \"Parse\", value: .}' | jq -s"
}
```

**Build/test:**
```json
{
  "kind": "Command",
  "script": "cargo test --json 2>&1 | jq -s 'map(select(.type == \"test\")) | map({kind: \"Report\", value: .})'"
}
```

## Mixing Commands and Agents

Commands and agents work together naturally:

```json
{
  "steps": [
    {
      "name": "Plan",
      "value_schema": {
        "type": "object",
        "required": ["task"],
        "properties": {
          "task": { "type": "string" }
        }
      },
      "action": { "kind": "Pool", "instructions": "Plan the implementation. Return `[{\"kind\": \"Execute\", \"value\": {\"command\": \"echo hello\"}}]`" },
      "next": ["Execute"]
    },
    {
      "name": "Execute",
      "value_schema": {
        "type": "object",
        "required": ["command"],
        "properties": {
          "command": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        "script": "jq -r '.value.command' | sh && echo '[{\"kind\": \"Verify\", \"value\": {}}]'"
      },
      "next": ["Verify"]
    },
    {
      "name": "Verify",
      "value_schema": { "type": "object" },
      "action": { "kind": "Pool", "instructions": "Verify the changes. Return `[]`." },
      "next": []
    }
  ]
}
```

## Initial Tasks

```bash
gsd run config.json --pool agents --initial-state '[{"kind": "Plan", "value": {"task": "Add logging"}}]'
```

## Key Points

- Commands run locally on the host machine
- Commands are async (don't block other tasks)
- Commands respect `max_concurrency`
- Use `jq` for JSON manipulation
- Always output valid JSON array
