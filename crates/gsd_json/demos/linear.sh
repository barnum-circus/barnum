#!/bin/bash
# Demo: Linear three-step GSD state machine
#
# This demonstrates a linear state machine:
# Start -> Middle -> End
#
# The agent follows the transition map exactly.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../.."
ROOT=$(mktemp -d)

# Build the binaries first
echo "Building binaries..."
cargo build -p agent_pool -p gsd_cli --quiet
echo "Build complete."
echo ""

AGENT_POOL="${AGENT_POOL:-$WORKSPACE_ROOT/target/debug/agent_pool}"
GSD="${GSD:-$WORKSPACE_ROOT/target/debug/gsd}"

echo "=== Demo: Linear State Machine (Start -> Middle -> End) ==="
echo "Working directory: $ROOT"
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    kill $AGENT_PID 2>/dev/null || true
    wait $AGENT_PID 2>/dev/null || true
    $AGENT_POOL stop "$ROOT" 2>/dev/null || true
    rm -rf "$ROOT"
    echo "Done."
}
trap cleanup EXIT

# Start agent pool
echo "Starting agent pool..."
$AGENT_POOL start "$ROOT" --log-level "${LOG_LEVEL:-info}" &
POOL_PID=$!
sleep 0.5

# Start GSD-aware agent with transition map
echo "Starting GSD agent with transitions: Start->Middle->End..."
"$SCRIPT_DIR/../scripts/gsd-agent.sh" "$ROOT" "linear-agent" "Start:Middle,Middle:End,End:" 0.1 &
AGENT_PID=$!
sleep 0.3

# Run GSD
echo ""
echo "Running GSD with linear config..."
$GSD run "$SCRIPT_DIR/../configs/linear.json" \
    --root "$ROOT" \
    --initial '[{"kind": "Start", "value": {}}]'

echo ""
echo "=== Success! ==="
