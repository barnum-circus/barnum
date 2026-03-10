#!/bin/bash
# Demo: Command script execution with relative paths
#
# Tests that Command actions can use scripts with relative paths from the config directory.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

# Build the binaries first
echo "Building binaries..."
cargo build -p barnum_cli --quiet
echo "Build complete."
echo ""

export BARNUM="${BARNUM:-$WORKSPACE_ROOT/target/debug/barnum}"

echo "=== Demo: Command Script with Relative Paths ==="
echo "Config directory: $SCRIPT_DIR"
echo ""

# Create a temp pool directory (needed by Barnum even for Command actions)
POOL_ROOT=$(mktemp -d)
POOL_ID="demo"

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    rm -rf "$POOL_ROOT"
    echo "Done."
}
trap cleanup EXIT

# Run Barnum - pass the demo directory as the folder to scan
echo "Running Barnum with command-script config..."
echo "This will list files in the demo directory and analyze each one."
echo ""

$BARNUM --root "$POOL_ROOT" run --config "$SCRIPT_DIR/config.jsonc" \
    --pool "$POOL_ID" \
    --initial-state "[{\"kind\": \"ListFiles\", \"value\": {\"folder\": \"$SCRIPT_DIR\"}}]"

echo ""
echo "=== Success! ==="
