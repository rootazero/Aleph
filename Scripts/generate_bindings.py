#!/usr/bin/env python3
"""
Generate Swift bindings from UniFFI .udl file

This script uses the uniffi-bindgen Python package to generate Swift bindings.
Install: pip install uniffi-bindgen
"""

import subprocess
import sys
import os

def main():
    # Use Python environment from claude.md
    python_path = "/Users/zouguojun/Workspace/python/.venv/bin/python3"

    # Fallback to system python3 if venv not available
    if not os.path.exists(python_path):
        python_path = "python3"

    try:
        # Generate Swift bindings
        result = subprocess.run([
            python_path, "-m", "uniffi_bindgen", "generate",
            "src/aether.udl",
            "--language", "swift",
            "--out-dir", "bindings"
        ], check=True, capture_output=True, text=True)

        print("✅ Swift bindings generated successfully in ./bindings/")
        print(result.stdout)

    except subprocess.CalledProcessError as e:
        print(f"❌ Error generating bindings: {e}")
        print(e.stderr)
        sys.exit(1)
    except FileNotFoundError:
        print("❌ uniffi-bindgen not found. Install it with: pip install uniffi-bindgen")
        sys.exit(1)

if __name__ == "__main__":
    main()
