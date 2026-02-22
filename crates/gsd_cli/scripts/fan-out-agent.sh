#!/bin/bash
# GSD agent that fans out Distribute -> 10 Worker tasks -> done.
#
# Usage: ./fan-out-agent.sh <root> <agent-id> [num-workers] [sleep-seconds]

set -e

ROOT="$1"
AGENT_ID="$2"
NUM_WORKERS="${3:-10}"
SLEEP_TIME="${4:-0.2}"

if [ -z "$ROOT" ] || [ -z "$AGENT_ID" ]; then
    echo "Usage: $0 <root> <agent-id> [num-workers] [sleep-seconds]" >&2
    exit 1
fi

AGENT_DIR="$ROOT/agents/$AGENT_ID"
mkdir -p "$AGENT_DIR"

echo "[$AGENT_ID] Started (fan-out agent, $NUM_WORKERS workers)" >&2

trap 'echo "[$AGENT_ID] Shutting down" >&2; exit 0' SIGINT SIGTERM

while true; do
    if [ -f "$AGENT_DIR/task.json" ] && [ ! -f "$AGENT_DIR/response.json" ]; then
        payload=$(cat "$AGENT_DIR/task.json")

        if command -v jq &> /dev/null; then
            kind=$(echo "$payload" | jq -r '.task.kind')
        else
            kind=$(echo "$payload" | grep -o '"kind"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"$/\1/')
        fi

        echo "[$AGENT_ID] Processing: $kind" >&2

        sleep "$SLEEP_TIME"

        case "$kind" in
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
                echo "$response" > "$AGENT_DIR/response.json"
                ;;
            Worker)
                echo "[$AGENT_ID] -> [] (done)" >&2
                echo '[]' > "$AGENT_DIR/response.json"
                ;;
            *)
                echo "[$AGENT_ID] Unknown kind: $kind, returning []" >&2
                echo '[]' > "$AGENT_DIR/response.json"
                ;;
        esac
    fi
    sleep 0.05
done
