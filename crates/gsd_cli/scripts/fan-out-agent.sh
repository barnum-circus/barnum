#!/bin/bash
# GSD agent that fans out Distribute -> 10 Worker tasks -> done.
#
# Usage: ./fan-out-agent.sh <pool> <agent-id> [num-workers] [sleep-seconds]

set -e

POOL="$1"
AGENT_ID="$2"
NUM_WORKERS="${3:-10}"
SLEEP_TIME="${4:-0.2}"

if [ -z "$POOL" ] || [ -z "$AGENT_ID" ]; then
    echo "Usage: $0 <pool> <agent-id> [num-workers] [sleep-seconds]" >&2
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

echo "[$AGENT_ID] Started (fan-out agent, $NUM_WORKERS workers)" >&2

cleanup() {
    echo "[$AGENT_ID] Shutting down" >&2
    # Kill any child processes (e.g., blocked get_task)
    pkill -P $$ 2>/dev/null || true
    exit 0
}
trap cleanup SIGINT SIGTERM

while true; do
    # Get next task
    TASK_JSON=$("$AGENT_POOL" get_task --pool "$POOL" --name "$AGENT_ID" 2>/dev/null) || {
        echo "[$AGENT_ID] get_task failed, exiting" >&2
        exit 1
    }

    # Extract response file path and message kind
    RESPONSE_FILE=$(echo "$TASK_JSON" | jq -r '.response_file')
    MSG_KIND=$(echo "$TASK_JSON" | jq -r '.kind // "Task"')

    # Handle kicked - exit gracefully
    if [ "$MSG_KIND" = "Kicked" ]; then
        echo "[$AGENT_ID] Kicked by daemon, exiting" >&2
        exit 0
    fi

    # Handle heartbeat - respond immediately
    if [ "$MSG_KIND" = "Heartbeat" ]; then
        echo "[$AGENT_ID] Heartbeat" >&2
        echo "{}" > "$RESPONSE_FILE"
        continue
    fi

    # Extract task kind from content
    TASK_KIND=$(echo "$TASK_JSON" | jq -r '.content.kind // empty')
    echo "[$AGENT_ID] Processing: $TASK_KIND" >&2

    sleep "$SLEEP_TIME"

    case "$TASK_KIND" in
        Distribute)
            # Fan out to N Worker tasks
            response="["
            for i in $(seq 1 $NUM_WORKERS); do
                if [ $i -gt 1 ]; then
                    response="$response,"
                fi
                response="$response{\"kind\": \"Worker\", \"value\": {\"id\": $i}}"
            done
            response="$response]"
            echo "[$AGENT_ID] -> $NUM_WORKERS Worker tasks" >&2
            ;;
        Worker)
            echo "[$AGENT_ID] -> [] (done)" >&2
            response='[]'
            ;;
        *)
            echo "[$AGENT_ID] Unknown kind: $TASK_KIND, returning []" >&2
            response='[]'
            ;;
    esac

    # Write response to file
    echo "$response" > "$RESPONSE_FILE"
done

echo "[$AGENT_ID] Agent exiting" >&2
