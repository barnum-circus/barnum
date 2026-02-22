#!/bin/bash
# GSD-aware demo agent that understands the GSD protocol.
#
# Usage: ./gsd-agent.sh <root> <agent-id> [transition-map] [sleep-seconds]
#
# The agent receives JSON payloads like:
#   {"task": {"kind": "Start", "value": {...}}, "instructions": "..."}
#
# And returns JSON arrays:
#   [{"kind": "Next", "value": {}}]
#
# The transition-map is a comma-separated list of from:to pairs:
#   "Start:Middle,Middle:End,End:"
#
# An empty "to" means terminate (return []).

set -e

ROOT="$1"
AGENT_ID="$2"
TRANSITION_MAP="${3:-}"
SLEEP_TIME="${4:-0.1}"

if [ -z "$ROOT" ] || [ -z "$AGENT_ID" ]; then
    echo "Usage: $0 <root> <agent-id> [transition-map] [sleep-seconds]" >&2
    exit 1
fi

AGENT_DIR="$ROOT/agents/$AGENT_ID"
mkdir -p "$AGENT_DIR"

echo "[$AGENT_ID] Started, watching $AGENT_DIR" >&2
if [ -n "$TRANSITION_MAP" ]; then
    echo "[$AGENT_ID] Transitions: $TRANSITION_MAP" >&2
fi

trap 'echo "[$AGENT_ID] Shutting down" >&2; exit 0' SIGINT SIGTERM

get_next_step() {
    local kind="$1"
    if [ -z "$TRANSITION_MAP" ]; then
        echo ""
        return
    fi

    IFS=',' read -ra pairs <<< "$TRANSITION_MAP"
    for pair in "${pairs[@]}"; do
        IFS=':' read -r from to <<< "$pair"
        if [ "$from" = "$kind" ]; then
            echo "$to"
            return
        fi
    done
    echo ""
}

while true; do
    # Process if task.json exists and response.json doesn't
    if [ -f "$AGENT_DIR/task.json" ] && [ ! -f "$AGENT_DIR/response.json" ]; then
        payload=$(cat "$AGENT_DIR/task.json")

        if command -v jq &> /dev/null; then
            kind=$(echo "$payload" | jq -r '.task.kind')
        else
            kind=$(echo "$payload" | grep -o '"kind"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"$/\1/')
        fi

        echo "[$AGENT_ID] Processing: $kind" >&2

        sleep "$SLEEP_TIME"

        next=$(get_next_step "$kind")
        if [ -z "$next" ]; then
            echo "[$AGENT_ID] -> [] (done)" >&2
            echo '[]' > "$AGENT_DIR/response.json"
        else
            echo "[$AGENT_ID] -> $next" >&2
            echo "[{\"kind\": \"$next\", \"value\": {}}]" > "$AGENT_DIR/response.json"
        fi
    fi
    sleep 0.05
done
