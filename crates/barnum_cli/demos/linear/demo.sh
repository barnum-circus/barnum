#!/bin/bash
# Demo: Linear three-step Barnum task queue
#
# Usage:
#   ./demo.sh                              # Run with demo agent pool
#   ./demo.sh /path/to/pool                # Run against existing pool
#   ./demo.sh /path/to/pool /path/to/wake  # Run with wake script
#
# This demonstrates a linear task queue:
# Start -> Middle -> End
#
# When using an existing pool, we skip starting the pool and demo agent.
# The wake script is called before Barnum starts to notify agents.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DEMOS_DIR="$SCRIPT_DIR/.."
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
    # Use existing pool — decompose path into root (parent) and pool (basename)
    POOL_ROOT="$(dirname "$EXISTING_POOL")"
    POOL_ID="$(basename "$EXISTING_POOL")"
    export BARNUM_POOL="$POOL_ID"
    export BARNUM_ROOT="$POOL_ROOT"

    echo "=== Demo: Linear Task Queue (using existing pool) ==="
    echo "Pool directory: $EXISTING_POOL"
    if [ -n "$WAKE_SCRIPT" ]; then
        echo "Wake script: $WAKE_SCRIPT"
    fi
    echo ""

    # Pass wake script via env var (read by barnum.config.ts)
    if [ -n "$WAKE_SCRIPT" ]; then
        export BARNUM_WAKE="$WAKE_SCRIPT"
    fi

    # Run Barnum
    echo "Running Barnum with linear config..."
    "$DEMOS_DIR/node_modules/.bin/tsx" "$SCRIPT_DIR/barnum.config.ts"

    echo ""
    echo "=== Success! ==="
    echo ""
    echo "View workflow graph: $SCRIPT_DIR/graph.dot"
else
    # Create demo pool
    POOL_ROOT=$(mktemp -d)
    POOL_ID="demo"
    export BARNUM_POOL="$POOL_ID"
    export BARNUM_ROOT="$POOL_ROOT"

    echo "=== Demo: Linear Task Queue (Start -> Middle -> End) ==="
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

    # Start Barnum-aware agent with transition map
    echo "Starting Barnum agent with transitions: Start->Middle->End..."
    "$SCRIPT_DIR/../../scripts/barnum-agent.sh" --root "$POOL_ROOT" --pool "$POOL_ID" --name "linear-agent" --transitions "Start:Middle,Middle:End,End:" --sleep 0.1 &
    AGENT_PID=$!
    sleep 0.3

    # Run Barnum
    echo ""
    echo "Running Barnum with linear config..."
    "$DEMOS_DIR/node_modules/.bin/tsx" "$SCRIPT_DIR/barnum.config.ts"

    echo ""
    echo "=== Success! ==="
    echo ""
    echo "View workflow graph: $SCRIPT_DIR/graph.dot"
fi
