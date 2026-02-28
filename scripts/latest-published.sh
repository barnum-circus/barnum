#!/bin/bash
set -e

GSD=$(pnpm view @gsd-now/gsd dist-tags.main --registry https://registry.npmjs.org 2>/dev/null)
AGENT_POOL=$(pnpm view @gsd-now/agent-pool dist-tags.main --registry https://registry.npmjs.org 2>/dev/null)

LOCAL_MASTER=$(git rev-parse --short master 2>/dev/null || echo "unknown")
REMOTE_MASTER=$(git rev-parse --short origin/master 2>/dev/null || echo "unknown")

echo "Local master:  $LOCAL_MASTER"
echo "Remote master: $REMOTE_MASTER"
echo ""
echo "Published: $GSD"
echo ""
echo "pnpm install @gsd-now/gsd@$GSD --registry https://registry.npmjs.org"
echo "pnpm install @gsd-now/agent-pool@$AGENT_POOL --registry https://registry.npmjs.org"
