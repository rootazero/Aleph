#!/bin/bash
#
# Create Aether.dmg with custom background for distribution
# Usage: ./Scripts/create-dmg.sh
#

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Configuration
APP_NAME="Aether"
DMG_NAME="${APP_NAME}"
VOLUME_NAME="${APP_NAME}"
DMG_TEMP="/tmp/${DMG_NAME}_temp.dmg"
DMG_FINAL="${PROJECT_DIR}/${DMG_NAME}.dmg"
STAGING_DIR="/tmp/dmg-staging"
BACKGROUND_DIR="/tmp/dmg-backgrounds"

# Window dimensions
WINDOW_WIDTH=540
WINDOW_HEIGHT=360

# Icon positions (left app, right Applications folder)
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

# Generate background image using Python
generate_background() {
    echo "Generating background images..."

    mkdir -p "$BACKGROUND_DIR"

    ~/.python3/bin/python << PYTHON_SCRIPT
from PIL import Image, ImageDraw, ImageFilter

WIDTH = ${WINDOW_WIDTH}
HEIGHT = ${WINDOW_HEIGHT}

# Dark theme colors
BG_COLOR_START = (12, 18, 30)
BG_COLOR_END = (22, 30, 48)

# Create background
img = Image.new('RGBA', (WIDTH, HEIGHT), BG_COLOR_START)
draw = ImageDraw.Draw(img)

# Vertical gradient
for y in range(HEIGHT):
    ratio = y / HEIGHT
    r = int(BG_COLOR_START[0] + (BG_COLOR_END[0] - BG_COLOR_START[0]) * ratio)
    g = int(BG_COLOR_START[1] + (BG_COLOR_END[1] - BG_COLOR_START[1]) * ratio)
    b = int(BG_COLOR_START[2] + (BG_COLOR_END[2] - BG_COLOR_START[2]) * ratio)
    draw.line([(0, y), (WIDTH, y)], fill=(r, g, b, 255))

# Load logo
icon_path = '${PROJECT_DIR}/Aether/Assets.xcassets/AppIcon.appiconset/icon_512x512.png'
try:
    logo = Image.open(icon_path).convert('RGBA')
    logo_size = 64
    logo = logo.resize((logo_size, logo_size), Image.Resampling.LANCZOS)

    # Position logo at top center
    logo_x = (WIDTH - logo_size) // 2
    logo_y = 25

    # Glow effect
    glow_size = logo_size + 50
    glow = Image.new('RGBA', (glow_size, glow_size), (0, 0, 0, 0))
    glow_draw = ImageDraw.Draw(glow)
    for i in range(25, 0, -2):
        alpha = int(12 * (25 - i) / 25)
        offset = (glow_size - logo_size) // 2
        glow_draw.ellipse([offset-i, offset-i, offset+logo_size+i, offset+logo_size+i],
                          fill=(10, 132, 255, alpha))
    glow = glow.filter(ImageFilter.GaussianBlur(10))

    glow_x = logo_x - (glow_size - logo_size) // 2
    glow_y = logo_y - (glow_size - logo_size) // 2
    img.paste(glow, (glow_x, glow_y), glow)
    img.paste(logo, (logo_x, logo_y), logo)
except Exception as e:
    print(f"Warning: Could not load logo: {e}")

# Draw arrow in center
arrow_center_y = ${APP_ICON_Y}
arrow_color = (10, 132, 255, 180)

shaft_start_x = 200
shaft_end_x = 340

for i, x in enumerate(range(shaft_start_x, shaft_end_x, 3)):
    progress = (x - shaft_start_x) / (shaft_end_x - shaft_start_x)
    alpha = int(80 + 100 * progress)
    draw.ellipse([x, arrow_center_y - 3, x + 8, arrow_center_y + 3], fill=(10, 132, 255, alpha))

# Arrow head
head_x = shaft_end_x
draw.polygon([
    (head_x, arrow_center_y - 18),
    (head_x + 28, arrow_center_y),
    (head_x, arrow_center_y + 18)
], fill=(10, 132, 255, 200))

# Ghost arrows
for offset in [-35, 35]:
    ghost_y = arrow_center_y + offset
    for x in range(220, 320, 6):
        draw.ellipse([x, ghost_y - 1, x + 4, ghost_y + 1], fill=(10, 132, 255, 40))
    draw.polygon([
        (320, ghost_y - 8),
        (332, ghost_y),
        (320, ghost_y + 8)
    ], fill=(10, 132, 255, 40))

# Save 1x
img.save('${BACKGROUND_DIR}/background.png', 'PNG')

# Create @2x version
img_2x = Image.new('RGBA', (WIDTH * 2, HEIGHT * 2), BG_COLOR_START)
draw_2x = ImageDraw.Draw(img_2x)

for y in range(HEIGHT * 2):
    ratio = y / (HEIGHT * 2)
    r = int(BG_COLOR_START[0] + (BG_COLOR_END[0] - BG_COLOR_START[0]) * ratio)
    g = int(BG_COLOR_START[1] + (BG_COLOR_END[1] - BG_COLOR_START[1]) * ratio)
    b = int(BG_COLOR_START[2] + (BG_COLOR_END[2] - BG_COLOR_START[2]) * ratio)
    draw_2x.line([(0, y), (WIDTH * 2, y)], fill=(r, g, b, 255))

# 2x logo
try:
    logo_2x = Image.open(icon_path).convert('RGBA')
    logo_size_2x = 128
    logo_2x = logo_2x.resize((logo_size_2x, logo_size_2x), Image.Resampling.LANCZOS)
    logo_x_2x = (WIDTH * 2 - logo_size_2x) // 2
    logo_y_2x = 50

    glow_size_2x = logo_size_2x + 100
    glow_2x = Image.new('RGBA', (glow_size_2x, glow_size_2x), (0, 0, 0, 0))
    glow_draw_2x = ImageDraw.Draw(glow_2x)
    for i in range(50, 0, -4):
        alpha = int(12 * (50 - i) / 50)
        offset = (glow_size_2x - logo_size_2x) // 2
        glow_draw_2x.ellipse([offset-i, offset-i, offset+logo_size_2x+i, offset+logo_size_2x+i],
                             fill=(10, 132, 255, alpha))
    glow_2x = glow_2x.filter(ImageFilter.GaussianBlur(20))

    glow_x_2x = logo_x_2x - (glow_size_2x - logo_size_2x) // 2
    glow_y_2x = logo_y_2x - (glow_size_2x - logo_size_2x) // 2
    img_2x.paste(glow_2x, (glow_x_2x, glow_y_2x), glow_2x)
    img_2x.paste(logo_2x, (logo_x_2x, logo_y_2x), logo_2x)
except Exception as e:
    print(f"Warning: Could not load logo for 2x: {e}")

# 2x arrow
arrow_center_y_2x = ${APP_ICON_Y} * 2
shaft_start_x_2x = 400
shaft_end_x_2x = 680

for i, x in enumerate(range(shaft_start_x_2x, shaft_end_x_2x, 6)):
    progress = (x - shaft_start_x_2x) / (shaft_end_x_2x - shaft_start_x_2x)
    alpha = int(80 + 100 * progress)
    draw_2x.ellipse([x, arrow_center_y_2x - 6, x + 16, arrow_center_y_2x + 6], fill=(10, 132, 255, alpha))

head_x_2x = shaft_end_x_2x
draw_2x.polygon([
    (head_x_2x, arrow_center_y_2x - 36),
    (head_x_2x + 56, arrow_center_y_2x),
    (head_x_2x, arrow_center_y_2x + 36)
], fill=(10, 132, 255, 200))

for offset in [-70, 70]:
    ghost_y = arrow_center_y_2x + offset
    for x in range(440, 640, 12):
        draw_2x.ellipse([x, ghost_y - 2, x + 8, ghost_y + 2], fill=(10, 132, 255, 40))
    draw_2x.polygon([
        (640, ghost_y - 16),
        (664, ghost_y),
        (640, ghost_y + 16)
    ], fill=(10, 132, 255, 40))

img_2x.save('${BACKGROUND_DIR}/background@2x.png', 'PNG')

print("Background images generated")
PYTHON_SCRIPT
}

