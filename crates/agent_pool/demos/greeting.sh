#!/bin/bash
# Demo: Greeting agent
#
# This demonstrates the greeting agent which responds differently
# based on the style requested (casual vs formal).

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../.."
ROOT=$(mktemp -d)

# Use pre-built binary if AGENT_POOL is set, otherwise build
if [ -z "$AGENT_POOL" ]; then
    echo "Building agent_pool..."
    cargo build -p agent_pool --quiet
    echo "Build complete."
    echo ""
    AGENT_POOL="$WORKSPACE_ROOT/target/debug/agent_pool"
fi

echo "=== Demo: Greeting Agent ==="
echo "Working directory: $ROOT"
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    $AGENT_POOL stop --pool "$ROOT" 2>/dev/null || true
    sleep 0.2
    kill -9 $AGENT_PID 2>/dev/null || true
    wait $AGENT_PID 2>/dev/null || true
    rm -rf "$ROOT"
    echo "Done."
}
trap cleanup EXIT

# Start agent pool
echo "Starting agent pool..."
$AGENT_POOL start --pool "$ROOT" &
POOL_PID=$!
sleep 0.5

# Start greeting agent
echo "Starting greeting agent..."
"$SCRIPT_DIR/../scripts/greeting-agent.sh" "$ROOT" "friendly-bot" 0.1 &
AGENT_PID=$!
sleep 0.3

# Submit greeting requests
echo ""
echo "Requesting casual greeting..."
result=$($AGENT_POOL submit_task --pool "$ROOT" --data '{"kind":"Task","task":{"instructions":"Return a greeting","data":"casual"}}')
echo "Response: $result"
echo ""

echo "Requesting formal greeting..."
result=$($AGENT_POOL submit_task --pool "$ROOT" --data '{"kind":"Task","task":{"instructions":"Return a greeting","data":"formal"}}')
echo "Response: $result"
echo ""

echo "=== Success! ==="
