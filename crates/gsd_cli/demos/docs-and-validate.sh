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

echo "--- Validating simple.jsonc ---"
$GSD validate "$SCRIPT_DIR/configs/simple.jsonc"
echo ""

echo "--- Validating linear.jsonc ---"
$GSD validate "$SCRIPT_DIR/configs/linear.jsonc"
echo ""

echo "--- Validating branching.jsonc ---"
$GSD validate "$SCRIPT_DIR/configs/branching.jsonc"
echo ""

echo "--- Generating docs for linear.jsonc ---"
echo ""
$GSD docs "$SCRIPT_DIR/configs/linear.jsonc"

echo ""
echo "=== Success! ==="
