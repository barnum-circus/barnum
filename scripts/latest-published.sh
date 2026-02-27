#!/bin/bash
set -e

GSD=$(pnpm view @gsd-now/gsd dist-tags.main --registry https://registry.npmjs.org 2>/dev/null)
AGENT_POOL=$(pnpm view @gsd-now/agent-pool dist-tags.main --registry https://registry.npmjs.org 2>/dev/null)

echo "$GSD"
echo ""
echo "pnpm install @gsd-now/gsd@$GSD --registry https://registry.npmjs.org"
echo "pnpm install @gsd-now/agent-pool@$AGENT_POOL --registry https://registry.npmjs.org"
