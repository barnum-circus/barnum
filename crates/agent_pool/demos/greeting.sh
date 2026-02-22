#!/bin/bash
# Demo: Greeting agent
#
# This demonstrates the greeting agent which responds differently
# based on the style requested (casual vs formal).

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../.."
ROOT=$(mktemp -d)

# Build the binary first
echo "Building agent_pool..."
cargo build -p agent_pool --quiet
echo "Build complete."
echo ""

AGENT_POOL="${AGENT_POOL:-$WORKSPACE_ROOT/target/debug/agent_pool}"

echo "=== Demo: Greeting Agent ==="
echo "Working directory: $ROOT"
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    kill $AGENT_PID 2>/dev/null || true
    wait $AGENT_PID 2>/dev/null || true
    $AGENT_POOL stop --pool "$ROOT" 2>/dev/null || true
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
result=$($AGENT_POOL submit_task --pool "$ROOT" --input "casual")
echo "Response: $result"
echo ""

echo "Requesting formal greeting..."
result=$($AGENT_POOL submit_task --pool "$ROOT" --input "formal")
echo "Response: $result"
echo ""

echo "=== Success! ==="
