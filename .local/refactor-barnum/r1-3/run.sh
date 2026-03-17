#!/bin/bash
# Loop over R9 branches, using parents.json for parent lookups.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CONFIG="$SCRIPT_DIR/config.jsonc"
PARENTS="$SCRIPT_DIR/parents.json"
WEBAPP="${WEBAPP:-$HOME/code/pinboard/webapp}"
cd "$WEBAPP"

if [ ! -f "$PARENTS" ]; then
  echo "ERROR: $PARENTS not found" >&2
  exit 1
fi

echo "Resetting all branches to known-good SHAs..."
bash "$SCRIPT_DIR/reset-branches.sh"
echo ""

for i in $(seq 1 174); do
  BRANCH="rb/r9/$i"
  PARENT=$(python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(d.get(sys.argv[2],''))" "$PARENTS" "$BRANCH")

  if [ -z "$PARENT" ]; then
    echo "[$BRANCH] No parent in parents.json, skipping"
    continue
  fi

  echo "[$BRANCH] parent: $PARENT, processing..."

  pnpm dlx @barnum/barnum run \
    --config "$CONFIG" \
    --entrypoint-value "{\"branch_name\":\"$BRANCH\",\"parent_branch\":\"$PARENT\",\"local_dir\":\"$WEBAPP\"}"

  echo "[$BRANCH] Done"
  echo ""
done
