# Config Schema Reference

Barnum workflows are defined in JSON (or JSONC) config files. This page documents all available fields.

Run `barnum config schema` to get the full JSON Schema for editor validation.

## Top-Level Structure

```json
{
  "$schema": "https://...",
  "entrypoint": "StepName",
  "options": { ... },
  "steps": [ ... ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `$schema` | string | No | JSON Schema URL for editor validation |
| `entrypoint` | string | No | Entry point step name. If set, use `--entrypoint-value` instead of `--initial-state` |
| `options` | object | No | Global runtime options |
| `steps` | array | **Yes** | Array of step definitions |

## Options

Global options that apply to all steps (can be overridden per-step).

```json
{
  "options": {
    "timeout": 120,
    "max_retries": 3,
    "max_concurrency": 5,
    "retry_on_timeout": true,
    "retry_on_invalid_response": true
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `timeout` | integer | null | Timeout in seconds per task (null = no timeout) |
| `max_retries` | integer | 0 | Maximum retry attempts per task |
| `max_concurrency` | integer | null | Maximum concurrent tasks (null = unlimited) |
| `retry_on_timeout` | boolean | true | Retry when agent times out |
| `retry_on_invalid_response` | boolean | true | Retry when agent returns invalid response |

## Steps

Each step defines a stage in your workflow.

```json
{
  "steps": [
    {
      "name": "Analyze",
      "value_schema": { "type": "object" },
      "action": { "kind": "Pool", "instructions": "..." },
      "pre": { "link": "scripts/pre.sh" },
      "post": { "link": "scripts/post.sh" },
      "finally": { "link": "scripts/finally.sh" },
      "next": ["Review", "Implement"],
      "options": { "timeout": 300 }
    }
  ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | **Yes** | Unique step identifier |
| `value_schema` | object | No | JSON Schema to validate task values |
| `action` | object | No | How to execute the step (Pool or Command) |
| `pre` | object | No | Pre-execution hook (`{"inline": "..."}` or `{"link": "path"}`) |
| `post` | object | No | Post-execution hook (`{"inline": "..."}` or `{"link": "path"}`) |
| `finally` | object | No | Finally hook (`{"inline": "..."}` or `{"link": "path"}`) |
| `next` | array | No | Valid next step names (empty = terminal step) |
| `options` | object | No | Per-step options override |

## Actions

### Pool Action

Send the task to an agent in the pool.

```json
{
  "action": {
    "kind": "Pool",
    "instructions": "Analyze the code and return findings. Return `[]` when done."
  }
}
```

Instructions can be inline or linked to a file:

```json
{
  "action": {
    "kind": "Pool",
    "instructions": { "link": "instructions/analyze.md" }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `kind` | `"Pool"` | Send to agent pool |
| `instructions` | string or object | Markdown instructions for agents |

### Command Action

Run a local shell command instead of sending to an agent.

```json
{
  "action": {
    "kind": "Command",
    "script": "jq -r '.value.path' | xargs cat | jq '{kind: \"Process\", value: .}' | jq -s"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `kind` | `"Command"` | Run locally |
| `script` | string | Shell script to execute |

**Command contract:**
- **stdin**: Task JSON (`{"kind": "StepName", "value": {...}}`)
- **stdout**: Response JSON (array of next tasks)
- **exit 0**: Success
- **exit non-zero**: Error, triggers retry

## Value Schema

Validate task payloads with JSON Schema.

```json
{
  "value_schema": {
    "type": "object",
    "required": ["file", "action"],
    "properties": {
      "file": { "type": "string" },
      "action": { "type": "string", "enum": ["read", "write"] }
    }
  }
}
```

Or link to an external schema file:

```json
{
  "value_schema": { "link": "schemas/task.json" }
}
```

## Per-Step Options

Override global options for specific steps.

```json
{
  "options": {
    "timeout": 600,
    "max_retries": 5,
    "retry_on_timeout": false,
    "retry_on_invalid_response": false
  }
}
```

All fields are optional and override the corresponding global option.

## Hooks

### Pre Hook

Runs before the action. Transforms input.

- **stdin**: Task value JSON
- **stdout**: Modified task value JSON
- **exit 0**: Continue with modified value
- **exit non-zero**: Skip action, run post hook with `PreHookError`

### Post Hook

Runs after the action. Can modify results.

- **stdin**: Result JSON (see below)
- **stdout**: Modified result JSON
- **exit 0**: Use modified result
- **exit non-zero**: Apply retry policy

Result kinds:
- `Success` - Agent completed, can modify `next` array
- `Timeout` - Agent timed out
- `Error` - Agent returned error
- `PreHookError` - Pre hook failed

### Finally Hook

Runs after ALL descendants complete.

- **stdin**: Original task value JSON
- **stdout**: Array of next tasks to spawn
- Runs even if descendants failed

## Complete Example

```json
{
  "$schema": "https://example.com/barnum-config-schema.json",
  "options": {
    "timeout": 120,
    "max_retries": 2,
    "max_concurrency": 5
  },
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
      "pre": { "link": "scripts/read-file.sh" },
      "action": {
        "kind": "Pool",
        "instructions": "Analyze this code. Return `[{\"kind\": \"Implement\", \"value\": {\"changes\": []}}]`"
      },
      "post": { "link": "scripts/validate-response.sh" },
      "next": ["Implement"]
    },
    {
      "name": "Implement",
      "value_schema": {
        "type": "object",
        "required": ["changes"],
        "properties": {
          "changes": { "type": "array" }
        }
      },
      "options": {
        "timeout": 300,
        "max_retries": 0
      },
      "action": {
        "kind": "Pool",
        "instructions": "Apply these changes. Return `[]` when done."
      },
      "next": []
    }
  ]
}
```
