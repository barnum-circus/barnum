#!/bin/bash
# Demo: Greeting agent
#
# This demonstrates the greeting agent which responds differently
# based on the style requested (casual vs formal).

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

echo "=== Demo: Greeting Agent ==="
echo "Working directory: $POOL_ROOT"
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

# Start agent pool
echo "Starting agent pool..."
$AGENT_POOL --pool-root "$POOL_ROOT" start --pool "$POOL_ID" &
POOL_PID=$!
sleep 0.5

# Start greeting agent
echo "Starting greeting agent..."
"$SCRIPT_DIR/../scripts/greeting-agent.sh" --pool-root "$POOL_ROOT" --pool "$POOL_ID" --name "friendly-bot" --sleep 0.1 &
AGENT_PID=$!
sleep 0.3

# Submit greeting requests
echo ""
echo "Requesting casual greeting..."
result=$($AGENT_POOL --pool-root "$POOL_ROOT" submit_task --pool "$POOL_ID" --data '{"kind":"Task","task":{"instructions":"Return a greeting","data":"casual"}}')
echo "Response: $result"
echo ""

echo "Requesting formal greeting..."
result=$($AGENT_POOL --pool-root "$POOL_ROOT" submit_task --pool "$POOL_ID" --data '{"kind":"Task","task":{"instructions":"Return a greeting","data":"formal"}}')
echo "Response: $result"
echo ""

echo "=== Success! ==="
