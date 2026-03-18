#!/bin/bash
# Generate OG preview images for every docs page.
# Usage: bash scripts/generate-og-images.sh
#
# Requires: ImageMagick 7 (`magick`)
#
# Generates 1200x630 PNGs in static/img/og/ with the page title overlaid
# on the Barnum brand background. Each doc page gets its own image.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DOCS_DIR="$SCRIPT_DIR/../docs"
OUT_DIR="$SCRIPT_DIR/../static/img/og"
TENT="$SCRIPT_DIR/../static/img/tent.png"
mkdir -p "$OUT_DIR"

BG_COLOR="#1A1A2E"
TITLE_COLOR="white"
SUBTITLE_COLOR="#9999AA"
BRAND_COLOR="#9999AA"
TENT_HEIGHT=150

# Extract the title from a markdown file.
# Tries YAML frontmatter `title:` first, then first `# Heading`.
get_title() {
  local file="$1"
  # Check for frontmatter title
  local fm_title
  fm_title=$(awk '/^---$/{if(++c==1)next; if(c==2)exit} c==1 && /^title:/{sub(/^title:[[:space:]]*["'"'"']?/,""); sub(/["'"'"']?[[:space:]]*$/,""); print; exit}' "$file")
  if [ -n "$fm_title" ]; then
    echo "$fm_title"
    return
  fi
  # Fall back to first # heading
  grep -m1 '^# ' "$file" | sed 's/^# //'
}

# Generate a single OG image.
# Args: title, subtitle, output_path
generate_image() {
  local title="$1"
  local subtitle="$2"
  local output="$3"

  magick -size 1200x630 "xc:$BG_COLOR" \
    \( "$TENT" -resize "x${TENT_HEIGHT}" \) -gravity North -geometry +0+40 -composite \
    -font "Helvetica-Neue-Bold" -pointsize 48 -fill "$TITLE_COLOR" \
    -gravity North -annotate +0+220 "$title" \
    -font "Helvetica-Neue" -pointsize 26 -fill "$SUBTITLE_COLOR" \
    -gravity North -annotate +0+290 "$subtitle" \
    -font "Helvetica-Neue-Bold" -pointsize 28 -fill "$BRAND_COLOR" \
    -gravity South -annotate +0+50 "Barnum" \
    "$output"
}

# Word-wrap a title to fit within ~30 chars per line.
wrap_title() {
  local title="$1"
  local max_chars=35
  local line=""
  local result=""

  for word in $title; do
    if [ -z "$line" ]; then
      line="$word"
    elif [ $(( ${#line} + 1 + ${#word} )) -le $max_chars ]; then
      line="$line $word"
    else
      if [ -n "$result" ]; then
        result="$result\n$line"
      else
        result="$line"
      fi
      line="$word"
    fi
  done
  if [ -n "$line" ]; then
    if [ -n "$result" ]; then
      result="$result\n$line"
    else
      result="$line"
    fi
  fi
  echo -e "$result"
}

COUNT=0

# Process every .md file under docs/
while IFS= read -r -d '' file; do
  rel="${file#$DOCS_DIR/}"
  # Derive slug: index.md -> parent dir name, otherwise filename without .md
  slug="${rel%.md}"
  if [[ "$slug" == */index ]]; then
    slug="${slug%/index}"
  fi
  # Flatten path for filename: reference/cli -> reference-cli
  safe_name="${slug//\//-}"
  if [ -z "$safe_name" ] || [ "$safe_name" = "index" ]; then
    safe_name="index"
  fi

  title=$(get_title "$file")
  if [ -z "$title" ]; then
    echo "SKIP $rel (no title found)"
    continue
  fi

  # Determine section subtitle
  case "$rel" in
    reference/*) subtitle="Reference" ;;
    repertoire/*) subtitle="Repertoire" ;;
    *)            subtitle="Documentation" ;;
  esac

  wrapped=$(wrap_title "$title")
  output="$OUT_DIR/${safe_name}.png"

  generate_image "$wrapped" "$subtitle" "$output"
  COUNT=$((COUNT + 1))
  echo "OK   $safe_name.png  <- \"$title\""

done < <(find "$DOCS_DIR" -name '*.md' -print0 | sort -z)

echo ""
echo "Generated $COUNT OG images in static/img/og/"
