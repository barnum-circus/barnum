#!/bin/bash
# Start command agents for the cmd pool.
#
# Usage: ./scripts/start-cmd-agents.sh [num-agents]

set -e

cd "$(dirname "$0")/.."

# Build if needed
echo -n "Building... "
cargo build -p agent_pool_cli --quiet
echo "done"

NUM_AGENTS="${1:-5}"

> /tmp/agent.log

cleanup() {
    echo ""
    echo "Stopping all agents..."
    pkill -9 -f "command-agent.sh --pool cmd" 2>/dev/null || true
    echo "Done."
    exit 0
}

trap cleanup SIGINT SIGTERM

# Start all agents in background
for i in $(seq 1 "$NUM_AGENTS"); do
    ./crates/agent_pool/scripts/command-agent.sh --pool cmd --log /tmp/agent.log &
done

echo "Started $NUM_AGENTS agents. Press Ctrl+C to stop all."

# Wait for all children
wait
