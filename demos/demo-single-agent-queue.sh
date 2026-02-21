#!/bin/bash
# Demo: Single agent, many tasks (queuing)
#
# This demonstrates the queue behavior:
# 1. Start the multiplexer
# 2. Start ONE agent with 1 second sleep
# 3. Submit 4 tasks rapidly
# 4. Watch them process sequentially
# 5. Clean up

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT=$(mktemp -d)
MULTIPLEXER="${MULTIPLEXER:-cargo run -p gsd_multiplexer --}"

echo "=== Demo: Single Agent Queue ==="
echo "Working directory: $ROOT"
echo "Tasks will queue and process one at a time."
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

# Start ONE agent with slow response time
echo "Starting single agent (1 second per task)..."
"$SCRIPT_DIR/agent.sh" "$ROOT" "only-agent" 1.0 &
AGENT_PID=$!
sleep 0.3

# Submit tasks rapidly
echo ""
echo "Submitting 4 tasks rapidly..."
echo "Watch them complete one by one (~1 second apart)"
echo ""

submit_task() {
    local task="$1"
    local start=$(date +%s.%N)
    result=$($MULTIPLEXER submit "$ROOT" "$task")
    local end=$(date +%s.%N)
    local elapsed=$(echo "$end - $start" | bc)
    echo "[${elapsed}s] $result"
}

submit_task "Task-A" &
submit_task "Task-B" &
submit_task "Task-C" &
submit_task "Task-D" &

# Wait for all submits to complete
wait

echo ""
echo "=== All tasks completed! ==="
echo "Notice tasks completed ~1 second apart (single-threaded)."
