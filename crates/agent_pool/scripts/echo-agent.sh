#!/bin/bash
# Simple demo agent that polls for tasks and processes them.
#
# Usage: ./echo-agent.sh <root> <agent-id> [sleep-seconds]
#
# The agent:
# 1. Creates its directory under <root>/agents/<agent-id>
# 2. Polls for next_task file
# 3. When found: atomically renames to in_progress (prevents race conditions)
# 4. Processes the task (simulated with sleep)
# 5. Writes output and cleans up in_progress
# 6. Output is: "<input> [processed by <agent-id>]"

set -e

ROOT="$1"
AGENT_ID="$2"
SLEEP_TIME="${3:-0.1}"

if [ -z "$ROOT" ] || [ -z "$AGENT_ID" ]; then
    echo "Usage: $0 <root> <agent-id> [sleep-seconds]" >&2
    exit 1
fi

AGENT_DIR="$ROOT/agents/$AGENT_ID"
mkdir -p "$AGENT_DIR"

echo "[$AGENT_ID] Agent started, watching $AGENT_DIR" >&2

cleanup() {
    echo "[$AGENT_ID] Agent shutting down" >&2
    exit 0
}
trap cleanup SIGINT SIGTERM

while true; do
    if [ -f "$AGENT_DIR/next_task" ]; then
        # Atomically rename next_task to in_progress to claim the task
        # This prevents race conditions - if rename fails, another process took it
        if mv "$AGENT_DIR/next_task" "$AGENT_DIR/in_progress" 2>/dev/null; then
            task=$(cat "$AGENT_DIR/in_progress")
            echo "[$AGENT_ID] Processing: $task" >&2

            sleep "$SLEEP_TIME"

            echo "$task [processed by $AGENT_ID]" > "$AGENT_DIR/output"
            rm -f "$AGENT_DIR/in_progress"
            echo "[$AGENT_ID] Done" >&2
        fi
    fi
    sleep 0.05
done
