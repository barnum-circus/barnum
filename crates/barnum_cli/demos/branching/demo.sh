#!/bin/bash
# Demo: Branching Barnum task queue
#
# Usage:
#   ./demo.sh                              # Run with demo agent pool
#   ./demo.sh /path/to/pool                # Run against existing pool
#   ./demo.sh /path/to/pool /path/to/wake  # Run with wake script
#
# This demonstrates a branching task queue:
# Decide -> PathA or PathB -> Done
#
# When using an existing pool, we skip starting the pool and demo agent.
# The agent always chooses PathA in the demo mode.
# The wake script is called before Barnum starts to notify agents.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

# Check if user provided an existing pool path and wake script
EXISTING_POOL="$1"
WAKE_SCRIPT="$2"

# Build the binaries first
echo "Building binaries..."
cargo build -p troupe -p barnum_cli --quiet
echo "Build complete."
echo ""

export TROUPE="${TROUPE:-$WORKSPACE_ROOT/target/debug/troupe}"
export BARNUM="${BARNUM:-$WORKSPACE_ROOT/target/debug/barnum}"

if [ -n "$EXISTING_POOL" ]; then
    # Use existing pool
    ROOT="$EXISTING_POOL"
    echo "=== Demo: Branching Task Queue (using existing pool) ==="
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

    # Run Barnum against existing pool
    echo "Running Barnum with branching config..."
    $BARNUM run --config "$SCRIPT_DIR/config.jsonc" \
        --pool "$ROOT" \
        --initial-state '[{"kind": "Decide", "value": {}}]' \
        $WAKE_ARG

    echo ""
    echo "=== Success! ==="
    echo ""
    echo "View workflow graph: $SCRIPT_DIR/graph.dot"
else
    # Create demo pool
    POOL_ROOT=$(mktemp -d)
    POOL_ID="demo"
    echo "=== Demo: Branching Task Queue ==="
    echo "Working directory: $POOL_ROOT"
    echo ""

    cleanup() {
        echo ""
        echo "=== Cleaning up ==="
        $TROUPE --root "$POOL_ROOT" stop --pool "$POOL_ID" 2>/dev/null || true
        sleep 0.2
        kill -9 $AGENT_PID 2>/dev/null || true
        wait $AGENT_PID 2>/dev/null || true
        rm -rf "$POOL_ROOT"
        echo "Done."
    }
    trap cleanup EXIT

    # Start agent pool
    echo "Starting agent pool..."
    $TROUPE --root "$POOL_ROOT" start --pool "$POOL_ID" --log-level "${LOG_LEVEL:-info}" &
    POOL_PID=$!
    sleep 0.5

    # Start Barnum-aware agent that chooses PathA
    echo "Starting Barnum agent (will choose PathA)..."
    "$SCRIPT_DIR/../../scripts/barnum-agent.sh" --root "$POOL_ROOT" --pool "$POOL_ID" --name "branching-agent" --transitions "Decide:PathA,PathA:Done,Done:" --sleep 0.1 &
    AGENT_PID=$!
    sleep 0.3

    # Run Barnum
    echo ""
    echo "Running Barnum with branching config..."
    $BARNUM --root "$POOL_ROOT" run --config "$SCRIPT_DIR/config.jsonc" \
        --pool "$POOL_ID" \
        --initial-state '[{"kind": "Decide", "value": {}}]'

    echo ""
    echo "=== Success! ==="
    echo ""
    echo "View workflow graph: $SCRIPT_DIR/graph.dot"
fi
