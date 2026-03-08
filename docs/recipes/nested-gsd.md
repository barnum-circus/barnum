# Nested GSD (Sub-Workflows)

Launch sub-workflows from within a GSD task queue.

## Why Nested GSD?

- **Scoped setup/cleanup**: Create temp resources, run tasks, then clean up
- **Fan-in**: Wait for all parallel tasks to complete before aggregating
- **Isolation**: Sub-workflow has its own config, retry policies, schemas
- **Composition**: Break complex workflows into reusable pieces
- **Dynamic generation**: Agents can generate configs for sub-workflows

## Primary Use Case: Scratchpad Pattern

Create a workspace, run independent tasks, aggregate results, clean up:

```
┌─────────────────────────────────────────────────────────────┐
│  Outer Workflow                                             │
│                                                             │
│  Setup ─────→ RunSubWorkflow ─────→ Aggregate ─────→ Done   │
│  (mkdir)      │                     (read results,          │
│               │                      cleanup)               │
│               ▼                                             │
│  ┌─────────────────────────────────────┐                    │
│  │  Inner Workflow (parallel tasks)    │                    │
│  │                                     │                    │
│  │  ProcessFile(1) ──┐                 │                    │
│  │  ProcessFile(2) ──┼──→ (all write   │                    │
│  │  ProcessFile(3) ──┘     to tmpdir)  │                    │
│  │                                     │                    │
│  └─────────────────────────────────────┘                    │
└─────────────────────────────────────────────────────────────┘
```

## Example: Scratchpad Workflow

Outer workflow config (`outer.json`):
```json
{
  "steps": [
    {
      "name": "Setup",
      "value_schema": {
        "type": "object",
        "required": ["files"],
        "properties": {
          "files": { "type": "array", "items": { "type": "string" } }
        }
      },
      "action": {
        "kind": "Command",
        "script": "scripts/setup-workspace.sh"
      },
      "next": ["RunAnalysis"]
    },
    {
      "name": "RunAnalysis",
      "value_schema": {
        "type": "object",
        "required": ["workspace"],
        "properties": {
          "workspace": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        "script": "scripts/run-analysis-workflow.sh"
      },
      "next": ["Aggregate"]
    },
    {
      "name": "Aggregate",
      "value_schema": {
        "type": "object",
        "required": ["workspace", "results"],
        "properties": {
          "workspace": { "type": "string" },
          "results": { "type": "array" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Read all results from the workspace directory and synthesize a summary. The results are in JSON files at the path provided. Return `[{\"kind\": \"Cleanup\", \"value\": {\"summary\": \"Found 3 issues\"}}]`"
      },
      "next": ["Cleanup"]
    },
    {
      "name": "Cleanup",
      "value_schema": {
        "type": "object",
        "required": ["summary"],
        "properties": {
          "summary": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        "script": "rm -rf \"$WORKSPACE\" && echo '[]'"
      },
      "next": []
    }
  ]
}
```

## Initial Tasks

```bash
gsd run --config outer.json --pool agents --initial-state '[{"kind": "Setup", "value": {"files": ["src/main.rs", "src/lib.rs"]}}]'
```

**scripts/setup-workspace.sh:**
```bash
#!/bin/bash
set -e
INPUT=$(cat)

# Create workspace
WORKSPACE=$(mktemp -d)
export WORKSPACE

# List files to analyze
FILES=$(echo "$INPUT" | jq -r '.value.files[]')

# Generate inner workflow config
cat > "$WORKSPACE/inner.json" << EOF
{
  "steps": [
    {
      "name": "AnalyzeFile",
      "post": "echo \"\$INPUT\" | jq '.output' > \"$WORKSPACE/results/\$(uuidgen).json\" && cat",
      "action": {"kind": "Pool", "instructions": "Analyze this file for issues. Return `[]`."},
      "next": []
    }
  ]
}
EOF

# Generate initial tasks
echo "$FILES" | jq -R -s 'split("\n") | map(select(. != "")) | map({kind: "AnalyzeFile", value: {file: .}})' > "$WORKSPACE/initial.json"

mkdir -p "$WORKSPACE/results"

