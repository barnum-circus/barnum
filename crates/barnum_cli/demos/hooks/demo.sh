#!/bin/bash
# Demo: Pre-hook, post-hook, and finally-hook execution
#
# Shows how hooks transform data at each stage of task processing:
# - pre-hook: transforms input before action runs
# - post-hook: can modify output/spawned tasks after action completes
# - finally: runs after task and all children complete

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

# Build the binaries first
echo "Building binaries..."
cargo build -p barnum_cli --quiet
echo "Build complete."
echo ""

export BARNUM="${BARNUM:-$WORKSPACE_ROOT/target/debug/barnum}"

echo "=== Demo: Pre/Post/Finally Hooks ==="
echo ""

# Create a temp pool directory
POOL_ROOT=$(mktemp -d)
POOL_ID="demo"

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    rm -rf "$POOL_ROOT"
    echo "Done."
}
trap cleanup EXIT

echo "Running Barnum with hooks config..."
echo "Watch for hook messages in the output."
echo ""

$BARNUM --root "$POOL_ROOT" run --config "$SCRIPT_DIR/config.jsonc" \
    --pool "$POOL_ID" \
    --initial-state '[{"kind": "Process", "value": {"item": "test-item"}}]'

echo ""
echo "=== Success! ==="
echo ""
echo "Hook execution order:"
echo "1. Pre-hook: Added timestamp to input"
echo "2. Process action: Processed the item"
echo "3. Post-hook: Logged completion, passed through spawned tasks"
echo "4. Cleanup action: Child task ran"
echo "5. Finally hook: Ran after Process and its child Cleanup completed"
