#!/bin/bash
# Start command agents for the cmd pool.
#
# Usage: ./scripts/start-cmd-agents.sh [num-agents]

set -e

cd "$(dirname "$0")/.."

NUM_AGENTS="${1:-5}"

> /tmp/agent.log

# Track child PIDs so we can kill them on Ctrl+C
CHILD_PIDS=()

cleanup() {
    echo ""
    echo "Stopping all agents..."
    # Kill tracked PIDs
    for pid in "${CHILD_PIDS[@]}"; do
        kill -9 "$pid" 2>/dev/null || true
    done
    # Also kill any remaining command-agent.sh processes we spawned
    pkill -9 -f "command-agent.sh --pool cmd" 2>/dev/null || true
    echo "Done."
    exit 0
}

trap cleanup SIGINT SIGTERM

# Start all agents in background
for i in $(seq 1 "$NUM_AGENTS"); do
    ./crates/agent_pool/scripts/command-agent.sh --pool cmd --log /tmp/agent.log &
    CHILD_PIDS+=($!)
done

echo "Started $NUM_AGENTS agents. Press Ctrl+C to stop all."

# Wait for all children
wait
