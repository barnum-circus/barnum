#!/bin/bash
# Demo: Single agent, single task
#
# This demonstrates the basic protocol:
# 1. Start the multiplexer
# 2. Start one agent
# 3. Submit one task
# 4. See the result
# 5. Clean up

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT=$(mktemp -d)

# Build the binary first
cargo build -p gsd_multiplexer

MULTIPLEXER="${MULTIPLEXER:-cargo run -p gsd_multiplexer --}"

echo "=== Demo: Single Agent, Single Task ==="
echo "Working directory: $ROOT"
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    kill $AGENT_PID 2>/dev/null || true
    $MULTIPLEXER stop "$ROOT" 2>/dev/null || true
    rm -rf "$ROOT"
    echo "Done."
}
trap cleanup EXIT

# Start multiplexer in background
echo "Starting multiplexer..."
$MULTIPLEXER start "$ROOT" &
MULTIPLEXER_PID=$!
sleep 0.5

# Start agent in background
echo "Starting agent..."
"$SCRIPT_DIR/../scripts/echo-agent.sh" "$ROOT" "agent-1" 0.1 &
AGENT_PID=$!
sleep 0.3

# Submit a task
echo ""
echo "Submitting task: 'Hello, World!'"
result=$($MULTIPLEXER submit "$ROOT" "Hello, World!")
echo "Result: $result"
echo ""
echo "=== Success! ==="
