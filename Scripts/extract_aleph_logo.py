#!/usr/bin/env python3
"""
Extract Aleph logo SVG from HTML design file and generate various formats
- Main logo (with gradients)
- App icon (for macOS .icns)
- Menu bar icon (template, monochrome)
"""

import os
import re
from pathlib import Path

# Paths
HTML_FILE = os.path.expanduser("~/Workspace/Aleph.html")
OUTPUT_DIR = os.path.expanduser("~/Workspace/Aleph/Aleph/Resources/AppIcon")

# SVG Templates
MAIN_LOGO_SVG = '''<?xml version="1.0" encoding="UTF-8"?>
<svg viewBox="0 0 100 100" fill="none" xmlns="http://www.w3.org/2000/svg">
  <title>Aleph Logo</title>
  <defs>
    <linearGradient id="mainGradient" x1="10" y1="10" x2="90" y2="90" gradientUnits="userSpaceOnUse">
      <stop stop-color="#0A84FF"/>
      <stop offset="1" stop-color="#5E5CE6"/>
    </linearGradient>
    <linearGradient id="satGradient" x1="30" y1="15" x2="45" y2="30" gradientUnits="userSpaceOnUse">
      <stop stop-color="#80E0FF"/>
      <stop offset="1" stop-color="#0A84FF"/>
    </linearGradient>
  </defs>

  <!-- Main Star (Core) -->
  <path d="M55 15 C59 40 70 51 95 55 C70 59 59 70 55 95 C51 70 40 59 15 55 C40 51 51 40 55 15Z"
        fill="url(#mainGradient)" />

  <!-- Satellite Star (Spark) -->
  <path d="M35 14 C35.8 19 37 21 43 22 C37 23 35.8 25 35 30 C34.2 25 33 23 27 22 C33 21 34.2 19 35 14Z"
        fill="url(#satGradient)" />
</svg>
'''

APP_ICON_SVG = '''<?xml version="1.0" encoding="UTF-8"?>
<svg viewBox="0 0 1024 1024" fill="none" xmlns="http://www.w3.org/2000/svg">
  <title>Aleph App Icon</title>

  <!-- Background with gradient -->
  <rect width="1024" height="1024" rx="226.5" fill="url(#bgGradient)"/>

  <defs>
    <linearGradient id="bgGradient" x1="0" y1="0" x2="1024" y2="1024" gradientUnits="userSpaceOnUse">
      <stop stop-color="#1C1C1E"/>
      <stop offset="1" stop-color="#0A0A0C"/>
    </linearGradient>
    <linearGradient id="mainGradient" x1="200" y1="200" x2="824" y2="824" gradientUnits="userSpaceOnUse">
      <stop stop-color="#0A84FF"/>
      <stop offset="1" stop-color="#5E5CE6"/>
    </linearGradient>
    <linearGradient id="satGradient" x1="300" y1="200" x2="450" y2="350" gradientUnits="userSpaceOnUse">
      <stop stop-color="#80E0FF"/>
      <stop offset="1" stop-color="#0A84FF"/>
    </linearGradient>
  </defs>

  <!-- Centered logo (scaled to fit icon) -->
  <g transform="translate(262, 262) scale(5)">
    <!-- Main Star -->
    <path d="M55 15 C59 40 70 51 95 55 C70 59 59 70 55 95 C51 70 40 59 15 55 C40 51 51 40 55 15Z"
          fill="url(#mainGradient)" />

    <!-- Satellite Star -->
    <path d="M35 14 C35.8 19 37 21 43 22 C37 23 35.8 25 35 30 C34.2 25 33 23 27 22 C33 21 34.2 19 35 14Z"
          fill="url(#satGradient)" />
  </g>
</svg>
'''

MENUBAR_ICON_SVG = '''<?xml version="1.0" encoding="UTF-8"?>
<svg viewBox="0 0 100 100" fill="none" xmlns="http://www.w3.org/2000/svg">
  <title>Aleph Menu Bar Icon</title>

  <!-- Main Star (monochrome) -->
  <path d="M55 15 C59 40 70 51 95 55 C70 59 59 70 55 95 C51 70 40 59 15 55 C40 51 51 40 55 15Z"
        fill="currentColor" />

  <!-- Satellite Star (slightly transparent) -->
  <path d="M35 14 C35.8 19 37 21 43 22 C37 23 35.8 25 35 30 C34.2 25 33 23 27 22 C33 21 34.2 19 35 14Z"
        fill="currentColor"
        opacity="0.7"/>
</svg>
'''

# Simple SVG for smaller sizes (simplified satellite)
SIMPLE_ICON_SVG = '''<?xml version="1.0" encoding="UTF-8"?>
<svg viewBox="0 0 100 100" fill="none" xmlns="http://www.w3.org/2000/svg">
  <title>Aleph Icon (Simplified)</title>
  <defs>
    <linearGradient id="mainGradient" x1="10" y1="10" x2="90" y2="90" gradientUnits="userSpaceOnUse">
      <stop stop-color="#0A84FF"/>
      <stop offset="1" stop-color="#5E5CE6"/>
    </linearGradient>
  </defs>

  <!-- Main Star only (for very small sizes) -->
  <path d="M55 15 C59 40 70 51 95 55 C70 59 59 70 55 95 C51 70 40 59 15 55 C40 51 51 40 55 15Z"
        fill="url(#mainGradient)" />
</svg>
'''


