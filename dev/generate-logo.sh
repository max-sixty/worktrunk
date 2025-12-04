#!/usr/bin/env bash
# Generate the Worktrunk logo from the JSON prompt
#
# Requirements:
#   - gemimg: uv tool install gemimg (or run with: uvx gemimg)
#   - rembg: uv tool install rembg[cli]
#   - imagemagick: brew install imagemagick
#
# Usage:
#   ./dev/generate-logo.sh
#
# Generates logo.png, logo@2x.png, and favicon.png directly into docs/static/.
# Re-run if you don't like the result.

set -euo pipefail

cd "$(dirname "$0")/.."

PROMPT_FILE="dev/logo-prompt.json"
STATIC_DIR="docs/static"
RAW_FILE=".tmp/logo-raw.png"
SIZE_1X=512
SIZE_2X=1024
SIZE_FAVICON=32

if [[ ! -f "$PROMPT_FILE" ]]; then
    echo "Error: $PROMPT_FILE not found"
    exit 1
fi

if ! command -v gemimg &> /dev/null; then
    echo "Error: gemimg not found. Install with: uv tool install gemimg"
    exit 1
fi

if ! command -v magick &> /dev/null; then
    echo "Error: imagemagick not found. Install with: brew install imagemagick"
    exit 1
fi

if ! command -v rembg &> /dev/null; then
    echo "Error: rembg not found. Install with: uv tool install rembg[cli]"
    exit 1
fi

mkdir -p .tmp

echo "Generating logo..."
gemimg "$(cat "$PROMPT_FILE")" \
    --model gemini-3-pro-image-preview \
    --aspect-ratio 1:1 \
    -o "$RAW_FILE"

echo "Removing background..."
rembg i "$RAW_FILE" "$RAW_FILE"

echo "Processing sizes..."

# 1x version (512px)
magick "$RAW_FILE" -resize "${SIZE_1X}x${SIZE_1X}" "$STATIC_DIR/logo.png"

# 2x version (1024px)
magick "$RAW_FILE" -resize "${SIZE_2X}x${SIZE_2X}" "$STATIC_DIR/logo@2x.png"

# Favicon (32px)
magick "$RAW_FILE" -resize "${SIZE_FAVICON}x${SIZE_FAVICON}" "$STATIC_DIR/favicon.png"

rm "$RAW_FILE"

echo "Done. Generated:"
ls -la "$STATIC_DIR"/logo.png "$STATIC_DIR"/logo@2x.png "$STATIC_DIR"/favicon.png
