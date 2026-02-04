#!/bin/bash
# Generate macOS .icns file from Aleph logo SVG
# Requires: rsvg-convert (install via: brew install librsvg)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESOURCES_DIR="$SCRIPT_DIR/../Aleph/Resources/AppIcon"
ICONSET_DIR="$RESOURCES_DIR/AppIcon.iconset"
SVG_FILE="$RESOURCES_DIR/AlephAppIcon.svg"

# Check if rsvg-convert is installed
if ! command -v rsvg-convert &> /dev/null; then
    echo "❌ rsvg-convert not found. Installing via Homebrew..."
    brew install librsvg
fi

# Create iconset directory
mkdir -p "$ICONSET_DIR"

echo "🎨 Generating PNG icons from SVG..."

# Generate all required icon sizes for macOS
sizes=(16 32 64 128 256 512 1024)

for size in "${sizes[@]}"; do
    rsvg-convert -w $size -h $size "$SVG_FILE" -o "$ICONSET_DIR/icon_${size}x${size}.png"
    echo "  ✓ ${size}x${size}"

    # Create @2x versions (except for 1024)
    if [ $size -lt 1024 ]; then
        size2x=$((size * 2))
        rsvg-convert -w $size2x -h $size2x "$SVG_FILE" -o "$ICONSET_DIR/icon_${size}x${size}@2x.png"
        echo "  ✓ ${size}x${size}@2x (${size2x}x${size2x})"
    fi
done

echo "📦 Converting iconset to .icns..."
iconutil -c icns "$ICONSET_DIR" -o "$RESOURCES_DIR/AppIcon.icns"

echo "✨ Done! AppIcon.icns created at:"
echo "   $RESOURCES_DIR/AppIcon.icns"

# Optionally clean up iconset directory
# rm -rf "$ICONSET_DIR"
