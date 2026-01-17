#!/bin/bash
#
# Create Aether.dmg with custom background for distribution
# Usage: ./Scripts/create-dmg.sh
#
# Requirements:
#   - Python with Pillow: cd ~/.python3 && uv pip install Pillow
#   - dmgbuild: cd ~/.python3 && uv pip install dmgbuild
#

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Configuration
APP_NAME="Aether"
VOLUME_NAME="${APP_NAME}"
DMG_FINAL="${PROJECT_DIR}/${APP_NAME}.dmg"
BACKGROUND_DIR="/tmp/dmg-backgrounds"
SETTINGS_FILE="/tmp/dmg_settings.py"

# Window dimensions
WINDOW_WIDTH=540
WINDOW_HEIGHT=360

# Icon positions
APP_ICON_X=130
APP_ICON_Y=200
APPS_ICON_X=410
APPS_ICON_Y=200

echo "=== Aether DMG Creator ==="
echo ""

# Find built app
find_app() {
    local app_path

    # Try Release first
    app_path=$(find ~/Library/Developer/Xcode/DerivedData -name "Aether.app" -path "*/Release/*" 2>/dev/null | head -1)

    # Fall back to Debug
    if [ -z "$app_path" ]; then
        app_path=$(find ~/Library/Developer/Xcode/DerivedData -name "Aether.app" -path "*/Debug/*" 2>/dev/null | head -1)
    fi

    # Check project build directory
    if [ -z "$app_path" ] && [ -d "${PROJECT_DIR}/build/Release/Aether.app" ]; then
        app_path="${PROJECT_DIR}/build/Release/Aether.app"
    fi

    echo "$app_path"
}

# Generate background image
generate_background() {
    echo "Generating background image..."

    mkdir -p "$BACKGROUND_DIR"

    ~/.python3/bin/python << PYTHON_SCRIPT
from PIL import Image, ImageDraw, ImageFilter

WIDTH = ${WINDOW_WIDTH}
HEIGHT = ${WINDOW_HEIGHT}

# Dark theme colors
BG_START = (12, 18, 30)
BG_END = (22, 30, 48)

img = Image.new('RGBA', (WIDTH, HEIGHT), BG_START)
draw = ImageDraw.Draw(img)

# Gradient background
for y in range(HEIGHT):
    ratio = y / HEIGHT
    r = int(BG_START[0] + (BG_END[0] - BG_START[0]) * ratio)
    g = int(BG_START[1] + (BG_END[1] - BG_START[1]) * ratio)
    b = int(BG_START[2] + (BG_END[2] - BG_START[2]) * ratio)
    draw.line([(0, y), (WIDTH, y)], fill=(r, g, b, 255))

# Load and place logo
icon_path = '${PROJECT_DIR}/Aether/Assets.xcassets/AppIcon.appiconset/icon_512x512.png'
try:
    logo = Image.open(icon_path).convert('RGBA')
    logo = logo.resize((64, 64), Image.Resampling.LANCZOS)

    logo_x = (WIDTH - 64) // 2
    logo_y = 25

    # Glow effect
    glow = Image.new('RGBA', (114, 114), (0, 0, 0, 0))
    glow_draw = ImageDraw.Draw(glow)
    for i in range(25, 0, -2):
        alpha = int(12 * (25 - i) / 25)
        glow_draw.ellipse([25-i, 25-i, 89+i, 89+i], fill=(10, 132, 255, alpha))
    glow = glow.filter(ImageFilter.GaussianBlur(10))

    img.paste(glow, (logo_x - 25, logo_y - 25), glow)
    img.paste(logo, (logo_x, logo_y), logo)
except Exception as e:
    print(f"Warning: Could not load logo: {e}")

# Draw arrow in center
arrow_y = ${APP_ICON_Y}

# Arrow shaft with gradient
for i, x in enumerate(range(200, 340, 3)):
    progress = (x - 200) / 140
    alpha = int(80 + 100 * progress)
    draw.ellipse([x, arrow_y - 3, x + 8, arrow_y + 3], fill=(10, 132, 255, alpha))

# Arrow head
draw.polygon([
    (340, arrow_y - 18),
    (368, arrow_y),
    (340, arrow_y + 18)
], fill=(10, 132, 255, 200))

# Ghost arrows for visual depth
for offset in [-35, 35]:
    gy = arrow_y + offset
    for x in range(220, 320, 6):
        draw.ellipse([x, gy - 1, x + 4, gy + 1], fill=(10, 132, 255, 40))
    draw.polygon([(320, gy - 8), (332, gy), (320, gy + 8)], fill=(10, 132, 255, 40))

img.save('${BACKGROUND_DIR}/background.png', 'PNG')
print("Background image created")
PYTHON_SCRIPT
}

# Create DMG using dmgbuild
create_dmg() {
    local app_path="$1"

    echo "Creating DMG from: $app_path"

    # Eject any existing volume
    hdiutil detach "/Volumes/${VOLUME_NAME}" 2>/dev/null || true

    # Remove old DMG
    rm -f "$DMG_FINAL"

    # Create settings file for dmgbuild
    cat > "$SETTINGS_FILE" << SETTINGS
# DMG build settings for Aether

volume_name = '${VOLUME_NAME}'
format = 'UDZO'
compression_level = 9
size = None

files = [
    ('${app_path}', '${APP_NAME}.app'),
]

symlinks = {
    'Applications': '/Applications',
}

background = '${BACKGROUND_DIR}/background.png'

window_rect = ((200, 120), (${WINDOW_WIDTH}, ${WINDOW_HEIGHT}))
icon_size = 96
icon_locations = {
    '${APP_NAME}.app': (${APP_ICON_X}, ${APP_ICON_Y}),
    'Applications': (${APPS_ICON_X}, ${APPS_ICON_Y}),
}

default_view = 'icon-view'
show_icon_preview = False
text_size = 14
SETTINGS

    echo "Building DMG with dmgbuild..."
    ~/.python3/bin/dmgbuild -s "$SETTINGS_FILE" "${VOLUME_NAME}" "$DMG_FINAL"

    # Cleanup
    rm -f "$SETTINGS_FILE"
    rm -rf "$BACKGROUND_DIR"
}

# Main
main() {
    # Find app
    local app_path=$(find_app)

    if [ -z "$app_path" ] || [ ! -d "$app_path" ]; then
        echo "Error: Aether.app not found. Please build the project first:"
        echo "  xcodegen generate && xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Release build"
        exit 1
    fi

    echo "Found app: $app_path"
    echo ""

    # Check dependencies
    if ! ~/.python3/bin/python -c "from PIL import Image" 2>/dev/null; then
        echo "Error: Python Pillow not found. Install with:"
        echo "  cd ~/.python3 && uv pip install Pillow"
        exit 1
    fi

    if ! command -v ~/.python3/bin/dmgbuild &>/dev/null; then
        echo "Error: dmgbuild not found. Install with:"
        echo "  cd ~/.python3 && uv pip install dmgbuild"
        exit 1
    fi

    # Generate background
    generate_background

    # Create DMG
    create_dmg "$app_path"

    echo ""
    echo "=== Done ==="
    echo "DMG created: $DMG_FINAL"
    ls -lh "$DMG_FINAL"
}

main "$@"
