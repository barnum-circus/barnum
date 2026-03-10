#!/bin/bash
# Simple demo agent that echoes tasks back with a processing marker.
#
# Usage: ./echo-agent.sh --root <root> --pool <id> --name <agent-id> [--sleep <seconds>]
#
# The agent:
# 1. Calls get_task to wait for a task
# 2. Receives tasks from the daemon
# 3. Echoes the task data back with "[processed by <agent-id>]"
# 4. Writes response to file, then loops for the next task

set -e

# Parse arguments
POOL_ROOT=""
POOL_ID=""
AGENT_ID=""
SLEEP_TIME="0.1"

while [[ $# -gt 0 ]]; do
    case $1 in
        --root) POOL_ROOT="$2"; shift 2 ;;
        --pool) POOL_ID="$2"; shift 2 ;;
        --name) AGENT_ID="$2"; shift 2 ;;
        --sleep) SLEEP_TIME="$2"; shift 2 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

if [ -z "$POOL_ROOT" ] || [ -z "$POOL_ID" ] || [ -z "$AGENT_ID" ]; then
    echo "Usage: $0 --root <root> --pool <id> --name <agent-id> [--sleep <seconds>]" >&2
    exit 1
fi

# Find troupe binary
if [ -n "$TROUPE" ]; then
    : # Use env var
elif [ -f "$(dirname "$0")/../../../target/debug/troupe" ]; then
    TROUPE="$(dirname "$0")/../../../target/debug/troupe"
else
    TROUPE="troupe"
fi

echo "[$AGENT_ID] Agent started" >&2

cleanup() {
    echo "[$AGENT_ID] Agent shutting down" >&2
    # Kill any child processes (e.g., blocked get_task)
    pkill -P $$ 2>/dev/null || true
    exit 0
}
trap cleanup SIGINT SIGTERM

while true; do
    # Get next task
    TASK_JSON=$("$TROUPE" --root "$POOL_ROOT" get_task --pool "$POOL_ID" --name "$AGENT_ID" 2>/dev/null) || {
        echo "[$AGENT_ID] get_task failed, exiting" >&2
        exit 1
    }

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
        echo "{}" > "$RESPONSE_FILE"
        continue
    fi

    echo "[$AGENT_ID] Processing: $TASK_DATA" >&2

    sleep "$SLEEP_TIME"

    # Build response
    RESPONSE="$TASK_DATA [processed by $AGENT_ID]"

    echo "[$AGENT_ID] Done" >&2

    # Write response to file
    echo "$RESPONSE" > "$RESPONSE_FILE"
done

echo "[$AGENT_ID] Agent exiting" >&2
