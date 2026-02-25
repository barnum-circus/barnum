#!/bin/bash
# Generate GraphViz DOT files for all GSD configs.
#
# Usage:
#   ./scripts/generate-graphs.sh        # Regenerate all .dot files
#   ./scripts/generate-graphs.sh --check  # Check if .dot files are up-to-date (for CI)

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/.."

# Build gsd if not using environment override
GSD="${GSD:-}"
if [ -z "$GSD" ]; then
    echo "Building gsd..."
    cargo build -p gsd_cli --quiet
    GSD="$WORKSPACE_ROOT/target/debug/gsd"
fi

CONFIGS_DIR="$WORKSPACE_ROOT/crates/gsd_cli/demos/configs"
CHECK_MODE=false

if [ "$1" = "--check" ]; then
    CHECK_MODE=true
fi

# Find all JSON config files
CONFIGS=$(find "$CONFIGS_DIR" -name "*.json" | sort)

FAILED=false

for config in $CONFIGS; do
    name=$(basename "$config" .json)
    dot_file="$CONFIGS_DIR/$name.dot"

    # Generate new content
    new_content=$("$GSD" graph "$config")

    if [ "$CHECK_MODE" = true ]; then
        # Check mode: compare with existing
        if [ ! -f "$dot_file" ]; then
            echo "MISSING: $dot_file"
            FAILED=true
        elif [ "$(cat "$dot_file")" != "$new_content" ]; then
            echo "OUTDATED: $dot_file"
            FAILED=true
        else
            echo "OK: $dot_file"
        fi
    else
        # Generate mode: write file
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
