# External Visualization of Barnum Runs

## Motivation

When running Barnum workflows — especially multi-step, multi-agent workflows — you want to see what's happening. Which tasks are in flight? Which agents are busy? What failed? What's the task tree look like? Today there's no way to answer these questions without reading raw log output.

The constraint: **Barnum itself should not build visualization tooling.** Barnum is a workflow engine. It produces structured data on disk. Visualization is a separate concern, handled by external tools.

This document explores what's already observable on disk (no Barnum changes needed) and what external tools could consume it.

## What's already on disk

A running Barnum workflow with `--state-log` and a troupe pool produces two independent data sources:

### 1. Troupe pool filesystem (live, ephemeral)

Under `<root>/pools/<pool_id>/`:

```
pools/<pool_id>/
├── status                          # "ready" or "stop"
├── daemon.sock                     # Unix socket (if accessible)
├── agents/
│   ├── <uuid>.ready.json           # Worker registered, waiting for task
│   ├── <uuid>.task.json            # Worker has been assigned a task
│   └── <uuid>.response.json        # Worker completed task
└── submissions/
    ├── <id>.request.json           # Task submitted (awaiting processing)
    └── <id>.response.json          # Task response (ready for pickup)
```

**What you can infer by watching this directory:**
- **Active workers:** Count of `*.task.json` files in `agents/` (assigned but not yet responded)
- **Idle workers:** Count of `*.ready.json` files in `agents/`
- **Pending submissions:** Count of `*.request.json` without matching `*.response.json` in `submissions/`
- **Throughput:** Rate of `*.response.json` file creation in `agents/`
- **Worker lifecycle:** Watch file creation/deletion patterns to see task assignment flow

**Limitations:**
- Files are ephemeral — cleaned up after processing
- No historical data (unless you snapshot)
- No task content visible without reading the JSON files
- The agent files cycle rapidly (create → assign → respond → delete)

### 2. Barnum state log (append-only, persistent)

When `--state-log <path>` is provided, Barnum writes an NDJSON file:

```
{"kind":"Config","config":{...}}
{"kind":"TaskSubmitted","task_id":0,"step":"Analyze","value":{...},"parent_id":null,"origin":{"kind":"Initial"}}
{"kind":"TaskCompleted","task_id":0,"outcome":{"kind":"Success","value":{"spawned_task_ids":[1,2,3],"finally_value":{...}}}}
{"kind":"TaskSubmitted","task_id":1,"step":"Implement","value":{...},"parent_id":0,"origin":{"kind":"Spawned"}}
...
```

Each line is a self-contained JSON object. Entry types:

| Entry | Fields | Meaning |
|-------|--------|---------|
| `Config` | `config` (full JSON) | Run configuration, first entry |
| `TaskSubmitted` | `task_id`, `step`, `value`, `parent_id`, `origin` | Task created |
| `TaskCompleted` | `task_id`, `outcome` (Success or Failed) | Task finished |

**What you can derive from the state log:**
- **Task tree:** `parent_id` links form a tree. Initial tasks are roots.
- **Task status:** Submitted but not completed = in flight. Completed with Success = done. Failed = see reason.
- **Step distribution:** Group by `step` to see how many tasks ran on each step.
- **Retry chains:** `origin.kind == "Retry"` with `origin.replaces` links retries to originals.
- **Finally hooks:** `origin.kind == "Finally"` with `origin.finally_for` links to parent.
- **Fan-out factor:** `outcome.value.spawned_task_ids.length` shows how many children each task spawned.
- **Failure analysis:** `outcome.value.reason` (Timeout, AgentLost, InvalidResponse with message).

