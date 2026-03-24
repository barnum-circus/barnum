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

SIMPLE='{"entrypoint":"Start","steps":[{"name":"Start","action":{"kind":"Bash","script":"echo []"},"next":[]}]}'
LINEAR='{"entrypoint":"Start","steps":[{"name":"Start","action":{"kind":"Bash","script":"echo []"},"next":["Middle"]},{"name":"Middle","action":{"kind":"Bash","script":"echo []"},"next":["End"]},{"name":"End","action":{"kind":"Bash","script":"echo []"},"next":[]}]}'
BRANCHING='{"entrypoint":"Decide","steps":[{"name":"Decide","action":{"kind":"Bash","script":"echo []"},"next":["PathA","PathB"]},{"name":"PathA","action":{"kind":"Bash","script":"echo []"},"next":["Done"]},{"name":"PathB","action":{"kind":"Bash","script":"echo []"},"next":["Done"]},{"name":"Done","action":{"kind":"Bash","script":"echo []"},"next":[]}]}'

echo "--- Validating simple config ---"
$BARNUM config validate --config "$SIMPLE"
echo ""

echo "--- Validating linear config ---"
$BARNUM config validate --config "$LINEAR"
echo ""

echo "--- Validating branching config ---"
$BARNUM config validate --config "$BRANCHING"
echo ""

echo "--- Generating docs for linear config ---"
echo ""
$BARNUM config docs --config "$LINEAR"

echo ""
echo "=== Success! ==="
