#!/bin/bash
# Demo: Command actions (no agent pool needed)
#
# This demo shows how to use command actions to transform data locally
# using jq, without needing an agent pool or LLM.
#
# The workflow:
#   Split -> Process (x3) -> Collect (x3)
#
# Split takes an array of items and fans out to Process
# Process doubles each number
# Collect terminates

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

# Build the binary
echo "Building barnum_cli..."
cargo build -p barnum_cli --quiet
echo "Build complete."
echo ""

BARNUM="${BARNUM:-$WORKSPACE_ROOT/target/debug/barnum}"

echo "=== Demo: Command Actions ==="
echo ""
echo "This demo uses 'command' actions (local scripts) instead of 'pool' actions."
echo "No agent pool is needed - tasks are processed by jq commands."
echo ""

# Create a temp directory for the pool root (even though we don't use the pool)
POOL_ROOT=$(mktemp -d)
POOL_ID="demo"
cleanup() {
    rm -rf "$POOL_ROOT"
}
trap cleanup EXIT

# Run Barnum with command demo config
echo "Running Barnum with command-demo config..."
echo ""
echo "Initial task: Split with items [{n:1}, {n:2}, {n:3}]"
echo ""

$BARNUM --root "$POOL_ROOT" run --config "$SCRIPT_DIR/config.jsonc" \
    --pool "$POOL_ID" \
    --entrypoint-value '{"items": [{"n": 1}, {"n": 2}, {"n": 3}]}'

echo ""
echo "=== Success! ==="
echo ""
echo "The workflow processed 3 items through local jq commands:"
echo "  Split -> Process x3 -> Collect x3"
echo ""
echo "View workflow graph: $SCRIPT_DIR/graph.dot"
