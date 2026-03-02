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
POOL_ROOT=$(mktemp -d)
POOL_ID="demo"

# Use pre-built binary if AGENT_POOL is set, otherwise build
if [ -z "$AGENT_POOL" ]; then
    echo "Building agent_pool..."
    cargo build -p agent_pool --quiet
    echo "Build complete."
    echo ""
    AGENT_POOL="$WORKSPACE_ROOT/target/debug/agent_pool"
fi

echo "=== Demo: Multiple Agents, Many Tasks ==="
echo "Working directory: $POOL_ROOT"
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    $AGENT_POOL --pool-root "$POOL_ROOT" stop --pool "$POOL_ID" 2>/dev/null || true
    sleep 0.2
    kill -9 $AGENT1_PID $AGENT2_PID $AGENT3_PID 2>/dev/null || true
    wait $AGENT1_PID $AGENT2_PID $AGENT3_PID 2>/dev/null || true
    rm -rf "$POOL_ROOT"
    echo "Done."
}
trap cleanup EXIT

# Start agent pool in background
echo "Starting agent pool..."
$AGENT_POOL --pool-root "$POOL_ROOT" start --pool "$POOL_ID" &
POOL_PID=$!
sleep 0.5

# Start agents with different sleep times
echo "Starting agents with varying response times..."
"$SCRIPT_DIR/../scripts/echo-agent.sh" --pool-root "$POOL_ROOT" --pool "$POOL_ID" --name "fast-agent" --sleep 0.1 &
AGENT1_PID=$!
"$SCRIPT_DIR/../scripts/echo-agent.sh" --pool-root "$POOL_ROOT" --pool "$POOL_ID" --name "medium-agent" --sleep 0.3 &
AGENT2_PID=$!
"$SCRIPT_DIR/../scripts/echo-agent.sh" --pool-root "$POOL_ROOT" --pool "$POOL_ID" --name "slow-agent" --sleep 0.5 &
AGENT3_PID=$!
sleep 0.3

# Submit tasks rapidly (in background so they're concurrent)
echo ""
echo "Submitting 6 tasks rapidly..."
echo ""

submit_task() {
    local task="$1"
    local json="{\"kind\":\"Task\",\"task\":{\"instructions\":\"Echo this back\",\"data\":\"$task\"}}"
    result=$($AGENT_POOL --pool-root "$POOL_ROOT" submit_task --pool "$POOL_ID" --data "$json")
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
