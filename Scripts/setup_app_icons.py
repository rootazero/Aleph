#!/usr/bin/env python3
"""
Setup Aleph app icons in Assets.xcassets
- Menu bar icon (template mode)
- App icon reference
"""

import os
import json
import shutil

# Paths
RESOURCES_DIR = os.path.expanduser("~/Workspace/Aleph/Aleph/Resources/AppIcon")
ASSETS_DIR = os.path.expanduser("~/Workspace/Aleph/Aleph/Assets.xcassets")

def setup_menubar_icon():
    """Add menu bar icon to Assets.xcassets as template image"""
    icon_name = "MenuBarIcon"
    imageset_dir = os.path.join(ASSETS_DIR, f"{icon_name}.imageset")

    # Create imageset directory
    os.makedirs(imageset_dir, exist_ok=True)

    # Copy SVG file
    src_svg = os.path.join(RESOURCES_DIR, "AlephMenuBar.svg")
    dst_svg = os.path.join(imageset_dir, "AlephMenuBar.svg")
    shutil.copy2(src_svg, dst_svg)

    # Create Contents.json for template rendering
    contents = {
        "images": [
            {
                "filename": "AlephMenuBar.svg",
                "idiom": "universal"
            }
        ],
        "info": {
            "author": "xcode",
            "version": 1
        },
        "properties": {
            "preserves-vector-representation": True,
            "template-rendering-intent": "template"
        }
    }

    contents_path = os.path.join(imageset_dir, "Contents.json")
    with open(contents_path, 'w', encoding='utf-8') as f:
        json.dump(contents, f, indent=2)

    print(f"✅ Menu bar icon added: {icon_name}")
    print(f"   Usage: Image(\"MenuBarIcon\").renderingMode(.template)")


def setup_app_logo():
    """Add main app logo to Assets.xcassets"""
    icon_name = "AppLogo"
    imageset_dir = os.path.join(ASSETS_DIR, f"{icon_name}.imageset")

    # Create imageset directory
    os.makedirs(imageset_dir, exist_ok=True)

    # Copy SVG file
    src_svg = os.path.join(RESOURCES_DIR, "AlephLogo.svg")
    dst_svg = os.path.join(imageset_dir, "AlephLogo.svg")
    shutil.copy2(src_svg, dst_svg)

    # Create Contents.json
    contents = {
        "images": [
            {
                "filename": "AlephLogo.svg",
                "idiom": "universal"
            }
        ],
        "info": {
            "author": "xcode",
            "version": 1
        },
        "properties": {
            "preserves-vector-representation": True,
            "template-rendering-intent": "original"
        }
    }

    contents_path = os.path.join(imageset_dir, "Contents.json")
    with open(contents_path, 'w', encoding='utf-8') as f:
        json.dump(contents, f, indent=2)

    print(f"✅ App logo added: {icon_name}")
    print(f"   Usage: Image(\"AppLogo\")")


def create_usage_guide():
    """Create SwiftUI usage example file"""
    usage_code = '''// Aleph Icon Usage Examples

import SwiftUI

// MARK: - Menu Bar Icon (Template Mode)
// Use in menu bar, always renders as monochrome
struct MenuBarExample: View {
    var body: some View {
        Image("MenuBarIcon")
            .renderingMode(.template)
            .foregroundColor(.primary)
    }
}

// MARK: - App Logo (Full Color)
// Use in settings, about screen, etc.
struct AppLogoExample: View {
    var body: some View {
        Image("AppLogo")
            .resizable()
            .aspectRatio(contentMode: .fit)
            .frame(width: 64, height: 64)
    }
}

// MARK: - Different Sizes
struct IconSizesExample: View {
    var body: some View {
        VStack(spacing: 20) {
            // Small (Menu bar)
            Image("MenuBarIcon")
                .renderingMode(.template)
                .frame(width: 16, height: 16)

            // Medium (Settings)
            Image("AppLogo")
                .frame(width: 32, height: 32)

            // Large (About screen)
            Image("AppLogo")
                .frame(width: 128, height: 128)
        }
    }
}
'''

    output_path = os.path.join(RESOURCES_DIR, "IconUsageExamples.swift")
    with open(output_path, 'w', encoding='utf-8') as f:
        f.write(usage_code)

    print(f"✅ Created usage examples: IconUsageExamples.swift")


def main():
    print("🎨 Setting up Aleph icons in Assets.xcassets...\n")

    setup_menubar_icon()
    setup_app_logo()
    create_usage_guide()

    print(f"\n✨ Setup complete!")
    print(f"\n📝 Next steps:")
    print(f"   1. In Xcode, select Assets.xcassets")
    print(f"   2. Find AppIcon imageset")
    print(f"   3. Drag AppIcon.icns to the icon slots")
    print(f"   4. Or configure in project.yml:")
    print(f"      ASSETCATALOG_COMPILER_APPICON_NAME: AppIcon")
    print(f"\n💡 Icons added:")
    print(f"   - MenuBarIcon (template, for menu bar)")
    print(f"   - AppLogo (full color, for UI)")
    print(f"\n📚 See IconUsageExamples.swift for usage code")


if __name__ == "__main__":
    main()
