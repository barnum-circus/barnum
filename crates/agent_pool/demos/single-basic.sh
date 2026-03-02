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
ROOT=$(mktemp -d)

# Use pre-built binary if AGENT_POOL is set, otherwise build
if [ -z "$AGENT_POOL" ]; then
    echo "Building agent_pool..."
    cargo build -p agent_pool --quiet
    echo "Build complete."
    echo ""
    AGENT_POOL="$WORKSPACE_ROOT/target/debug/agent_pool"
fi

echo "=== Demo: Single Agent, Single Task ==="
echo "Working directory: $ROOT"
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    # Stop pool first - this kicks the agent cleanly
    $AGENT_POOL stop --pool "$ROOT" 2>/dev/null || true
    sleep 0.2
    # Force kill agent if still running
    kill -9 $AGENT_PID 2>/dev/null || true
    wait $AGENT_PID 2>/dev/null || true
    rm -rf "$ROOT"
    echo "Done."
}
trap cleanup EXIT

# Start agent pool in background (use LOG_LEVEL=debug or trace for more output)
echo "Starting agent pool..."
$AGENT_POOL start --pool "$ROOT" --log-level "${LOG_LEVEL:-info}" &
POOL_PID=$!
sleep 0.5

# Start agent in background
echo "Starting agent..."
"$SCRIPT_DIR/../scripts/echo-agent.sh" "$ROOT" "agent-1" 0.1 &
AGENT_PID=$!
sleep 0.3

# Submit a task
echo ""
echo "Submitting task: 'Hello, World!'"
result=$($AGENT_POOL submit_task --pool "$ROOT" --data '{"kind":"Task","task":{"instructions":"Echo this back","data":"Hello, World!"}}')
echo "Result: $result"
echo ""
echo "=== Success! ==="
