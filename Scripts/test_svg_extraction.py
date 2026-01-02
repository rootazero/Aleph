#!/usr/bin/env python3
"""Test SVG extraction to debug gradient issue"""

import os
import re

TSX_FILE = os.path.expanduser("~/workspace/lobe-icons/src/Gemini/components/Color.tsx")

with open(TSX_FILE, 'r', encoding='utf-8') as f:
    content = f.read()

# Extract all path elements
paths = re.findall(r'<path[^>]*?(?:/>|>.*?</path>)', content, re.DOTALL)

print(f"Found {len(paths)} paths\n")

for i, path in enumerate(paths, 1):
    print(f"=== Path {i} ===")
    print(path[:200])  # First 200 chars
    print("...")
    print()

# Test clean_react_syntax on one path
test_path = paths[1]  # Second path with {a.fill}
print("=== Testing clean_react_syntax on path 2 ===")
print("Original:")
print(test_path[:300])
print()

# Simulate clean_react_syntax
gradient_id_map = {'a': 'gemini-gradient-1', 'b': 'gemini-gradient-2', 'c': 'gemini-gradient-3'}

def replace_gradient_fill(match):
    var_name = match.group(1)
    if var_name in gradient_id_map:
        return f'"url(#{gradient_id_map[var_name]})"'
    return '"currentColor"'

cleaned = re.sub(r'\{(\w+)\.fill\}', replace_gradient_fill, test_path)

print("After regex replacement:")
print(cleaned[:300])
