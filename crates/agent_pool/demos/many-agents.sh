#!/bin/bash
# Demo: Multiple agents, many tasks
#
# This demonstrates parallel processing:
# 1. Start the agent pool
# 2. Start 3 agents with random sleep times
# 3. Submit 6 tasks rapidly (without waiting)
# 4. Watch responses come back interleaved
# 5. Clean up

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../.."
ROOT=$(mktemp -d)

# Build the binary first (quiet mode to avoid interleaving with demo output)
echo "Building agent_pool..."
cargo build -p agent_pool --quiet
echo "Build complete."
echo ""

# Use the built binary directly instead of cargo run to avoid recompilation output
AGENT_POOL="${AGENT_POOL:-$WORKSPACE_ROOT/target/debug/agent_pool}"

echo "=== Demo: Multiple Agents, Many Tasks ==="
echo "Working directory: $ROOT"
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    # Kill and wait for agents to fully terminate (suppresses shutdown message timing issues)
    kill $AGENT1_PID $AGENT2_PID $AGENT3_PID 2>/dev/null || true
    wait $AGENT1_PID $AGENT2_PID $AGENT3_PID 2>/dev/null || true
    $AGENT_POOL stop --pool "$ROOT" 2>/dev/null || true
    rm -rf "$ROOT"
    echo "Done."
}
trap cleanup EXIT

# Start agent pool in background
echo "Starting agent pool..."
$AGENT_POOL start --pool "$ROOT" &
POOL_PID=$!
sleep 0.5

# Start agents with different sleep times
echo "Starting agents with varying response times..."
"$SCRIPT_DIR/../scripts/echo-agent.sh" "$ROOT" "fast-agent" 0.1 &
AGENT1_PID=$!
"$SCRIPT_DIR/../scripts/echo-agent.sh" "$ROOT" "medium-agent" 0.3 &
AGENT2_PID=$!
"$SCRIPT_DIR/../scripts/echo-agent.sh" "$ROOT" "slow-agent" 0.5 &
AGENT3_PID=$!
sleep 0.3

# Submit tasks rapidly (in background so they're concurrent)
echo ""
echo "Submitting 6 tasks rapidly..."
echo ""

submit_task() {
    local task="$1"
    result=$($AGENT_POOL submit_task --pool "$ROOT" --input "$task")
    echo "Result: $result"
}

submit_task "Task-1" & PIDS="$!"
submit_task "Task-2" & PIDS="$PIDS $!"
submit_task "Task-3" & PIDS="$PIDS $!"
submit_task "Task-4" & PIDS="$PIDS $!"
submit_task "Task-5" & PIDS="$PIDS $!"
submit_task "Task-6" & PIDS="$PIDS $!"

# Wait for submit tasks only (not the daemon)
wait $PIDS

echo ""
echo "=== All tasks completed! ==="
