#!/bin/bash
# Generate GraphViz DOT files for all Barnum demo configs.
#
# Each demo has a barnum.config.ts that defines the config inline.
# This script extracts the config JSON from each and generates a .dot file.
#
# Usage:
#   ./scripts/generate-graphs.sh        # Regenerate all .dot files
#   ./scripts/generate-graphs.sh --check  # Check if .dot files are up-to-date (for CI)

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/.."

# Build barnum if not using environment override
BARNUM="${BARNUM:-}"
if [ -z "$BARNUM" ]; then
    echo "Building barnum..."
    cargo build -p barnum_cli --quiet
    BARNUM="$WORKSPACE_ROOT/target/debug/barnum"
fi

DEMOS_DIR="$WORKSPACE_ROOT/crates/barnum_cli/demos"
TSX="$DEMOS_DIR/node_modules/.bin/tsx"
EXTRACT="$DEMOS_DIR/extract-config.ts"
CHECK_MODE=false

if [ "$1" = "--check" ]; then
    CHECK_MODE=true
fi

if [ ! -x "$TSX" ]; then
    echo "ERROR: tsx not found at $TSX"
    echo "Run: pnpm install (in $DEMOS_DIR)"
    exit 1
fi

# Find all demo directories that have both barnum.config.ts and graph.dot
FAILED=false

for dot_file in $(find "$DEMOS_DIR" -name "graph.dot" | sort); do
    demo_dir=$(dirname "$dot_file")
    config_ts="$demo_dir/barnum.config.ts"
    name=$(basename "$demo_dir")

    if [ ! -f "$config_ts" ]; then
        echo "SKIP: $name (no barnum.config.ts)"
        continue
    fi

    # Extract config JSON from the TypeScript file
    config_json=$("$TSX" "$EXTRACT" "$config_ts")

    # Generate new DOT content
    new_content=$("$BARNUM" config graph --config "$config_json")

    if [ "$CHECK_MODE" = true ]; then
        if [ "$(cat "$dot_file")" != "$new_content" ]; then
            echo "OUTDATED: $dot_file"
            FAILED=true
        else
            echo "OK: $dot_file"
        fi
    else
        echo "$new_content" > "$dot_file"
        echo "Generated: $dot_file"
    fi
done

if [ "$CHECK_MODE" = true ] && [ "$FAILED" = true ]; then
    echo ""
    echo "Some .dot files are out of date. Run: ./scripts/generate-graphs.sh"
    exit 1
fi

echo ""
echo "Done."
