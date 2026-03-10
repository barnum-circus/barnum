#!/bin/bash
set -e

BARNUM=$(pnpm view @barnum/barnum dist-tags.main --registry https://registry.npmjs.org 2>/dev/null)
TROUPE=$(pnpm view @barnum/troupe dist-tags.main --registry https://registry.npmjs.org 2>/dev/null)

LOCAL_MASTER=$(git rev-parse --short master 2>/dev/null || echo "unknown")
REMOTE_MASTER=$(git rev-parse --short origin/master 2>/dev/null || echo "unknown")

echo "Local master:  $LOCAL_MASTER"
echo "Remote master: $REMOTE_MASTER"
echo ""
echo "Published: $BARNUM"
echo ""
echo "pnpm install @barnum/barnum@$BARNUM --registry https://registry.npmjs.org"
echo "pnpm install @barnum/troupe@$TROUPE --registry https://registry.npmjs.org"
