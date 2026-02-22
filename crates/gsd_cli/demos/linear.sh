#!/bin/bash
# Demo: Linear three-step GSD task queue
#
# Usage:
#   ./linear.sh                              # Run with demo agent pool
#   ./linear.sh /path/to/pool                # Run against existing pool
#   ./linear.sh /path/to/pool /path/to/wake  # Run with wake script
#
# This demonstrates a linear task queue:
# Start -> Middle -> End
#
# When using an existing pool, we skip starting the pool and demo agent.
# The wake script is called before GSD starts to notify agents.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../.."

# Check if user provided an existing pool path and wake script
EXISTING_POOL="$1"
WAKE_SCRIPT="$2"

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
    echo "=== Demo: Linear Task Queue (using existing pool) ==="
    echo "Pool directory: $ROOT"
    if [ -n "$WAKE_SCRIPT" ]; then
        echo "Wake script: $WAKE_SCRIPT"
    fi
    echo ""

    # Build wake argument if provided
    WAKE_ARG=""
    if [ -n "$WAKE_SCRIPT" ]; then
        WAKE_ARG="--wake $WAKE_SCRIPT"
    fi

    # Run GSD against existing pool
    echo "Running GSD with linear config..."
    $GSD run "$SCRIPT_DIR/../../gsd_config/configs/linear.json" \
        --root "$ROOT" \
        --initial '[{"kind": "Start", "value": {}}]' \
        $WAKE_ARG

    echo ""
    echo "=== Success! ==="
else
    # Create demo pool
    ROOT=$(mktemp -d)
    echo "=== Demo: Linear Task Queue (Start -> Middle -> End) ==="
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
    $GSD run "$SCRIPT_DIR/../../gsd_config/configs/linear.json" \
        --root "$ROOT" \
        --initial '[{"kind": "Start", "value": {}}]'

    echo ""
    echo "=== Success! ==="
fi
