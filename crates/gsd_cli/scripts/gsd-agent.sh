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

# Find agent_pool binary
if [ -n "$AGENT_POOL" ]; then
    : # Use env var
elif [ -f "$(dirname "$0")/../../../target/debug/agent_pool" ]; then
    AGENT_POOL="$(dirname "$0")/../../../target/debug/agent_pool"
else
    AGENT_POOL="agent_pool"
fi

echo "[$AGENT_ID] Started" >&2
if [ -n "$TRANSITION_MAP" ]; then
    echo "[$AGENT_ID] Transitions: $TRANSITION_MAP" >&2
fi

cleanup() {
    echo "[$AGENT_ID] Shutting down" >&2
    # Kill any child processes (e.g., blocked next_task)
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

# First task via register
TASK_JSON=$("$AGENT_POOL" register --pool "$ROOT" --name "$AGENT_ID" 2>/dev/null) || {
    echo "[$AGENT_ID] Register failed, exiting" >&2
    exit 1
}

while true; do
    # Extract response file path and task kind
    RESPONSE_FILE=$(echo "$TASK_JSON" | jq -r '.response_file')
    MSG_KIND=$(echo "$TASK_JSON" | jq -r '.kind // "Task"')

    # Handle kicked - exit gracefully
    if [ "$MSG_KIND" = "Kicked" ]; then
        echo "[$AGENT_ID] Kicked by daemon, exiting" >&2
        exit 0
    fi

    # Handle heartbeat - respond immediately via next_task
    if [ "$MSG_KIND" = "Heartbeat" ]; then
        echo "[$AGENT_ID] Heartbeat" >&2
        TASK_JSON=$("$AGENT_POOL" next_task --pool "$ROOT" --response-file "$RESPONSE_FILE" --data "{}" --name "$AGENT_ID" 2>/dev/null) || break
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

    # Submit response and get next task
    TASK_JSON=$("$AGENT_POOL" next_task --pool "$ROOT" --response-file "$RESPONSE_FILE" --data "$RESPONSE" --name "$AGENT_ID" 2>/dev/null) || break
done

echo "[$AGENT_ID] Agent exiting" >&2
