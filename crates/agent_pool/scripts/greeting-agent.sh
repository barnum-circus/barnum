#!/bin/bash
# Simple greeting agent that demonstrates multi-command handling.
#
# Usage: ./greeting-agent.sh <root> <agent-id> [sleep-seconds]
#
# Input format (single line):
#   "casual"  -> "Hi <agent-id>, how are ya?"
#   "formal"  -> "Salutations <agent-id>, how are you doing on this most splendiferous and utterly magnificent day?"

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

echo "[$AGENT_ID] Greeting agent started, watching $AGENT_DIR" >&2

cleanup() {
    echo "[$AGENT_ID] Agent shutting down" >&2
    exit 0
}
trap cleanup SIGINT SIGTERM

# Process a task and return greeting
process_task() {
    local style="$1"

    case "$style" in
        casual)
            echo "Hi $AGENT_ID, how are ya?"
            ;;
        formal)
            echo "Salutations $AGENT_ID, how are you doing on this most splendiferous and utterly magnificent day?"
            ;;
        *)
            echo "Error: unknown style '$style' (use 'casual' or 'formal')"
            ;;
    esac
}

while true; do
    if [ -f "$AGENT_DIR/next_task" ]; then
        # Atomically rename next_task to in_progress to claim the task
        if mv "$AGENT_DIR/next_task" "$AGENT_DIR/in_progress" 2>/dev/null; then
            task=$(cat "$AGENT_DIR/in_progress")
            # Trim whitespace
            task=$(echo "$task" | tr -d '[:space:]')
            echo "[$AGENT_ID] Processing: $task" >&2

            sleep "$SLEEP_TIME"

            # Process and write result
            result=$(process_task "$task")
            echo "$result" > "$AGENT_DIR/output"
            rm -f "$AGENT_DIR/in_progress"
            echo "[$AGENT_ID] Done: $result" >&2
        fi
    fi
    sleep 0.05
done
