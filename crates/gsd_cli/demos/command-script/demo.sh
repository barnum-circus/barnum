#!/bin/bash
# Demo: Command script execution with relative paths
#
# Tests that Command actions can use scripts with relative paths from the config directory.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

# Build the binaries first
echo "Building binaries..."
cargo build -p gsd_cli --quiet
echo "Build complete."
echo ""

export GSD="${GSD:-$WORKSPACE_ROOT/target/debug/gsd}"

echo "=== Demo: Command Script with Relative Paths ==="
echo "Config directory: $SCRIPT_DIR"
echo ""

# Create a temp pool directory (needed by GSD even for Command actions)
POOL_ROOT=$(mktemp -d)
POOL_ID="demo"

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    rm -rf "$POOL_ROOT"
    echo "Done."
}
trap cleanup EXIT

# Run GSD - pass the demo directory as the folder to scan
echo "Running GSD with command-script config..."
echo "This will list files in the demo directory and analyze each one."
echo ""

$GSD --pool-root "$POOL_ROOT" run "$SCRIPT_DIR/config.jsonc" \
    --pool "$POOL_ID" \
    --initial "[{\"kind\": \"ListFiles\", \"value\": {\"folder\": \"$SCRIPT_DIR\"}}]"

echo ""
echo "=== Success! ==="
