#!/usr/bin/env python3
"""
Extract color SVG icons from lobe-icons React components.
Converts React/TSX icon components to standalone SVG files.
"""

import os
import re
import sys
from pathlib import Path
from typing import Optional

# Icon mappings: (provider_name, lobe-icons_folder_name)
ICON_MAPPINGS = [
    ("OpenAI", "OpenAI"),
    ("Claude", "Claude"),
    ("Gemini", "Gemini"),
    ("Ollama", "Ollama"),
    ("DeepSeek", "DeepSeek"),
    ("Moonshot", "Moonshot"),
    ("OpenRouter", "OpenRouter"),
    ("Azure", "Azure"),
    ("Github", "Github"),
]

def extract_svg_from_tsx(tsx_path: Path) -> Optional[str]:
    """
    Extract SVG content from TSX Color component.
    Converts React JSX to standard SVG.
    """
    try:
        content = tsx_path.read_text(encoding='utf-8')

        # Find the return statement with SVG
        svg_match = re.search(r'return\s*\(\s*<svg([^>]*)>(.*?)</svg>', content, re.DOTALL)
        if not svg_match:
            print(f"  ⚠️  No SVG found in {tsx_path.name}")
            return None

        svg_attrs = svg_match.group(1)
        svg_body = svg_match.group(2)

        # Extract viewBox from attributes
        viewbox_match = re.search(r'viewBox="([^"]+)"', svg_attrs)
        viewbox = viewbox_match.group(1) if viewbox_match else "0 0 24 24"

        # Clean up JSX syntax
        # Replace dynamic fill IDs with static ones (need to handle multiple gradients)
        # First, replace gradient IDs in defs
        gradient_counter = [0]  # Use list to allow modification in nested function
        def replace_gradient_id(match):
            gradient_counter[0] += 1
            return f'id="gradient{gradient_counter[0]}"'
        
        # Replace gradient IDs in <linearGradient> tags
        svg_body = re.sub(r'id=\{[a-z]\.id\}', replace_gradient_id, svg_body)
        
        # Replace fill references to gradients
        gradient_counter[0] = 0  # Reset counter
        def replace_fill_ref(match):
            gradient_counter[0] += 1
            return f'fill="url(#gradient{gradient_counter[0]})"'
        
        svg_body = re.sub(r'fill=\{[a-z]\.fill\}', replace_fill_ref, svg_body)

        # Convert JSX attributes to HTML attributes
        svg_body = re.sub(r'stopColor=', 'stop-color=', svg_body)
        svg_body = re.sub(r'stopOpacity=', 'stop-opacity=', svg_body)
        svg_body = re.sub(r'gradientUnits=', 'gradientUnits=', svg_body)
        
        # Replace {TITLE} placeholder
        svg_body = re.sub(r'\{TITLE\}', tsx_path.parent.parent.name, svg_body)

        # Remove React-specific attributes
        svg_body = re.sub(r'\s*\{\.\.\.rest\}', '', svg_body)

        # Build final SVG
        svg = f'''<?xml version="1.0" encoding="UTF-8"?>
<svg width="24" height="24" viewBox="{viewbox}" fill="none" xmlns="http://www.w3.org/2000/svg">
{svg_body.strip()}
</svg>'''

        return svg

    except Exception as e:
        print(f"  ❌ Error processing {tsx_path.name}: {e}")
        return None


def extract_all_icons(lobe_icons_path: Path, output_dir: Path):
    """Extract all provider icons from lobe-icons to output directory."""

    if not lobe_icons_path.exists():
        print(f"❌ lobe-icons path not found: {lobe_icons_path}")
        sys.exit(1)

    output_dir.mkdir(parents=True, exist_ok=True)
    print(f"📂 Extracting icons from: {lobe_icons_path}")
    print(f"📁 Output directory: {output_dir}\n")

    success_count = 0

    for provider_name, folder_name in ICON_MAPPINGS:
        print(f"🔍 Processing {provider_name}...")

        # Find Color.tsx component
        color_tsx = lobe_icons_path / folder_name / "components" / "Color.tsx"

        if not color_tsx.exists():
            print(f"  ⚠️  Color.tsx not found at {color_tsx}")
            continue

        # Extract SVG
        svg_content = extract_svg_from_tsx(color_tsx)

        if svg_content:
            # Write to output
            output_file = output_dir / f"{provider_name}.svg"
            output_file.write_text(svg_content, encoding='utf-8')
            print(f"  ✅ Extracted to {output_file.name}")
            success_count += 1

    print(f"\n✨ Successfully extracted {success_count}/{len(ICON_MAPPINGS)} icons")


def main():
    # Paths
    lobe_icons_path = Path(__file__).parent.parent / "icons"
    output_dir = Path(__file__).parent.parent / "Aleph" / "Assets.xcassets" / "_extracted_icons"

    extract_all_icons(lobe_icons_path, output_dir)


if __name__ == "__main__":
    main()
