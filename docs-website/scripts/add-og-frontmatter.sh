#!/bin/bash
# Add `image` frontmatter to each docs page pointing to its OG image.
# Usage: bash scripts/add-og-frontmatter.sh
# Idempotent: skips files that already have `image:` in frontmatter.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DOCS_DIR="$SCRIPT_DIR/../docs"
COUNT=0

while IFS= read -r -d '' file; do
  rel="${file#$DOCS_DIR/}"
  slug="${rel%.md}"
  if [[ "$slug" == */index ]]; then
    slug="${slug%/index}"
  fi
  safe_name="${slug//\//-}"
  if [ -z "$safe_name" ] || [ "$safe_name" = "index" ]; then
    safe_name="index"
  fi

  img_path="/img/og/${safe_name}.png"

  # Check if file already has frontmatter with image
  if head -1 "$file" | grep -q '^---$'; then
    if grep -q '^image:' "$file"; then
      echo "SKIP $rel (already has image)"
      continue
    fi
    # Has frontmatter but no image — inject image line after ---
    sed -i '' "1 a\\
image: $img_path
" "$file"
  else
    # No frontmatter — prepend it
    tmp=$(mktemp)
    printf '%s\n' "---" "image: $img_path" "---" "" > "$tmp"
    cat "$file" >> "$tmp"
    mv "$tmp" "$file"
  fi

  COUNT=$((COUNT + 1))
  echo "OK   $rel  -> $img_path"
done < <(find "$DOCS_DIR" -name '*.md' -print0 | sort -z)

echo ""
echo "Updated $COUNT files with OG image frontmatter"
