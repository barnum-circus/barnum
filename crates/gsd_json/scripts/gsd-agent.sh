#!/bin/bash
# GSD-aware demo agent that understands the GSD protocol.
#
# Usage: ./gsd-agent.sh <root> <agent-id> [transition-map]
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

echo "[$AGENT_ID] GSD agent started, watching $AGENT_DIR" >&2
if [ -n "$TRANSITION_MAP" ]; then
    echo "[$AGENT_ID] Transitions: $TRANSITION_MAP" >&2
fi

cleanup() {
    echo "[$AGENT_ID] Agent shutting down" >&2
    exit 0
}
trap cleanup SIGINT SIGTERM

# Parse transition map into associative array format for lookup
# Format: "Start:Middle,Middle:End,End:"
get_next_step() {
    local kind="$1"
    local map="$TRANSITION_MAP"

    # If no map, always terminate
    if [ -z "$map" ]; then
        echo ""
        return
    fi

    # Parse comma-separated pairs
    IFS=',' read -ra pairs <<< "$map"
    for pair in "${pairs[@]}"; do
        IFS=':' read -r from to <<< "$pair"
        if [ "$from" = "$kind" ]; then
            echo "$to"
            return
        fi
    done

    # No match found, terminate
    echo ""
}

while true; do
    if [ -f "$AGENT_DIR/next_task" ]; then
        if mv "$AGENT_DIR/next_task" "$AGENT_DIR/in_progress" 2>/dev/null; then
            payload=$(cat "$AGENT_DIR/in_progress")

            # Extract kind from JSON payload using basic parsing
            # Payload format: {"task": {"kind": "...", "value": ...}, ...}
            kind=$(echo "$payload" | grep -o '"kind"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*"\([^"]*\)"$/\1/')

            echo "[$AGENT_ID] Processing task kind: $kind" >&2

            sleep "$SLEEP_TIME"

            # Get next step from transition map
            next=$(get_next_step "$kind")

            if [ -z "$next" ]; then
                # Terminate
                echo "[$AGENT_ID] Returning: []" >&2
                echo '[]' > "$AGENT_DIR/output"
            else
                # Transition to next step
                response="[{\"kind\": \"$next\", \"value\": {}}]"
                echo "[$AGENT_ID] Returning: $response" >&2
                echo "$response" > "$AGENT_DIR/output"
            fi

            rm -f "$AGENT_DIR/in_progress"
            echo "[$AGENT_ID] Done" >&2
        fi
    fi
    sleep 0.05
done
