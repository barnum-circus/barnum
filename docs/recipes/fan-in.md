# Fan-In (Collecting Results)

Fan-in collects results from multiple parallel tasks into a single aggregation.

## The Challenge

GSD tasks are independent - there's no built-in way to "wait for all siblings to complete." Each task completes individually.

## Pattern 1: File-Based Collection

Use the filesystem to collect results, then aggregate:

```json
{
  "steps": [
    {
      "name": "Split",
      "value_schema": {
        "type": "object",
        "required": ["items"],
        "properties": {
          "items": { "type": "array" }
        }
      },
      "action": {
        "kind": "Command",
        "script": "scripts/split-work.sh"
      },
      "next": ["Process"]
    },
    {
      "name": "Process",
      "value_schema": {
        "type": "object",
        "required": ["task_id", "data"],
        "properties": {
          "task_id": { "type": "string" },
          "data": { "type": "object" }
        }
      },
      "post": "scripts/save-result.sh",
      "action": {
        "kind": "Pool",
        "instructions": "Process this chunk. Return `[]` when done."
      },
      "next": []
    }
  ]
}
```

## Initial Tasks

```bash
gsd run config.json --pool agents --initial '[{"kind": "Split", "value": {"items": ["a", "b", "c"]}}]'
```

**scripts/save-result.sh** (post hook):
```bash
#!/bin/bash
INPUT=$(cat)
KIND=$(echo "$INPUT" | jq -r '.kind')

if [ "$KIND" = "Success" ]; then
  TASK_ID=$(echo "$INPUT" | jq -r '.input.task_id')
  echo "$INPUT" | jq '.output' > "/tmp/results/$TASK_ID.json"
fi
```

After the workflow completes, aggregate:
```bash
jq -s '.' /tmp/results/*.json > aggregated.json
```

## Pattern 2: Counter-Based Aggregation

Track completion count and trigger aggregation on last task:

```json
{
  "steps": [
    {
      "name": "Process",
      "value_schema": { "type": "object" },
      "post": "scripts/check-and-aggregate.sh",
      "action": { "kind": "Pool", "instructions": "Process this item. Return `[]`." },
      "next": ["Aggregate"]
    },
    {
      "name": "Aggregate",
      "value_schema": {
        "type": "object",
        "required": ["results"],
        "properties": {
          "results": { "type": "array" }
        }
      },
      "action": { "kind": "Pool", "instructions": "Aggregate all results. Return `[]`." },
      "next": []
    }
  ]
}
```

**scripts/check-and-aggregate.sh**:
```bash
#!/bin/bash
INPUT=$(cat)

# Save result
echo "$INPUT" | jq '.output' >> /tmp/results.jsonl

# Increment counter
COUNTER_FILE="/tmp/counter"
flock "$COUNTER_FILE" bash -c '
  COUNT=$(($(cat "$COUNTER_FILE" 2>/dev/null || echo 0) + 1))
  echo $COUNT > "$COUNTER_FILE"
  echo $COUNT
'

# Check if all done (TOTAL set by split step)
if [ "$COUNT" = "$TOTAL" ]; then
  # Trigger aggregation by outputting a task
  # (Note: post hooks currently can't spawn tasks - this is aspirational)
  echo "All done, ready to aggregate"
fi
```

## Pattern 3: External Orchestration

Use an external system to track and aggregate:

```json
{
  "steps": [
    {
      "name": "Process",
      "value_schema": {
        "type": "object",
        "required": ["id"],
        "properties": {
          "id": { "type": "string" }
        }
      },
      "pre": "scripts/register-task.sh",
      "post": "scripts/report-completion.sh",
      "action": { "kind": "Pool", "instructions": "Process this item. Return `[]`." },
      "next": []
    }
  ]
}
```

The external system (Redis, database, etc.) tracks:
- Total expected tasks
- Completed tasks
- Results from each task

When all tasks complete, it triggers the next action.

## Current Limitations

- No built-in "wait for N tasks" primitive
- No shared state between tasks

## Implemented Features

Post hooks can now modify the `next` array to filter, add, or transform tasks.
See [hooks.md](hooks.md) for details.

The `finally` hook runs after all descendants complete, enabling aggregation patterns.

## Future Possibilities

Potential features that would improve fan-in:

1. **Barrier steps**: `"wait_for": ["Process"]` - wait for all Process tasks
2. **Accumulator**: Built-in result collection across tasks

## Key Points

- Fan-in requires external coordination (filesystem, database)
- Post hooks can save results for later aggregation
- Consider running aggregation as a separate workflow after main completes
