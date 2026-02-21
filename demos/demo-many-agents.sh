#!/bin/bash
# Demo: Multiple agents, many tasks
#
# This demonstrates parallel processing:
# 1. Start the multiplexer
# 2. Start 3 agents with random sleep times
# 3. Submit 6 tasks rapidly (without waiting)
# 4. Watch responses come back interleaved
# 5. Clean up

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT=$(mktemp -d)
MULTIPLEXER="${MULTIPLEXER:-cargo run -p gsd_multiplexer --}"

echo "=== Demo: Multiple Agents, Many Tasks ==="
echo "Working directory: $ROOT"
echo ""

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    kill $AGENT1_PID $AGENT2_PID $AGENT3_PID 2>/dev/null || true
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

# Start agents with different sleep times
echo "Starting agents with varying response times..."
"$SCRIPT_DIR/agent.sh" "$ROOT" "fast-agent" 0.1 &
AGENT1_PID=$!
"$SCRIPT_DIR/agent.sh" "$ROOT" "medium-agent" 0.3 &
AGENT2_PID=$!
"$SCRIPT_DIR/agent.sh" "$ROOT" "slow-agent" 0.5 &
AGENT3_PID=$!
sleep 0.3

# Submit tasks rapidly (in background so they're concurrent)
echo ""
echo "Submitting 6 tasks rapidly..."
echo ""

submit_task() {
    local task="$1"
    result=$($MULTIPLEXER submit "$ROOT" "$task")
    echo "Result: $result"
}

submit_task "Task-1" &
submit_task "Task-2" &
submit_task "Task-3" &
submit_task "Task-4" &
submit_task "Task-5" &
submit_task "Task-6" &

# Wait for all submits to complete
wait

echo ""
echo "=== All tasks completed! ==="
