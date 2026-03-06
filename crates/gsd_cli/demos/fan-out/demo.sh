#!/bin/bash
# Demo: Fan-out with multiple agents
#
# Usage:
#   ./demo.sh                              # Run with demo agent pool
#   ./demo.sh /path/to/pool                # Run against existing pool
#   ./demo.sh /path/to/pool /path/to/wake  # Run with wake script
#
# This demonstrates parallel processing with fan-out:
# 1. One "Distribute" task spawns 20 "Worker" tasks
# 2. Multiple agents process Worker tasks in parallel
# 3. Watch tasks complete faster with more agents
#
# When using an existing pool, we skip starting the pool and demo agents.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

# Check if user provided an existing pool path and wake script
EXISTING_POOL="$1"
WAKE_SCRIPT="$2"

# Build the binaries first
echo "Building binaries..."
cargo build -p agent_pool -p gsd_cli --quiet
echo "Build complete."
echo ""

export AGENT_POOL="${AGENT_POOL:-$WORKSPACE_ROOT/target/debug/agent_pool}"
export GSD="${GSD:-$WORKSPACE_ROOT/target/debug/gsd}"

NUM_WORKERS=20
NUM_AGENTS=3
WORKER_SLEEP=0.3

if [ -n "$EXISTING_POOL" ]; then
    # Use existing pool
    ROOT="$EXISTING_POOL"
    echo "=== Demo: Fan-Out (using existing pool) ==="
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
    echo "Running GSD with fan-out config..."
    $GSD run "$SCRIPT_DIR/config.jsonc" \
        --pool "$ROOT" \
        --initial-state '[{"kind": "Distribute", "value": {}}]' \
        $WAKE_ARG

    echo ""
    echo "=== Success! ==="
    echo ""
    echo "View workflow graph: $SCRIPT_DIR/graph.dot"
else
    # Create demo pool
    POOL_ROOT=$(mktemp -d)
    POOL_ID="demo"
    echo "=== Demo: Fan-Out with Multiple Agents ==="
    echo "Working directory: $POOL_ROOT"
    echo ""
    echo "This demo:"
    echo "  1. Starts $NUM_AGENTS agents"
    echo "  2. Submits 1 Distribute task"
    echo "  3. Distribute fans out to $NUM_WORKERS Worker tasks"
    echo "  4. Agents process Worker tasks in parallel"
    echo ""

    AGENT_PIDS=""

    cleanup() {
        echo ""
        echo "=== Cleaning up ==="
        $AGENT_POOL --pool-root "$POOL_ROOT" stop --pool "$POOL_ID" 2>/dev/null || true
        sleep 0.2
        for pid in $AGENT_PIDS; do
            kill -9 $pid 2>/dev/null || true
        done
        for pid in $AGENT_PIDS; do
            wait $pid 2>/dev/null || true
        done
        rm -rf "$POOL_ROOT"
        echo "Done."
    }
    trap cleanup EXIT

    # Start agent pool
    echo "Starting agent pool..."
    $AGENT_POOL --pool-root "$POOL_ROOT" start --pool "$POOL_ID" --log-level "${LOG_LEVEL:-warn}" &
    POOL_PID=$!
    sleep 0.5

    # Start multiple agents
    echo "Starting $NUM_AGENTS agents..."
    for i in $(seq 1 $NUM_AGENTS); do
        "$SCRIPT_DIR/../../scripts/fan-out-agent.sh" --pool-root "$POOL_ROOT" --pool "$POOL_ID" --name "agent-$i" --workers "$NUM_WORKERS" --sleep "$WORKER_SLEEP" &
        AGENT_PIDS="$AGENT_PIDS $!"
    done
    sleep 0.3

    # Run GSD
    echo ""
    echo "Running GSD with fan-out config..."
    echo "  Distribute -> $NUM_WORKERS Worker tasks -> done"
    echo ""

    START_TIME=$(date +%s.%N)

    $GSD --pool-root "$POOL_ROOT" run "$SCRIPT_DIR/config.jsonc" \
        --pool "$POOL_ID" \
        --initial-state '[{"kind": "Distribute", "value": {}}]'

    END_TIME=$(date +%s.%N)
    ELAPSED=$(echo "$END_TIME - $START_TIME" | bc 2>/dev/null || echo "?")

    echo ""
    echo "=== Success! ==="
    echo "Processed $NUM_WORKERS tasks with $NUM_AGENTS agents in ${ELAPSED}s"
    echo ""
    echo "View workflow graph: $SCRIPT_DIR/graph.dot"
fi
