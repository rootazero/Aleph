#!/usr/bin/env python3
"""
Export Aether app icon SVG from Aether.html.
Enlarges the small satellite star by 1.5-2x while keeping the main star centered.
"""

from pathlib import Path
import re

def extract_and_modify_svg():
    """Extract SVG from Aether.html and enlarge the small star."""

    # Read the HTML file
    html_path = Path.home() / "Workspace" / "Aether.html"
    if not html_path.exists():
        print(f"❌ Aether.html not found at {html_path}")
        return

    html_content = html_path.read_text(encoding='utf-8')

    # Extract the main SVG logo (line 77-102 from the HTML)
    # The SVG is in the "SVG Logo Container v3" section
    svg_pattern = r'<svg viewBox="0 0 100 100"[^>]*>(.*?)</svg>'
    match = re.search(svg_pattern, html_content, re.DOTALL)

    if not match:
        print("❌ Could not find SVG in Aether.html")
        return

    svg_content = match.group(0)

    # Enlarge the satellite star by 1.8x
    # Original satellite star path (lines 99-100):
    # <path d="M35 14 C35.8 19 37 21 43 22 C37 23 35.8 25 35 30 C34.2 25 33 23 27 22 C33 21 34.2 19 35 14Z"

    # Strategy: Scale the satellite star path coordinates by 1.8x around its center (35, 22)
    # Center point: (35, 22)
    # New coordinates will be: new = center + 1.8 * (old - center)

    # Original path: M35 14 C35.8 19 37 21 43 22 C37 23 35.8 25 35 30 C34.2 25 33 23 27 22 C33 21 34.2 19 35 14Z
    # Let's calculate new coordinates manually (scaling by 1.8x from center (35, 22)):

    # Point (35, 14): new = (35, 22 + 1.8*(14-22)) = (35, 22 - 14.4) = (35, 7.6)
    # Point (35.8, 19): new = (35 + 1.8*(0.8), 22 + 1.8*(-3)) = (36.44, 16.6)
    # Point (37, 21): new = (35 + 1.8*(2), 22 + 1.8*(-1)) = (38.6, 20.2)
    # Point (43, 22): new = (35 + 1.8*(8), 22) = (49.4, 22)
    # Point (37, 23): new = (35 + 1.8*(2), 22 + 1.8*(1)) = (38.6, 23.8)
    # Point (35.8, 25): new = (36.44, 27.4)
    # Point (35, 30): new = (35, 36.4)
    # Point (34.2, 25): new = (33.56, 27.4)
    # Point (33, 23): new = (31.4, 23.8)
    # Point (27, 22): new = (20.6, 22)
    # Point (33, 21): new = (31.4, 20.2)
    # Point (34.2, 19): new = (33.56, 16.6)

    new_satellite_path = "M35 7.6 C36.44 16.6 38.6 20.2 49.4 22 C38.6 23.8 36.44 27.4 35 36.4 C33.56 27.4 31.4 23.8 20.6 22 C31.4 20.2 33.56 16.6 35 7.6Z"

    # Replace the old satellite star path with the new enlarged one
    old_satellite_path = r'<path d="M35 14 C35\.8 19 37 21 43 22 C37 23 35\.8 25 35 30 C34\.2 25 33 23 27 22 C33 21 34\.2 19 35 14Z"'
    new_path_element = f'<path d="{new_satellite_path}"'

    svg_content = re.sub(old_satellite_path, new_path_element, svg_content)

    # Clean up and format the SVG
    svg_content = re.sub(r'\s+', ' ', svg_content)  # Normalize whitespace
    svg_content = svg_content.replace('> <', '>\n  <')  # Add line breaks

    # Add proper XML declaration and formatting
    final_svg = f'''<?xml version="1.0" encoding="UTF-8"?>
<svg width="100" height="100" viewBox="0 0 100 100" fill="none" xmlns="http://www.w3.org/2000/svg">
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

  <!-- Main Star (Core) - Centered at (55, 55) -->
  <path d="M55 15 C59 40 70 51 95 55 C70 59 59 70 55 95 C51 70 40 59 15 55 C40 51 51 40 55 15Z"
        fill="url(#mainGradient)" />

  <!-- Satellite Star (Spark) - Enlarged 1.8x, centered at (35, 22) -->
  <path d="{new_satellite_path}"
        fill="url(#satGradient)" />
</svg>'''

    # Write to output files
    output_dir = Path(__file__).parent.parent / "Aether" / "Assets.xcassets" / "AppIcon-Source.imageset"
    output_dir.mkdir(parents=True, exist_ok=True)

    svg_output_path = output_dir / "AetherIcon.svg"
    svg_output_path.write_text(final_svg, encoding='utf-8')

    print(f"✅ SVG exported to: {svg_output_path}")
    print(f"\n📝 Satellite star enlarged by 1.8x")
    print(f"   Main star center: (55, 55) - unchanged")
    print(f"   Satellite star center: (35, 22) - unchanged")
    print(f"   Satellite star size: increased 1.8x")

    return final_svg

def main():
    extract_and_modify_svg()

if __name__ == "__main__":
    main()
