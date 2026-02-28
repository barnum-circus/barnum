#!/usr/bin/env bash
# Run all demo scripts across the workspace.
#
# Usage:
#   ./scripts/run-all-demos.sh
#
# Environment variables (optional):
#   AGENT_POOL - path to agent_pool binary (default: searches PATH, then target/debug)
#   GSD - path to gsd binary (default: searches PATH, then target/debug)

set -euo pipefail

# Color output (disabled if not a terminal)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    NC='\033[0m'
else
    RED=''
    GREEN=''
    YELLOW=''
    NC=''
fi

# Find project root (where Cargo.toml is)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Track results
PASSED=0
FAILED=0
FAILED_DEMOS=""

echo "=== Running all demos ==="
echo ""

# Find all demo scripts (portable version)
# Note: refactor-workflow is excluded because it requires a real codebase and Claude agents
DEMOS=$(find "$PROJECT_ROOT/crates" -path "*/demos/*/demo.sh" -type f | grep -v refactor-workflow | sort)

if [ -z "$DEMOS" ]; then
    echo -e "${YELLOW}No demos found${NC}"
    exit 0
fi

DEMO_COUNT=$(echo "$DEMOS" | wc -l | tr -d ' ')
echo "Found $DEMO_COUNT demo(s):"
echo "$DEMOS" | while read -r demo; do
    echo "  - ${demo#$PROJECT_ROOT/}"
done
echo ""

# Run each demo
echo "$DEMOS" | while read -r demo; do
    demo_name="${demo#$PROJECT_ROOT/}"
    echo -e "${YELLOW}>>> Running $demo_name${NC}"

    if bash "$demo"; then
        echo -e "${GREEN}<<< PASSED: $demo_name${NC}"
    else
        echo -e "${RED}<<< FAILED: $demo_name${NC}"
        # Write to a temp file to track failures across subshell
        echo "$demo_name" >> "$PROJECT_ROOT/.demo-failures.tmp"
    fi
    echo ""
done

# Check for failures
if [ -f "$PROJECT_ROOT/.demo-failures.tmp" ]; then
    FAILED_COUNT=$(wc -l < "$PROJECT_ROOT/.demo-failures.tmp" | tr -d ' ')
    echo "=== Summary ==="
    echo -e "${RED}Failed: $FAILED_COUNT${NC}"
    echo ""
    echo "Failed demos:"
    cat "$PROJECT_ROOT/.demo-failures.tmp" | while read -r failed; do
        echo "  - $failed"
    done
    rm -f "$PROJECT_ROOT/.demo-failures.tmp"
    exit 1
else
    echo "=== Summary ==="
    echo -e "${GREEN}All demos passed!${NC}"
fi
