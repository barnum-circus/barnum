#!/bin/bash
# Demo: Single agent, single task
#
# This demonstrates the basic protocol:
# 1. Start the agent pool
# 2. Start one agent
# 3. Submit one task
# 4. See the result
# 5. Clean up

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../.."
POOL_ROOT=$(mktemp -d)
POOL_ID="demo"

# Use pre-built binary if TROUPE is set, otherwise build
if [ -z "$TROUPE" ]; then
    echo "Building troupe..."
    cargo build -p troupe --quiet
    echo "Build complete."
    echo ""
    TROUPE="$WORKSPACE_ROOT/target/debug/troupe"
fi

echo "=== Demo: Single Agent, Single Task ==="
echo "Working directory: $POOL_ROOT"
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    # Stop pool first - this kicks the agent cleanly
    $TROUPE --root "$POOL_ROOT" stop --pool "$POOL_ID" 2>/dev/null || true
    sleep 0.2
    # Force kill agent if still running
    kill -9 $AGENT_PID 2>/dev/null || true
    wait $AGENT_PID 2>/dev/null || true
    rm -rf "$POOL_ROOT"
    echo "Done."
}
trap cleanup EXIT

# Start agent pool in background (use LOG_LEVEL=debug or trace for more output)
echo "Starting agent pool..."
$TROUPE --root "$POOL_ROOT" start --pool "$POOL_ID" --log-level "${LOG_LEVEL:-info}" &
POOL_PID=$!
sleep 0.5

# Start agent in background
echo "Starting agent..."
"$SCRIPT_DIR/../scripts/echo-agent.sh" --root "$POOL_ROOT" --pool "$POOL_ID" --name "agent-1" --sleep 0.1 &
AGENT_PID=$!
sleep 0.3

# Submit a task
echo ""
echo "Submitting task: 'Hello, World!'"
result=$($TROUPE --root "$POOL_ROOT" submit_task --pool "$POOL_ID" --data '{"kind":"Task","task":{"instructions":"Echo this back","data":"Hello, World!"}}')
echo "Result: $result"
echo ""
echo "=== Success! ==="
