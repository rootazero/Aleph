#!/usr/bin/env python3
"""
Extract SVG icons from lobe-icons and create standard SVG files
for use in Xcode Assets.xcassets

Fixed version that properly handles:
- React variable references ({a.id}, {b.fill}, etc.)
- Missing fill attributes (adds brand color)
- linearGradient IDs generation
"""

import os
import re
import json
from collections import defaultdict

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


def extract_svg_from_tsx(tsx_path, provider_name):
    """Extract SVG attributes and paths from TSX file"""
    with open(tsx_path, 'r', encoding='utf-8') as f:
        content = f.read()

    # Extract viewBox
    viewbox_match = re.search(r'viewBox="([^"]+)"', content)
    viewbox = viewbox_match.group(1) if viewbox_match else "0 0 24 24"

    # Extract all path elements (including multi-line)
    paths = re.findall(r'<path[^>]*?(?:/>|>.*?</path>)', content, re.DOTALL)

    # Extract defs if any (for gradients)
    defs_match = re.search(r'<defs>(.*?)</defs>', content, re.DOTALL)
    defs_content = defs_match.group(1) if defs_match else None

    return {
        'viewbox': viewbox,
        'paths': paths,
        'defs': defs_content
    }


def clean_react_syntax(content, provider_name, gradient_id_map=None):
    """Remove React/TSX syntax and replace with valid SVG"""
    # Generate unique gradient IDs (shared across calls if provided)
    if gradient_id_map is None:
        gradient_id_map = {}

    gradient_counter = len(gradient_id_map)  # Start from existing count

    # Replace {a.id}, {b.id}, {c.id} etc with actual IDs (with quotes)
    def replace_gradient_id(match):
        nonlocal gradient_counter
        var_name = match.group(1)
        if var_name not in gradient_id_map:
            gradient_counter += 1
            gradient_id_map[var_name] = f"{provider_name.lower()}-gradient-{gradient_counter}"
        return f'"{gradient_id_map[var_name]}"'  # Add quotes

    # Replace ID references in gradient definitions
    content = re.sub(r'\{(\w+)\.id\}', replace_gradient_id, content)

    # Replace fill references {a.fill}, {b.fill} etc with url(#gradient-id)
    def replace_gradient_fill(match):
        var_name = match.group(1)
        if var_name in gradient_id_map:
            return f'"url(#{gradient_id_map[var_name]})"'  # Add quotes
        return '"currentColor"'  # Fallback with quotes

    content = re.sub(r'\{(\w+)\.fill\}', replace_gradient_fill, content)

    # Replace any remaining fill=currentColor (without quotes) with quoted version
    content = re.sub(r'fill=currentColor(?!["\w])', 'fill="currentColor"', content)

    # Remove any remaining JSX props
    content = re.sub(r'\s+className="[^"]*"', '', content)
    content = re.sub(r'\s+\{\.\.\.props\}', '', content)

    return content, gradient_id_map


def create_svg_file(name, svg_data, color):
    """Create a standalone SVG file"""
    svg_content = f'''<?xml version="1.0" encoding="UTF-8"?>
<svg viewBox="{svg_data['viewbox']}" xmlns="http://www.w3.org/2000/svg">
  <title>{name}</title>
'''

    # Shared gradient_id_map for defs and paths
    gradient_map = {}

    # Process defs if present
    if svg_data['defs']:
        cleaned_defs, gradient_map = clean_react_syntax(svg_data['defs'], name, gradient_map)
        svg_content += f"  <defs>\n{cleaned_defs}\n  </defs>\n"
        has_gradients = bool(gradient_map)
    else:
        has_gradients = False

    # Process paths with shared gradient_map
    for path in svg_data['paths']:
        # Clean React syntax from paths
        path_clean, gradient_map = clean_react_syntax(path, name, gradient_map)
        path_clean = path_clean.strip()

        # Add fill color if missing (for Mono icons)
        if not has_gradients and 'fill=' not in path_clean:
            # Add fill before the closing /> or >
            if path_clean.endswith('/>'):
                path_clean = path_clean[:-2] + f' fill="{color}" />'
            elif '>' in path_clean:
                path_clean = path_clean.replace('>', f' fill="{color}">', 1)

        # Replace fill="currentColor" with actual color (only for non-gradient icons)
        if 'fill="currentColor"' in path_clean and not has_gradients:
            path_clean = path_clean.replace('fill="currentColor"', f'fill="{color}"')

        # Format indentation
        if path_clean:
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
            svg_data = extract_svg_from_tsx(tsx_file, provider_name)

            # Create SVG file
            svg_content = create_svg_file(provider_name, svg_data, config['color'])

            # Save to file
            output_file = os.path.join(OUTPUT_DIR, f"{provider_name}.svg")
            with open(output_file, 'w', encoding='utf-8') as f:
                f.write(svg_content)

            print(f"✅ {provider_name}: Created {output_file}")
            print(f"   Color: {config['color']}, Paths: {len(svg_data['paths'])}")

        except Exception as e:
            print(f"❌ {provider_name}: Error - {str(e)}")
            import traceback
            traceback.print_exc()

    print(f"\n✨ Icon extraction complete!")
    print(f"📦 {len(PROVIDERS)} SVG files created in {OUTPUT_DIR}")


if __name__ == "__main__":
    main()