# Pass workspace path to next step
echo "[{\"kind\": \"RunAnalysis\", \"value\": {\"workspace\": \"$WORKSPACE\"}}]"
```

**scripts/run-analysis-workflow.sh:**
```bash
#!/bin/bash
set -e
INPUT=$(cat)
WORKSPACE=$(echo "$INPUT" | jq -r '.value.workspace')

# Run inner workflow (blocks until complete)
gsd run --config "$WORKSPACE/inner.json" \
  --initial-state "$WORKSPACE/initial.json" \
  --pool "$POOL_ID"

# Collect results for aggregation step
RESULTS=$(cat "$WORKSPACE/results/"*.json | jq -s '.')

echo "[{\"kind\": \"Aggregate\", \"value\": {\"workspace\": \"$WORKSPACE\", \"results\": $RESULTS}}]"
```

## Example: Agent-Generated Sub-Workflow

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
      "action": {
        "kind": "Pool",
        "instructions": "Analyze the task and generate a GSD config for the sub-workflow. Return `[{\"kind\": \"RunSubWorkflow\", \"value\": {\"config\": {\"steps\": []}, \"initial_tasks\": []}}]`"
      },
      "next": ["RunSubWorkflow"]
    },
    {
      "name": "RunSubWorkflow",
      "value_schema": {
        "type": "object",
        "required": ["config", "initial_tasks"],
        "properties": {
          "config": { "type": "object" },
          "initial_tasks": { "type": "array" }
        }
      },
      "action": {
        "kind": "Command",
        "script": "scripts/run-sub-gsd.sh"
      },
      "next": ["Report"]
    },
    {
      "name": "Report",
      "value_schema": {
        "type": "object",
        "required": ["status"],
        "properties": {
          "status": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Summarize the sub-workflow results. Return `[]`."
      },
      "next": []
    }
  ]
}
```

**scripts/run-sub-gsd.sh:**
```bash
#!/bin/bash
set -e

INPUT=$(cat)
CONFIG=$(echo "$INPUT" | jq -r '.value.config')
INITIAL=$(echo "$INPUT" | jq -r '.value.initial_tasks')

# Write temp config
TMPDIR=$(mktemp -d)
echo "$CONFIG" > "$TMPDIR/config.json"
echo "$INITIAL" > "$TMPDIR/initial.json"

# Run sub-workflow (reusing same pool)
gsd run --config "$TMPDIR/config.json" --initial-state "$TMPDIR/initial.json" --pool "$POOL_ID"

# Return result
echo '[{"kind": "Report", "value": {"status": "completed"}}]'
```

## Pool Considerations

### Reusing the Same Pool

Pros:
- No setup overhead
- Agents already warm

Cons:
- Can exhaust agents if sub-workflow is large
- Parent workflow blocked waiting for agents

### Separate Pools

```bash
# Start dedicated pool for sub-workflow
SUB_POOL=$(agent_pool start --json | jq -r '.id')
# ... spawn agents for sub-pool ...
gsd run --config config.json --pool "$SUB_POOL" ...
agent_pool stop --pool "$SUB_POOL"
```

## Recursive Patterns

An agent could generate a config that itself spawns sub-workflows:

```
Plan
  └─→ RunSubWorkflow
        └─→ Plan (recursive)
              └─→ RunSubWorkflow
                    └─→ ...
```

**Caution**: Ensure base cases to prevent infinite recursion!

## Ideation: Future Improvements

Current limitations and potential solutions:

1. **Agent exhaustion**: Sub-workflows compete for the same agents
   - Solution: Dynamic pool scaling, agent priorities

2. **Result passing**: Hard to get structured results from sub-workflow
   - Solution: Sub-workflow writes results to file, parent reads it

3. **Progress visibility**: Parent can't see sub-workflow progress
   - Solution: Shared event stream, progress callbacks

4. **Error handling**: Sub-workflow failure hard to handle gracefully
   - Solution: Sub-workflow returns structured error, parent decides

## Key Points

- Sub-workflows are just GSD invocations from commands
- Reusing pools is simple but can cause contention
- Agents can dynamically generate sub-workflow configs
- Use temp directories for sub-workflow artifacts
