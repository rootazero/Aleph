#!/usr/bin/env python3
"""
Extract SVG icons from lobe-icons and create standard SVG files
for use in Xcode Assets.xcassets
"""

import os
import re
import json

# Provider configurations
PROVIDERS = {
    "OpenAI": {
        "source": "OpenAI",
        "component": "Mono.tsx",
        "color": "#10a37f",
    },
    "Claude": {
        "source": "Claude",
        "component": "Color.tsx",
        "color": "#D97757",
    },
    "Gemini": {
        "source": "Gemini",
        "component": "Color.tsx",
        "color": "#3186FF",
    },
    "Ollama": {
        "source": "Ollama",
        "component": "Mono.tsx",
        "color": "#000000",
    },
    "DeepSeek": {
        "source": "DeepSeek",
        "component": "Color.tsx",
        "color": "#4D6BFE",
    },
    "Moonshot": {
        "source": "Moonshot",
        "component": "Mono.tsx",
        "color": "#ff6b6b",
    },
    "OpenRouter": {
        "source": "OpenRouter",
        "component": "Mono.tsx",
        "color": "#8b5cf6",
    },
    "Azure": {
        "source": "Azure",
        "component": "Color.tsx",
        "color": "#0078D4",
    },
    "Github": {
        "source": "Github",
        "component": "Mono.tsx",
        "color": "#24292e",
    },
}

LOBE_ICONS_PATH = os.path.expanduser("~/workspace/lobe-icons/src")
OUTPUT_DIR = os.path.expanduser("~/Workspace/Aether/Aether/Resources/ProviderIcons")


def extract_svg_from_tsx(tsx_path):
    """Extract SVG attributes and paths from TSX file"""
    with open(tsx_path, 'r', encoding='utf-8') as f:
        content = f.read()

    # Extract viewBox
    viewbox_match = re.search(r'viewBox="([^"]+)"', content)
    viewbox = viewbox_match.group(1) if viewbox_match else "0 0 24 24"

    # Extract all path elements
    paths = re.findall(r'<path[^>]*\/>', content, re.MULTILINE)

    # Also try multi-line path tags
    if not paths:
        paths = re.findall(r'<path[^>]*>.*?<\/path>', content, re.DOTALL)

    # Extract defs if any (for gradients)
    defs_match = re.search(r'<defs>(.*?)<\/defs>', content, re.DOTALL)
    defs = defs_match.group(0) if defs_match else None

    return {
        'viewbox': viewbox,
        'paths': paths,
        'defs': defs
    }


def create_svg_file(name, svg_data, color):
    """Create a standalone SVG file"""
    svg_content = f'''<?xml version="1.0" encoding="UTF-8"?>
<svg viewBox="{svg_data['viewbox']}" xmlns="http://www.w3.org/2000/svg">
  <title>{name}</title>
'''

    # Add defs if present
    if svg_data['defs']:
        svg_content += f"  {svg_data['defs']}\n"

    # Add paths
    for path in svg_data['paths']:
        # Clean up path formatting
        path_clean = path.strip()
        # Replace fill="currentColor" with actual color for monochrome icons
        if 'fill="currentColor"' in path_clean and not svg_data['defs']:
            path_clean = path_clean.replace('fill="currentColor"', f'fill="{color}"')
        svg_content += f"  {path_clean}\n"

    svg_content += "</svg>\n"

    return svg_content


def main():
    """Main extraction process"""
    # Create output directory
    os.makedirs(OUTPUT_DIR, exist_ok=True)

    print(f"🔍 Extracting icons from {LOBE_ICONS_PATH}")
    print(f"📁 Output directory: {OUTPUT_DIR}\n")

    for provider_name, config in PROVIDERS.items():
        source_dir = os.path.join(LOBE_ICONS_PATH, config['source'], 'components')
        tsx_file = os.path.join(source_dir, config['component'])

        if not os.path.exists(tsx_file):
            print(f"⚠️  {provider_name}: {tsx_file} not found")
            continue

        try:
            # Extract SVG data
            svg_data = extract_svg_from_tsx(tsx_file)

            # Create SVG file
            svg_content = create_svg_file(provider_name, svg_data, config['color'])

            # Save to file
            output_file = os.path.join(OUTPUT_DIR, f"{provider_name}.svg")
            with open(output_file, 'w', encoding='utf-8') as f:
                f.write(svg_content)

            print(f"✅ {provider_name}: Created {output_file}")
            print(f"   Paths: {len(svg_data['paths'])}, ViewBox: {svg_data['viewbox']}")

        except Exception as e:
            print(f"❌ {provider_name}: Error - {str(e)}")

    print(f"\n✨ Icon extraction complete!")
    print(f"📦 {len(PROVIDERS)} SVG files created in {OUTPUT_DIR}")


if __name__ == "__main__":
    main()
