#!/bin/bash
# Demo: GSD docs and validate commands
#
# This demonstrates the non-runtime GSD commands:
# - gsd validate: Check config validity
# - gsd docs: Generate markdown documentation

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../.."

# Build the binary first
echo "Building gsd..."
cargo build -p gsd_cli --quiet
echo "Build complete."
echo ""

GSD="${GSD:-$WORKSPACE_ROOT/target/debug/gsd}"

echo "=== Demo: GSD Docs and Validate ==="
echo ""

echo "--- Validating simple.json ---"
$GSD validate "$SCRIPT_DIR/../configs/simple.json"
echo ""

echo "--- Validating linear.json ---"
$GSD validate "$SCRIPT_DIR/../configs/linear.json"
echo ""

echo "--- Validating branching.json ---"
$GSD validate "$SCRIPT_DIR/../configs/branching.json"
echo ""

echo "--- Generating docs for linear.json ---"
echo ""
$GSD docs "$SCRIPT_DIR/../configs/linear.json"

echo ""
echo "=== Success! ==="
