---
image: /img/og/repertoire-commands.png
---

# Local Commands

Use shell commands instead of agents for deterministic or system-level operations.

## When to Use Commands

An agent *can* do anything a command does (list files, call APIs, run builds). But agents are expensive: they consume an LLM call, need time to reason, and can hallucinate. Commands are instant, deterministic, and free.

Use commands when:

- **The logic is deterministic**: listing files, parsing JSON, calling an API with known parameters. There's no judgment involved, so there's nothing for an LLM to add.
- **Precision matters more than flexibility**: a `jq` pipeline that reshapes data will get it right every time. An agent doing the same transformation might subtly alter values or miss edge cases.
- **The operation is a building block, not a decision**: fan-out (splitting a list into tasks), fan-in (aggregating results), and data plumbing are mechanical. Save the agent for the step that requires reasoning.

Use agents when the task requires judgment, creativity, or understanding natural language. A good workflow mixes both: commands handle the plumbing, agents handle the thinking.

## Basic Command

```jsonc
{
  "entrypoint": "ListFiles",
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
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "Analyze this file. Return `[]`." }
      },
      "next": []
    }
  ]
}
```

## Running

```bash
barnum run --config config.json
```

## Command Contract

- **stdin**: Task JSON (`{"kind": "StepName", "value": {"key": "value"}}`)
- **stdout**: Response JSON (array of next tasks)
- **exit 0**: Success
- **exit non-zero**: Error, triggers retry

## Use Cases

**File operations:**
```jsonc
{
  "kind": "Command",
  "script": "jq -r '.value.path' | xargs cat | jq -Rs '{kind: \"Process\", value: {contents: .}}'"
}
```

**API calls:**
```jsonc
{
  "kind": "Command",
  "script": "jq -r '.value.url' | xargs curl -s | jq '{kind: \"Parse\", value: .}' | jq -s"
}
```

**Build/test:**
```jsonc
{
  "kind": "Command",
  "script": "cargo test --json 2>&1 | jq -s 'map(select(.type == \"test\")) | map({kind: \"Report\", value: .})'"
}
```

## Mixing Commands and Agents

Commands and agents work together naturally:

```jsonc
{
  "entrypoint": "Plan",
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
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "Plan the implementation. Return `[{\"kind\": \"Execute\", \"value\": {\"command\": \"echo hello\"}}]`" }
      },
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
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "Verify the changes. Return `[]`." }
      },
      "next": []
    }
  ]
}
```

## Running

```bash
barnum run --config config.json --entrypoint-value '{"task": "Add logging"}'
```

## Key Points

- Commands run locally on the host machine
- Commands are async (don't block other tasks)
- Commands respect `max_concurrency`
- Use `jq` for JSON manipulation
- Always output valid JSON array
