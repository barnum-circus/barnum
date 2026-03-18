---
image: /img/og/repertoire-hooks.png
---

# Pre/Post Hooks

Hooks are shell commands that run before and after each task's action.

## Lifecycle

Each task goes through three phases, each with its own timeout:

```
┌─────────────────────────────────────────────────────────────┐
│                         Task Slot                           │
│  ┌──────────┐    ┌──────────┐    ┌───────────┐              │
│  │ Pre Hook │ →  │  Action  │ →  │ Post Hook │              │
│  │ timeout  │    │ timeout  │    │  timeout  │              │
│  └──────────┘    └──────────┘    └───────────┘              │
│     (max T)         (max T)         (max T)                 │
└─────────────────────────────────────────────────────────────┘
                    Total: up to 3T
```

All phases respect `max_concurrency` - a task holds its slot for the entire lifecycle.

## Pre Hooks

Pre hooks transform the input before it reaches the agent.

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
      // Add git context to the task value before the agent sees it.
      "pre": { "kind": "Command", "script": "jq '. + {git_branch: env.BRANCH, git_sha: env.SHA}'" },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Analyze this code with the enriched context. Return `[]`." }
      },
      "next": []
    }
  ]
}
```

## Running

```bash
barnum run --config config.json --entrypoint-value '{"file": "src/main.rs"}'
```

**Pre hook contract:**
- **stdin**: Task value as JSON
- **stdout**: Modified task value as JSON
- **exit 0**: Continue with modified value
- **exit non-zero**: Skip action, run post hook with `PreHookError`, then apply retry policy

> **Note:** Pre hook output is **not** re-validated against the step's `value_schema`. Adding fields is safe (JSON Schema allows extra properties by default), but removing required fields or changing types will pass silently. Keep pre hooks additive: enrich the value, don't reshape it.

## Post Hooks

Post hooks run after the action completes and can modify the results.

```jsonc
{
  "entrypoint": "Deploy",
  "steps": [
    {
      "name": "Deploy",
      "value_schema": {
        "type": "object",
        "required": ["version"],
        "properties": {
          "version": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Deploy the application. Return `[]`." }
      },
      // Log the deployment result to an external endpoint.
      "post": { "kind": "Command", "script": "INPUT=$(cat) && curl -s -X POST \"$LOG_ENDPOINT\" -d \"$INPUT\" > /dev/null && echo \"$INPUT\"" },
      "next": []
    }
  ]
}
```

**Post hook contract:**
- **stdin**: Result JSON (see below)
- **stdout**: Modified result JSON (same structure, can change `next`)
- **exit 0**: Use modified result
- **exit non-zero**: Apply retry policy

Post hooks receive and can modify:

**Success** - can modify next tasks:
```jsonc
{
  "kind": "Success",
  "input": {"file": "main.rs"},
  "output": {"result": "ok"},
  "next": [{"kind": "NextStep", "value": {"data": "example"}}]
}
```

**Timeout** - runs even on timeout:
```jsonc
{
  "kind": "Timeout",
  "input": {"file": "main.rs"}
}
```

**Error** - runs even on error:
```jsonc
{
  "kind": "Error",
  "input": {"file": "main.rs"},
  "error": "error message"
}
```

**PreHookError** - pre hook failed:
```jsonc
{
  "kind": "PreHookError",
  "input": {"file": "main.rs"},
  "error": "pre hook error message"
}
```

Example post hook that filters and transforms results:
```bash
#!/bin/bash
INPUT=$(cat)
KIND=$(echo "$INPUT" | jq -r '.kind')

if [ "$KIND" = "Success" ]; then
  # Filter next tasks, only keep high-priority ones
  echo "$INPUT" | jq '.next = [.next[] | select(.value.priority == "high")]'
else
  # Pass through unchanged
  echo "$INPUT"
fi
```

Example post hook that adds logging:
```bash
#!/bin/bash
INPUT=$(cat)
KIND=$(echo "$INPUT" | jq -r '.kind')

# Log to external system
curl -X POST "$LOG_ENDPOINT" -d "$INPUT"

# Pass through unchanged (or with modifications)
echo "$INPUT"
```

## Use Cases

**Pre hooks:**
- Fetch additional context (git info, environment)
- Read files referenced in the task
- Validate or sanitize input
- Add timestamps or request IDs
- Run setup commands (`yarn install`)

**Post hooks:**
- Filter or transform next tasks
- Add additional tasks to the response
- Send notifications (Slack, email)
- Log to external systems
- Update dashboards/metrics
- Run cleanup commands (`yarn tsc` to verify)
- Convert errors to recovery tasks

## Retry Behavior

Hooks follow the same retry policy as actions:

| Phase | Failure | Behavior |
|-------|---------|----------|
| Pre hook | Exit non-zero | Skip action, run post hook with `PreHookError`, retry if policy allows |
| Action | Timeout/error | Run post hook with error kind, retry if policy allows |
| Post hook | Exit non-zero | Retry entire task (pre + action + post) if policy allows |

## Finally Hook

The `finally` hook runs after ALL descendants of a task complete (not just direct children).

```jsonc
{
  "entrypoint": "AnalyzeAll",
  "steps": [
    {
      "name": "AnalyzeAll",
      "value_schema": {
        "type": "object",
        "required": ["files"],
        "properties": {
          "files": { "type": "array", "items": { "type": "string" } }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Fan out to analyze each file. Return `[{\"kind\": \"AnalyzeFile\", \"value\": {\"file\": \"src/main.rs\"}}]`" }
      },
      "next": ["AnalyzeFile"],
      // After all analyses complete, emit a summary task.
      "finally": { "kind": "Command", "script": "echo '[{\"kind\": \"Summarize\", \"value\": {\"status\": \"all files analyzed\"}}]'" }
    },
    {
      "name": "AnalyzeFile",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Analyze this file. Return `[]`." }
      },
      "next": []
    },
    {
      "name": "Summarize",
      "value_schema": {
        "type": "object",
        "required": ["status"],
        "properties": {
          "status": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Summarize the analysis results. Return `[]`." }
      },
      "next": []
    }
  ]
}
```

**Finally hook contract:**
- **stdin**: Original task value JSON (the value of the task that had `finally`)
- **stdout**: Array of next tasks (spawns follow-up work)
- Runs even if some descendants failed
- Failure is logged but doesn't prevent the workflow from continuing

**Use cases:**
- Aggregate results after fan-out completes
- Cleanup temp directories created for a batch
- Trigger follow-up work (categorization, prioritization)
- Send completion notifications

See [fan-out-finally.md](fan-out-finally.md) for a complete pattern.

## Key Points

- Each phase has its own timeout (up to 3x total)
- All phases respect `max_concurrency`
- Post hooks can modify `next` tasks
- Post hooks run even on timeout/error
- `finally` runs after all descendants complete
- `finally` can spawn follow-up tasks
- Hook failures trigger the retry policy
- All hooks have access to environment variables
