#!/bin/bash
# Barnum-aware demo agent that understands the Barnum protocol.
#
# Usage: ./barnum-agent.sh --root <root> --pool <id> --name <agent-id> [--transitions <map>] [--sleep <seconds>]
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

# Parse arguments
POOL_ROOT=""
POOL_ID=""
AGENT_ID=""
TRANSITION_MAP=""
SLEEP_TIME="0.1"

while [[ $# -gt 0 ]]; do
    case $1 in
        --root) POOL_ROOT="$2"; shift 2 ;;
        --pool) POOL_ID="$2"; shift 2 ;;
        --name) AGENT_ID="$2"; shift 2 ;;
        --transitions) TRANSITION_MAP="$2"; shift 2 ;;
        --sleep) SLEEP_TIME="$2"; shift 2 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

if [ -z "$POOL_ROOT" ] || [ -z "$POOL_ID" ] || [ -z "$AGENT_ID" ]; then
    echo "Usage: $0 --root <root> --pool <id> --name <agent-id> [--transitions <map>] [--sleep <seconds>]" >&2
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

echo "[$AGENT_ID] Started" >&2
if [ -n "$TRANSITION_MAP" ]; then
    echo "[$AGENT_ID] Transitions: $TRANSITION_MAP" >&2
fi

cleanup() {
    echo "[$AGENT_ID] Shutting down" >&2
    # Kill any child processes (e.g., blocked get_task)
    pkill -P $$ 2>/dev/null || true
    exit 0
}
trap cleanup SIGINT SIGTERM

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
    # Get next task
    TASK_JSON=$("$TROUPE" --root "$POOL_ROOT" get_task --pool "$POOL_ID" --name "$AGENT_ID" 2>/dev/null) || {
        echo "[$AGENT_ID] get_task failed, exiting" >&2
        exit 1
    }

    # Extract response file path and task kind
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

    # Build response
    next=$(get_next_step "$TASK_KIND")
    if [ -z "$next" ]; then
        echo "[$AGENT_ID] -> [] (done)" >&2
        RESPONSE='[]'
    else
        echo "[$AGENT_ID] -> $next" >&2
        RESPONSE="[{\"kind\": \"$next\", \"value\": {}}]"
    fi

    # Write response to file
    echo "$RESPONSE" > "$RESPONSE_FILE"
done

echo "[$AGENT_ID] Agent exiting" >&2
