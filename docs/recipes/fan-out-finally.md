# Fan-Out with Finally

Use `finally` to aggregate results or trigger follow-up work after a fan-out completes.

## The Pattern

```
┌─────────────────────────────────────────────────────────────────┐
│  Outer Task (with finally)                                       │
│                                                                   │
│  Coordinate ──┬──→ AnalyzeFile(1) ──→ (writes to tmpdir)        │
│               ├──→ AnalyzeFile(2) ──→ (writes to tmpdir)        │
│               └──→ AnalyzeFile(3) ──→ (writes to tmpdir)        │
│                                                                   │
│  ════════════════════════════════════════════════════════════    │
│  After ALL descendants complete:                                 │
│                                                                   │
│  finally ──→ Categorize ──→ Prioritize ──→ Done                  │
│  (reads tmpdir, spawns follow-up)                                │
└─────────────────────────────────────────────────────────────────┘
```

## Example: Code Analysis Pipeline

Analyze files, collect findings, then categorize and prioritize.

```json
{
  "steps": [
    {
      "name": "Coordinate",
      "value_schema": {
        "type": "object",
        "required": ["files"],
        "properties": {
          "files": { "type": "array", "items": { "type": "string" } }
        }
      },
      "action": {
        "kind": "Command",
        "script": "scripts/setup-and-split.sh"
      },
      "finally": "scripts/aggregate-and-continue.sh",
      "next": ["AnalyzeFile"]
    },
    {
      "name": "AnalyzeFile",
      "value_schema": {
        "type": "object",
        "required": ["file", "tmpdir"],
        "properties": {
          "file": { "type": "string" },
          "tmpdir": { "type": "string" }
        }
      },
      "post": "scripts/save-findings.sh",
      "action": {
        "kind": "Pool",
        "instructions": "Analyze this file for refactoring opportunities. Return findings as JSON. Return `[]`."
      },
      "next": []
    },
    {
      "name": "Categorize",
      "value_schema": {
        "type": "object",
        "required": ["findings"],
        "properties": {
          "findings": { "type": "array" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Read all findings and categorize them by type (performance, readability, security, etc.). Return `[{\"kind\": \"Prioritize\", \"value\": {\"categorized\": [{\"type\": \"performance\", \"items\": []}]}}]`"
      },
      "next": ["Prioritize"]
    },
    {
      "name": "Prioritize",
      "value_schema": {
        "type": "object",
        "required": ["categorized"],
        "properties": {
          "categorized": { "type": "array" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Select the top 5 highest-impact refactoring opportunities. Return `[]`."
      },
      "next": []
    }
  ]
}
```

## Initial Tasks

```bash
gsd run --config config.json --pool agents --initial-state '[{"kind": "Coordinate", "value": {"files": ["src/main.rs", "src/lib.rs"]}}]'
```

**scripts/setup-and-split.sh:**
```bash
#!/bin/bash
set -e
INPUT=$(cat)

# Create temp directory for results
TMPDIR=$(mktemp -d)
FILES=$(echo "$INPUT" | jq -r '.value.files[]')

# Fan out to analyze each file, passing tmpdir in value
echo "$FILES" | jq -R -s --arg tmpdir "$TMPDIR" '
  split("\n") | map(select(. != "")) |
  map({kind: "AnalyzeFile", value: {file: ., tmpdir: $tmpdir}})
'
```

**scripts/save-findings.sh** (post hook):
```bash
#!/bin/bash
INPUT=$(cat)
KIND=$(echo "$INPUT" | jq -r '.kind')

if [ "$KIND" = "Success" ]; then
  TMPDIR=$(echo "$INPUT" | jq -r '.input.tmpdir')
  FILE=$(echo "$INPUT" | jq -r '.input.file')

  # Save findings to tmpdir
  echo "$INPUT" | jq '.output' > "$TMPDIR/$(basename "$FILE").json"
fi

# Pass through (AnalyzeFile is terminal, no next tasks)
echo "$INPUT"
```

**scripts/aggregate-and-continue.sh** (finally hook):
```bash
#!/bin/bash
INPUT=$(cat)
TMPDIR=$(echo "$INPUT" | jq -r '.tmpdir')

# Aggregate all findings
FINDINGS=$(cat "$TMPDIR"/*.json | jq -s '.')

# Clean up
rm -rf "$TMPDIR"

# Spawn follow-up work
echo "[{\"kind\": \"Categorize\", \"value\": {\"findings\": $FINDINGS}}]"
```

## When to Use This Pattern

Use fan-out with finally when:

- You need to process items in parallel, then aggregate
- Results should be collected before follow-up work starts
- Cleanup must happen after all parallel work completes
- You want to spawn different follow-up work based on aggregated results

## Alternative: Nested GSD

For more complex scenarios (separate retry policies, isolated configs), consider [nested-gsd.md](nested-gsd.md). The `finally` pattern is simpler when you just need aggregation within a single workflow.

## Key Points

- `finally` runs after ALL descendants complete (not just direct children)
- `finally` receives the original task's value (useful for tmpdir paths)
- `finally` outputs an array of next tasks to spawn follow-up work
- Post hooks can save results to a shared location
- The pattern enables: fan-out → collect → continue
