#!/bin/bash
# Update Aleph app icon with the new enlarged satellite star design
# Requires: rsvg-convert (install via: brew install librsvg)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR/.."
SVG_SOURCE="$PROJECT_DIR/Aleph/Assets.xcassets/AppIcon-Source.imageset/AlephIcon.svg"
APPICONSET_DIR="$PROJECT_DIR/Aleph/Assets.xcassets/AppIcon.appiconset"

# Check if source SVG exists
if [ ! -f "$SVG_SOURCE" ]; then
    echo "❌ Source SVG not found at: $SVG_SOURCE"
    exit 1
fi

# Check if rsvg-convert is installed
if ! command -v rsvg-convert &> /dev/null; then
    echo "⚠️  rsvg-convert not found. Installing via Homebrew..."
    brew install librsvg
fi

echo "🎨 Generating PNG icons from SVG..."
echo "📂 Source: $SVG_SOURCE"
echo "📁 Output: $APPICONSET_DIR"
echo ""

# Generate all required icon sizes for macOS
# Format: size filename_suffix
declare -a icons=(
    "16 icon_16x16.png"
    "32 icon_16x16@2x.png"
    "32 icon_32x32.png"
    "64 icon_32x32@2x.png"
    "128 icon_128x128.png"
    "256 icon_128x128@2x.png"
    "256 icon_256x256.png"
    "512 icon_256x256@2x.png"
    "512 icon_512x512.png"
    "1024 icon_512x512@2x.png"
)

for icon_spec in "${icons[@]}"; do
    read -r size filename <<< "$icon_spec"
    output_path="$APPICONSET_DIR/$filename"

    rsvg-convert -w $size -h $size "$SVG_SOURCE" -o "$output_path"
    echo "  ✅ Generated: $filename (${size}x${size})"
done

echo ""
echo "✨ App icon successfully updated!"
echo "🔄 Please rebuild the app to see the changes"
