#!/bin/bash
# Simple demo agent that echoes tasks back with a processing marker.
#
# Usage: ./echo-agent.sh <root> <agent-id> [sleep-seconds]
#
# The agent:
# 1. Registers with the pool via CLI
# 2. Receives tasks from the daemon
# 3. Echoes the task data back with "[processed by <agent-id>]"
# 4. Loops for the next task

set -e

ROOT="$1"
AGENT_ID="$2"
SLEEP_TIME="${3:-0.1}"

if [ -z "$ROOT" ] || [ -z "$AGENT_ID" ]; then
    echo "Usage: $0 <root> <agent-id> [sleep-seconds]" >&2
    exit 1
fi

# Find agent_pool binary
if [ -n "$AGENT_POOL" ]; then
    : # Use env var
elif [ -f "$(dirname "$0")/../../../target/debug/agent_pool" ]; then
    AGENT_POOL="$(dirname "$0")/../../../target/debug/agent_pool"
else
    AGENT_POOL="agent_pool"
fi

echo "[$AGENT_ID] Agent started" >&2

cleanup() {
    echo "[$AGENT_ID] Agent shutting down" >&2
    # Kill any child processes (e.g., blocked next_task)
    pkill -P $$ 2>/dev/null || true
    exit 0
}
trap cleanup SIGINT SIGTERM

# First task via register
TASK_JSON=$("$AGENT_POOL" register --pool "$ROOT" --name "$AGENT_ID" 2>/dev/null) || {
    echo "[$AGENT_ID] Register failed, exiting" >&2
    exit 1
}

while true; do
    # Extract response file path, kind, and task data
    RESPONSE_FILE=$(echo "$TASK_JSON" | jq -r '.response_file')
    KIND=$(echo "$TASK_JSON" | jq -r '.kind // "Task"')
    TASK_DATA=$(echo "$TASK_JSON" | jq -r '.content.data // .content // empty')

    # Handle kicked - exit gracefully
    if [ "$KIND" = "Kicked" ]; then
        echo "[$AGENT_ID] Kicked by daemon, exiting" >&2
        exit 0
    fi

    # Handle heartbeat - respond immediately
    if [ "$KIND" = "Heartbeat" ]; then
        echo "[$AGENT_ID] Heartbeat" >&2
        TASK_JSON=$("$AGENT_POOL" next_task --pool "$ROOT" --response-file "$RESPONSE_FILE" --data "{}" --name "$AGENT_ID" 2>/dev/null) || break
        continue
    fi

    echo "[$AGENT_ID] Processing: $TASK_DATA" >&2

    sleep "$SLEEP_TIME"

    # Build response
    RESPONSE="$TASK_DATA [processed by $AGENT_ID]"

    echo "[$AGENT_ID] Done" >&2

    # Submit response and get next task
    TASK_JSON=$("$AGENT_POOL" next_task --pool "$ROOT" --response-file "$RESPONSE_FILE" --data "$RESPONSE" --name "$AGENT_ID" 2>/dev/null) || break
done

echo "[$AGENT_ID] Agent exiting" >&2
