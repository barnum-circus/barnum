#!/bin/bash
# Demo: GSD config docs and validate commands
#
# This demonstrates the non-runtime GSD commands:
# - gsd config validate: Check config validity
# - gsd config docs: Generate markdown documentation

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../../../.."

# Build the binary first
echo "Building gsd..."
cargo build -p gsd_cli --quiet
echo "Build complete."
echo ""

GSD="${GSD:-$WORKSPACE_ROOT/target/debug/gsd}"

echo "=== Demo: GSD Config Docs and Validate ==="
echo ""

echo "--- Validating simple.jsonc ---"
$GSD config validate "$SCRIPT_DIR/../simple/config.jsonc"
echo ""

echo "--- Validating linear.jsonc ---"
$GSD config validate "$SCRIPT_DIR/../linear/config.jsonc"
echo ""

echo "--- Validating branching.jsonc ---"
$GSD config validate "$SCRIPT_DIR/../branching/config.jsonc"
echo ""

echo "--- Generating docs for linear.jsonc ---"
echo ""
$GSD config docs "$SCRIPT_DIR/../linear/config.jsonc"

echo ""
echo "=== Success! ==="
