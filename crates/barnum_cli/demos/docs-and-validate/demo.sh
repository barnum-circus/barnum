#!/bin/bash
# Demo: Barnum config docs and validate commands
#
# This demonstrates the non-runtime Barnum commands:
# - barnum config validate: Check config validity
# - barnum config docs: Generate markdown documentation

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

# Build the binary first
echo "Building barnum..."
cargo build -p barnum_cli --quiet
echo "Build complete."
echo ""

BARNUM="${BARNUM:-$WORKSPACE_ROOT/target/debug/barnum}"

echo "=== Demo: Barnum Config Docs and Validate ==="
echo ""

echo "--- Validating simple.jsonc ---"
$BARNUM config validate "$SCRIPT_DIR/../simple/config.jsonc"
echo ""

echo "--- Validating linear.jsonc ---"
$BARNUM config validate "$SCRIPT_DIR/../linear/config.jsonc"
echo ""

echo "--- Validating branching.jsonc ---"
$BARNUM config validate "$SCRIPT_DIR/../branching/config.jsonc"
echo ""

echo "--- Generating docs for linear.jsonc ---"
echo ""
$BARNUM config docs "$SCRIPT_DIR/../linear/config.jsonc"

echo ""
echo "=== Success! ==="
