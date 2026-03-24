#!/bin/bash
# Demo: Finally-hook execution
#
# Shows how the finally hook runs after a task and all its children complete.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DEMOS_DIR="$SCRIPT_DIR/.."
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

# Build the binaries first
echo "Building binaries..."
cargo build -p barnum_cli --quiet
echo "Build complete."
echo ""

export BARNUM="${BARNUM:-$WORKSPACE_ROOT/target/debug/barnum}"

echo "=== Demo: Finally Hooks ==="
echo ""

echo "Running Barnum with hooks config..."
echo "Watch for hook messages in the output."
echo ""

"$DEMOS_DIR/node_modules/.bin/tsx" "$SCRIPT_DIR/barnum.config.ts"

echo ""
echo "=== Success! ==="
echo ""
echo "Execution order:"
echo "1. Process action: Processed the item"
echo "2. Cleanup action: Child task ran"
echo "3. Finally hook: Ran after Process and its child Cleanup completed"