**Limitations:**
- No timestamps (the state log doesn't record when events happened — this is a gap)
- No agent/worker identity (state log doesn't know which agent processed which task)
- No LLM response content (just task values and outcomes, not the full agent conversation)

## External visualization approaches

### Approach 1: `tail -f` + `jq` (zero-install)

The simplest live monitoring. Since the state log is NDJSON, `tail -f` streams new entries and `jq` filters them:

```bash
# Live task submissions
tail -f state.ndjson | jq 'select(.kind == "TaskSubmitted") | "\(.task_id) → \(.step)"'

# Live completions
tail -f state.ndjson | jq 'select(.kind == "TaskCompleted") | "\(.task_id): \(.outcome.kind)"'

# Live failure stream
tail -f state.ndjson | jq 'select(.kind == "TaskCompleted" and .outcome.kind == "Failed")'

# In-flight count (snapshot, not live)
jq -s '[.[] | select(.kind == "TaskSubmitted")] | length - ([.[] | select(.kind == "TaskCompleted")] | length)' state.ndjson
```

### Approach 2: `watch` + filesystem stats (zero-install)

Monitor the troupe pool directory for live agent activity:

```bash
# Live pool status (refreshes every second)
watch -n1 'echo "=== Pool ===" && \
  echo "Status: $(cat pools/demo/status 2>/dev/null || echo "not running")" && \
  echo "Active workers: $(ls pools/demo/agents/*.task.json 2>/dev/null | wc -l)" && \
  echo "Ready workers: $(ls pools/demo/agents/*.ready.json 2>/dev/null | wc -l)" && \
  echo "Pending submissions: $(ls pools/demo/submissions/*.request.json 2>/dev/null | wc -l)"'
```

### Approach 3: Filesystem watcher script

A small script that watches both the pool directory and the state log, maintaining a live dashboard in the terminal:

```bash
#!/bin/bash
# barnum-monitor.sh <root> <state-log>
ROOT=$1; LOG=$2

# Background: tail state log, count tasks
tail -f "$LOG" | while read -r line; do
  kind=$(echo "$line" | jq -r '.kind')
  case "$kind" in
    TaskSubmitted) echo "▶ Task $(echo "$line" | jq -r '.task_id'): $(echo "$line" | jq -r '.step')" ;;
    TaskCompleted) echo "✓ Task $(echo "$line" | jq -r '.task_id'): $(echo "$line" | jq -r '.outcome.kind')" ;;
  esac
done
```

### Approach 4: Post-hoc analysis with Visidata

[Visidata](https://www.visidata.org/) reads NDJSON natively:

```bash
vd state.ndjson
```

This gives a spreadsheet-like view where you can:
- Sort by `task_id`, `step`, or outcome
- Filter to just failures
- Group by step to see distribution
- Follow parent-child chains manually

### Approach 5: HTML tree viewer (generated)

Ask any LLM to generate a single-file HTML page that:
1. Reads a state log NDJSON file (drag-and-drop or file input)
2. Reconstructs the task tree from `parent_id` relationships
3. Color-codes nodes by status (in-flight = yellow, success = green, failed = red)
4. Shows task details on click (step, value, outcome)

The state log format is simple enough that this is a 200-line HTML file. No framework needed. Can be generated once and reused across projects.

### Approach 6: `fswatch` + live tree

For real-time visualization on macOS, `fswatch` watches the pool directory:

```bash
fswatch -r pools/demo/agents/ | while read -r path; do
  file=$(basename "$path")
  case "$file" in
    *.ready.json)   echo "🟢 Worker ready: ${file%.ready.json}" ;;
    *.task.json)    echo "🔵 Worker assigned: ${file%.task.json}" ;;
    *.response.json) echo "⚪ Worker done: ${file%.response.json}" ;;
  esac
done
```

Combined with state log tailing, this gives a complete picture of both the agent pool and the workflow state.

## What would make this better (no Barnum changes needed)

### Timestamps in the state log

The biggest gap. Without timestamps, you can't compute:
- Task duration (time between submitted and completed)
- Throughput (tasks per second)
- Time-to-first-task (warmup overhead)
- Gantt chart / timeline visualization

**However:** this doc is explicitly about no Barnum changes. An external wrapper can add timestamps:

```bash
# Wrap state log with timestamps
tail -f state.ndjson | while read -r line; do
  echo "{\"ts\":\"$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)\",\"entry\":$line}"
done > timestamped-state.ndjson
```

This produces a timestamped stream without modifying Barnum. The external tool reads the wrapper format.

### Agent identity mapping

The state log doesn't record which agent processed which task. The troupe pool filesystem shows agent UUIDs, but they're ephemeral. An external watcher could correlate:

1. Watch `agents/<uuid>.task.json` creation (captures task payload)
2. Watch `agents/<uuid>.response.json` creation (captures response)
3. Match UUIDs to state log `task_id`s by comparing payloads

This is fragile but possible without Barnum changes.

## Recommended tooling for different needs

| Need | Tool | Complexity |
|------|------|-----------|
| Quick check: "is it working?" | `tail -f state.ndjson \| jq` | Zero |
| Live agent pool status | `watch` + `ls` on pool dir | Zero |
| Post-hoc analysis | `vd state.ndjson` or `jq -s` | Install one tool |
| Tree visualization | Generated HTML file | One-time generation |
| Production monitoring | Export to Grafana/Datadog via sidecar | Medium |
| Real-time dashboard | Custom `fswatch` + state log script | Custom script |

The state log NDJSON format is the key enabler. It's a standard format that every tool in the ecosystem can consume. The pool filesystem structure is a bonus for real-time monitoring but not necessary for post-hoc analysis.
