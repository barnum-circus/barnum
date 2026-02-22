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
        # Read envelope and extract content
        envelope=$(cat "$AGENT_DIR/task.json")
        task=$(echo "$envelope" | jq -r '.content // .')
        echo "[$AGENT_ID] Processing: $task" >&2

        sleep "$SLEEP_TIME"

        echo "$task [processed by $AGENT_ID]" > "$AGENT_DIR/response.json"
        echo "[$AGENT_ID] Done" >&2
    fi
    sleep 0.05
done
