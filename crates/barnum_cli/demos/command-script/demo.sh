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

# Run Barnum - pass the demo directory as the folder to scan
echo "Running Barnum with command-script config..."
echo "This will list files in the demo directory and analyze each one."
echo ""

$BARNUM run --config "$SCRIPT_DIR/config.json" \
    --entrypoint-value "{\"folder\": \"$SCRIPT_DIR\"}"

echo ""
echo "=== Success! ==="
