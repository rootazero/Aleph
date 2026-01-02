#!/usr/bin/env python3
"""
Setup provider icons in Assets.xcassets
Creates imageset directories with proper Contents.json for SVG vector assets
"""

import os
import json
import shutil

PROVIDERS = [
    "OpenAI",
    "Claude",
    "Gemini",
    "Ollama",
    "DeepSeek",
    "Moonshot",
    "OpenRouter",
    "Azure",
    "Github",
]

SVG_DIR = os.path.expanduser("~/Workspace/Aether/Aether/Resources/ProviderIcons")
ASSETS_DIR = os.path.expanduser("~/Workspace/Aether/Aether/Assets.xcassets")


def create_imageset_contents(svg_filename):
    """Create Contents.json for an imageset with SVG vector support"""
    return {
        "images": [
            {
                "filename": svg_filename,
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


def setup_provider_icon(provider_name):
    """Setup a single provider icon in Assets.xcassets"""
    svg_source = os.path.join(SVG_DIR, f"{provider_name}.svg")
    imageset_name = f"ProviderIcon-{provider_name}.imageset"
    imageset_dir = os.path.join(ASSETS_DIR, imageset_name)

    # Create imageset directory
    os.makedirs(imageset_dir, exist_ok=True)

    # Copy SVG file
    svg_dest = os.path.join(imageset_dir, f"{provider_name}.svg")
    shutil.copy2(svg_source, svg_dest)

    # Create Contents.json
    contents = create_imageset_contents(f"{provider_name}.svg")
    contents_path = os.path.join(imageset_dir, "Contents.json")
    with open(contents_path, 'w', encoding='utf-8') as f:
        json.dump(contents, f, indent=2)
        f.write('\n')  # Add trailing newline

    return imageset_dir


def main():
    """Main setup process"""
    if not os.path.exists(ASSETS_DIR):
        print(f"❌ Assets.xcassets not found at {ASSETS_DIR}")
        return

    if not os.path.exists(SVG_DIR):
        print(f"❌ SVG directory not found at {SVG_DIR}")
        return

    print(f"📦 Setting up provider icons in Assets.xcassets")
    print(f"📁 Assets: {ASSETS_DIR}")
    print(f"📁 SVGs: {SVG_DIR}\n")

    success_count = 0
    for provider in PROVIDERS:
        try:
            imageset_dir = setup_provider_icon(provider)
            print(f"✅ {provider}: Created imageset")
            success_count += 1
        except Exception as e:
            print(f"❌ {provider}: Error - {str(e)}")

    print(f"\n✨ Setup complete!")
    print(f"📦 {success_count}/{len(PROVIDERS)} provider icons added to Assets.xcassets")
    print(f"\n💡 Usage in SwiftUI: Image(\"ProviderIcon-OpenAI\")")


if __name__ == "__main__":
    main()