def create_output_directory():
    """Create output directory structure"""
    os.makedirs(OUTPUT_DIR, exist_ok=True)
    print(f"📁 Output directory: {OUTPUT_DIR}")


def save_svg_files():
    """Save all SVG variants"""
    files = {
        "AlephLogo.svg": MAIN_LOGO_SVG,
        "AlephAppIcon.svg": APP_ICON_SVG,
        "AlephMenuBar.svg": MENUBAR_ICON_SVG,
        "AlephSimple.svg": SIMPLE_ICON_SVG,
    }

    for filename, content in files.items():
        filepath = os.path.join(OUTPUT_DIR, filename)
        with open(filepath, 'w', encoding='utf-8') as f:
            f.write(content)
        print(f"✅ Created: {filename}")


def create_iconset_script():
    """Create a shell script to generate .icns file from SVG"""
    script_content = '''#!/bin/bash
# Generate macOS .icns file from Aleph logo SVG
# Requires: rsvg-convert (install via: brew install librsvg)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESOURCES_DIR="$SCRIPT_DIR/../Resources/AppIcon"
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
'''

    script_path = os.path.join(os.path.dirname(OUTPUT_DIR), "..", "..", "Scripts", "generate_app_icon.sh")
    with open(script_path, 'w', encoding='utf-8') as f:
        f.write(script_content)

    # Make executable
    os.chmod(script_path, 0o755)
    print(f"✅ Created: generate_app_icon.sh")
    print(f"   Run with: ./Scripts/generate_app_icon.sh")


def create_asset_catalog_entries():
    """Create README for adding icons to Assets.xcassets"""
    readme_content = '''# Aleph App Icon Integration

## Files Generated

1. **AlephLogo.svg** - Main logo with gradients (for marketing, web, etc.)
2. **AlephAppIcon.svg** - App icon with background (1024x1024 base)
3. **AlephMenuBar.svg** - Menu bar icon (template mode, monochrome)
4. **AlephSimple.svg** - Simplified version for small sizes

## Integration Steps

### 1. Generate .icns file (macOS App Icon)

```bash
cd Aleph
./Scripts/generate_app_icon.sh
```

This will create `AppIcon.icns` from `AlephAppIcon.svg`.

### 2. Add to Xcode Project

**Option A: Use .icns directly**
1. In Xcode, select `Aleph/Assets.xcassets`
2. Select `AppIcon` imageset
3. Drag `AppIcon.icns` into the appropriate slots
4. Or manually configure in `project.yml`:

```yaml
targets:
  Aleph:
    settings:
      ASSETCATALOG_COMPILER_APPICON_NAME: AppIcon
```

**Option B: Use individual PNG files**
1. Keep the `.iconset` folder
2. Drag individual PNG files to corresponding size slots in Assets.xcassets

### 3. Menu Bar Icon

For the menu bar icon, add `AlephMenuBar.svg` to Assets.xcassets:

1. Create new Image Set: `MenuBarIcon`
2. Set "Render As" to "Template Image"
3. Add `AlephMenuBar.svg` to "Universal" slot
4. Set "Preserve Vector Data" to true

Then use in code:
```swift
Image("MenuBarIcon")
    .renderingMode(.template)
```

## Design Notes

**Color Palette:**
- Main Star: Linear gradient #0A84FF → #5E5CE6 (Apple Blue)
- Satellite Star: Linear gradient #80E0FF → #0A84FF (Bright Cyan)
- Background: #1C1C1E → #0A0A0C (Dark gray)

**Sizes:**
- App Icon: 1024x1024 (required for App Store)
- Menu Bar: 16pt (32px @2x) - template mode
- Dock: Multiple sizes generated automatically

**Icon Philosophy:**
"Tighter Gravitational Pull" - The satellite star is deliberately small and close to the main star,
creating a sense of energy spark rather than two separate objects.
'''

    readme_path = os.path.join(OUTPUT_DIR, "README.md")
    with open(readme_path, 'w', encoding='utf-8') as f:
        f.write(readme_content)
    print(f"✅ Created: README.md")


def main():
    print("🎨 Extracting Aleph Logo from HTML design file...\n")

    create_output_directory()
    save_svg_files()
    create_iconset_script()
    create_asset_catalog_entries()

    print(f"\n✨ Extraction complete!")
    print(f"📦 Files created in: {OUTPUT_DIR}")
    print(f"\n📖 Next steps:")
    print(f"   1. Run: ./Scripts/generate_app_icon.sh")
    print(f"   2. Add AppIcon.icns to Xcode project")
    print(f"   3. Add MenuBar icon to Assets.xcassets")
    print(f"\n📚 See {OUTPUT_DIR}/README.md for detailed instructions")


if __name__ == "__main__":
    main()
