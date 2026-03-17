#!/bin/bash
# Reset rb/r9/* branches to their known-good SHAs from original-shas.json.
# Usage: bash reset-branches.sh [start]
#   start: branch number to begin at (default: 1). Branches before start are left untouched.
set -euo pipefail

START=${1:-1}
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SHAS="$SCRIPT_DIR/original-shas.json"
WEBAPP="${WEBAPP:-$HOME/code/pinboard/webapp}"

if [ ! -f "$SHAS" ]; then
  echo "ERROR: $SHAS not found" >&2
  exit 1
fi

cd "$WEBAPP"

# Abort any in-progress rebase or cherry-pick
git -c core.hooksPath=/dev/null rebase --abort >/dev/null 2>&1 || true
git -c core.hooksPath=/dev/null cherry-pick --abort >/dev/null 2>&1 || true

COUNT=0
FIXED=0
for i in $(seq "$START" 174); do
  BRANCH="rb/r9/$i"
  ORIGINAL_SHA=$(python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(d.get(sys.argv[2],''))" "$SHAS" "$BRANCH")

  if [ -z "$ORIGINAL_SHA" ]; then
    echo "[$BRANCH] No SHA in original-shas.json, skipping"
    continue
  fi

  CURRENT_SHA=$(git -c core.hooksPath=/dev/null rev-parse "refs/heads/$BRANCH" 2>/dev/null || echo "MISSING")

  if [ "$CURRENT_SHA" = "$ORIGINAL_SHA" ]; then
    COUNT=$((COUNT + 1))
    continue
  fi

  if [ "$CURRENT_SHA" = "MISSING" ]; then
    echo "[$BRANCH] Branch missing, creating at $ORIGINAL_SHA"
    git -c core.hooksPath=/dev/null branch "$BRANCH" "$ORIGINAL_SHA"
  else
    echo "[$BRANCH] Resetting $CURRENT_SHA -> $ORIGINAL_SHA"
    git -c core.hooksPath=/dev/null update-ref "refs/heads/$BRANCH" "$ORIGINAL_SHA"
  fi
  FIXED=$((FIXED + 1))
done

echo "Done: $FIXED branches reset, $COUNT already correct"
