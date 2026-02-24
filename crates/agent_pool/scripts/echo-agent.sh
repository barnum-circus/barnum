#!/bin/bash
# Simple demo agent that polls for tasks and processes them.
#
# Usage: ./echo-agent.sh <root> <agent-id> [sleep-seconds]
#
# The agent:
# 1. Creates its directory under <root>/agents/<agent-id>
# 2. Polls for task.json
# 3. When found (and response.json doesn't exist): processes the task
# 4. Writes response.json
# 5. Output is: "<input> [processed by <agent-id>]"

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
    # Process if task.json exists and response.json doesn't
    if [ -f "$AGENT_DIR/task.json" ] && [ ! -f "$AGENT_DIR/response.json" ]; then
        # Read envelope and extract kind/task data
        envelope=$(cat "$AGENT_DIR/task.json")
        kind=$(echo "$envelope" | jq -r '.kind // "Task"')
        task=$(echo "$envelope" | jq -r '.task.data // .task // .')

        # Handle kicked - exit gracefully
        if [ "$kind" = "Kicked" ]; then
            echo "[$AGENT_ID] Kicked by daemon, exiting" >&2
            exit 0
        fi

        # Handle heartbeat immediately (atomic write via temp file + mv)
        if [ "$kind" = "Heartbeat" ]; then
            echo "[$AGENT_ID] Heartbeat" >&2
            echo "{}" > "$AGENT_DIR/response.json.tmp"
            mv "$AGENT_DIR/response.json.tmp" "$AGENT_DIR/response.json"
            sleep 0.05
            continue
        fi

        echo "[$AGENT_ID] Processing: $task" >&2

        sleep "$SLEEP_TIME"

        # Atomic write via temp file + mv to avoid race conditions
        echo "$task [processed by $AGENT_ID]" > "$AGENT_DIR/response.json.tmp"
        mv "$AGENT_DIR/response.json.tmp" "$AGENT_DIR/response.json"
        echo "[$AGENT_ID] Done" >&2
    fi
    sleep 0.05
done
