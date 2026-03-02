#!/bin/bash
# Demo: Single agent, many tasks (queuing)
#
# This demonstrates the queue behavior:
# 1. Start the agent pool
# 2. Start ONE agent with 1 second sleep
# 3. Submit 4 tasks rapidly
# 4. Watch them process sequentially
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

echo "=== Demo: Single Agent Queue ==="
echo "Working directory: $POOL_ROOT"
echo "Tasks will queue and process one at a time."
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    $AGENT_POOL --pool-root "$POOL_ROOT" stop --pool "$POOL_ID" 2>/dev/null || true
    sleep 0.2
    kill -9 $AGENT_PID 2>/dev/null || true
    wait $AGENT_PID 2>/dev/null || true
    rm -rf "$POOL_ROOT"
    echo "Done."
}
trap cleanup EXIT

# Start agent pool in background
echo "Starting agent pool..."
$AGENT_POOL --pool-root "$POOL_ROOT" start --pool "$POOL_ID" &
POOL_PID=$!
sleep 0.5

# Start ONE agent with slow response time
echo "Starting single agent (1 second per task)..."
"$SCRIPT_DIR/../scripts/echo-agent.sh" --pool-root "$POOL_ROOT" --pool "$POOL_ID" --name "only-agent" --sleep 1.0 &
AGENT_PID=$!
sleep 0.3

# Submit tasks rapidly
echo ""
echo "Submitting 4 tasks rapidly..."
echo "Watch them complete one by one (~1 second apart)"
echo ""

submit_task() {
    local task="$1"
    local start=$(date +%s.%N)
    local json="{\"kind\":\"Task\",\"task\":{\"instructions\":\"Echo this back\",\"data\":\"$task\"}}"
    result=$($AGENT_POOL --pool-root "$POOL_ROOT" submit_task --pool "$POOL_ID" --data "$json")
    local end=$(date +%s.%N)
    local elapsed=$(echo "$end - $start" | bc)
    echo "[${elapsed}s] $result"
}

submit_task "Task-A" & PIDS="$!"
submit_task "Task-B" & PIDS="$PIDS $!"
submit_task "Task-C" & PIDS="$PIDS $!"
submit_task "Task-D" & PIDS="$PIDS $!"

# Wait for submit tasks only (not the daemon)
wait $PIDS

echo ""
echo "=== All tasks completed! ==="
echo "Notice tasks completed ~1 second apart (single-threaded)."