# Create DMG
create_dmg() {
    local app_path="$1"

    echo "Creating DMG from: $app_path"

    # Clean up
    rm -rf "$STAGING_DIR"
    rm -f "$DMG_TEMP" "$DMG_FINAL"
    mkdir -p "$STAGING_DIR"

    # Copy app
    cp -R "$app_path" "$STAGING_DIR/"

    # Create Applications symlink
    ln -s /Applications "$STAGING_DIR/Applications"

    # Copy background
    mkdir -p "$STAGING_DIR/.background"
    cp "$BACKGROUND_DIR/background.png" "$STAGING_DIR/.background/"
    cp "$BACKGROUND_DIR/background@2x.png" "$STAGING_DIR/.background/"

    # Calculate size
    local size=$(du -sm "$STAGING_DIR" | cut -f1)
    size=$((size + 20))

    echo "Creating writable DMG (${size}MB)..."

    # Create writable DMG
    hdiutil create -srcfolder "$STAGING_DIR" -volname "$VOLUME_NAME" -fs HFS+ \
        -fsargs "-c c=64,a=16,e=16" -format UDRW -size ${size}m "$DMG_TEMP"

    # Mount
    local device=$(hdiutil attach -readwrite -noverify -noautoopen "$DMG_TEMP" | egrep '^/dev/' | sed 1q | awk '{print $1}')

    sleep 2

    echo "Setting Finder window properties..."

    # Set Finder properties
    osascript << APPLESCRIPT
tell application "Finder"
    tell disk "${VOLUME_NAME}"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set bounds of container window to {200, 150, $((200 + WINDOW_WIDTH)), $((150 + WINDOW_HEIGHT))}

        set theViewOptions to the icon view options of container window
        set arrangement of theViewOptions to not arranged
        set icon size of theViewOptions to 96
        set background picture of theViewOptions to file ".background:background.png"

        set position of item "${APP_NAME}.app" of container window to {${APP_ICON_X}, ${APP_ICON_Y}}
        set position of item "Applications" of container window to {${APPS_ICON_X}, ${APPS_ICON_Y}}

        close
        open
        update without registering applications
        delay 2
        close
    end tell
end tell
APPLESCRIPT

    sync
    hdiutil detach "$device"

    echo "Converting to compressed DMG..."
    hdiutil convert "$DMG_TEMP" -format UDZO -imagekey zlib-level=9 -o "$DMG_FINAL"

    # Clean up
    rm -f "$DMG_TEMP"
    rm -rf "$STAGING_DIR"
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

    # Check Python and Pillow
    if ! ~/.python3/bin/python -c "from PIL import Image" 2>/dev/null; then
        echo "Error: Python Pillow not found. Install with:"
        echo "  cd ~/.python3 && uv pip install Pillow"
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
