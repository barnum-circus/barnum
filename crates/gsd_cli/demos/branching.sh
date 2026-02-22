#!/bin/bash
# Demo: Branching GSD task queue
#
# Usage:
#   ./branching.sh                    # Run with demo agent pool
#   ./branching.sh /path/to/pool      # Run against existing agent pool (e.g., Claude Code)
#
# This demonstrates a branching task queue:
# Decide -> PathA or PathB -> Done
#
# When using an existing pool, we skip starting the pool and demo agent.
# The agent always chooses PathA in the demo mode.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../.."

# Check if user provided an existing pool path
EXISTING_POOL="$1"

# Build the binaries first
echo "Building binaries..."
cargo build -p agent_pool -p gsd_cli --quiet
echo "Build complete."
echo ""

AGENT_POOL="${AGENT_POOL:-$WORKSPACE_ROOT/target/debug/agent_pool}"
GSD="${GSD:-$WORKSPACE_ROOT/target/debug/gsd}"

if [ -n "$EXISTING_POOL" ]; then
    # Use existing pool
    ROOT="$EXISTING_POOL"
    echo "=== Demo: Branching Task Queue (using existing pool) ==="
    echo "Pool directory: $ROOT"
    echo ""

    # Run GSD against existing pool
    echo "Running GSD with branching config..."
    $GSD run "$SCRIPT_DIR/../../gsd_config/configs/branching.json" \
        --root "$ROOT" \
        --initial '[{"kind": "Decide", "value": {}}]'

    echo ""
    echo "=== Success! ==="
else
    # Create demo pool
    ROOT=$(mktemp -d)
    echo "=== Demo: Branching Task Queue ==="
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

    # Start GSD-aware agent that chooses PathA
    echo "Starting GSD agent (will choose PathA)..."
    "$SCRIPT_DIR/../scripts/gsd-agent.sh" "$ROOT" "branching-agent" "Decide:PathA,PathA:Done,Done:" 0.1 &
    AGENT_PID=$!
    sleep 0.3

    # Run GSD
    echo ""
    echo "Running GSD with branching config..."
    $GSD run "$SCRIPT_DIR/../../gsd_config/configs/branching.json" \
        --root "$ROOT" \
        --initial '[{"kind": "Decide", "value": {}}]'

    echo ""
    echo "=== Success! ==="
fi
