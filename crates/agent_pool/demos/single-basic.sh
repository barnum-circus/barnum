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

# Build the binary first (quiet mode to avoid interleaving with demo output)
echo "Building agent_pool..."
cargo build -p agent_pool --quiet
echo "Build complete."
echo ""

# Use the built binary directly instead of cargo run to avoid recompilation output
AGENT_POOL="${AGENT_POOL:-$WORKSPACE_ROOT/target/debug/agent_pool}"

echo "=== Demo: Single Agent, Single Task ==="
echo "Working directory: $ROOT"
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    # Kill and wait for agent to fully terminate (suppresses shutdown message timing issues)
    kill $AGENT_PID 2>/dev/null || true
    wait $AGENT_PID 2>/dev/null || true
    $AGENT_POOL stop "$ROOT" 2>/dev/null || true
    rm -rf "$ROOT"
    echo "Done."
}
trap cleanup EXIT

# Start agent pool in background (use LOG_LEVEL=debug or trace for more output)
echo "Starting agent pool..."
$AGENT_POOL start "$ROOT" --log-level "${LOG_LEVEL:-info}" &
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
result=$($AGENT_POOL submit "$ROOT" "Hello, World!")
echo "Result: $result"
echo ""
echo "=== Success! ==="
