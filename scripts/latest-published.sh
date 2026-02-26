#!/bin/bash
# Show the latest published npm versions and install commands

set -e

echo "=== @gsd-now/gsd ==="
VERSION=$(pnpm view @gsd-now/gsd dist-tags.latest 2>/dev/null || echo "not published")
echo "Latest: $VERSION"
echo "Install: pnpm install @gsd-now/gsd@$VERSION"
echo ""

echo "=== @gsd-now/agent-pool ==="
VERSION=$(pnpm view @gsd-now/agent-pool dist-tags.latest 2>/dev/null || echo "not published")
echo "Latest: $VERSION"
echo "Install: pnpm install @gsd-now/agent-pool@$VERSION"
