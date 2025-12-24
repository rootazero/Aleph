#!/usr/bin/env python3
"""
Verify Swift syntax changes for Phase 9 implementation.
This script checks that the key modifications are present and syntactically valid.
"""

import re
import sys

def check_file_contains(filepath, patterns, description):
    """Check if file contains all patterns."""
    print(f"\nChecking {description}...")
    print(f"File: {filepath}")

    with open(filepath, 'r') as f:
        content = f.read()

    all_found = True
    for pattern_desc, pattern in patterns:
        if isinstance(pattern, str):
            found = pattern in content
        else:
            found = pattern.search(content) is not None

        status = "✓" if found else "✗"
        print(f"  {status} {pattern_desc}")
        if not found:
            all_found = False

    return all_found

def main():
    checks = [
        (
            "Aether/Sources/EventHandler.swift",
            [
                ("onAiProcessingStarted callback", "func onAiProcessingStarted"),
                ("onAiResponseReceived callback", "func onAiResponseReceived"),
                ("handleAiProcessingStarted method", "private func handleAiProcessingStarted"),
                ("handleAiResponseReceived method", "private func handleAiResponseReceived"),
                ("parseHexColor method", "private func parseHexColor"),
                ("retrievingMemory case", re.compile(r"case \.retrievingMemory:")),
                ("processingWithAi case", re.compile(r"case \.processingWithAi:")),
            ],
            "EventHandler callbacks and state handling"
        ),
        (
            "Aether/Sources/HaloState.swift",
            [
                ("retrievingMemory state", "case retrievingMemory"),
                ("processingWithAI state", "case processingWithAI"),
                ("retrievingMemory equality", re.compile(r"case \(\.retrievingMemory, \.retrievingMemory\):")),
                ("processingWithAI equality", re.compile(r"case \(\.processingWithAI.*\):")),
            ],
            "HaloState enum updates"
        ),
        (
            "Aether/Sources/HaloView.swift",
            [
                ("retrievingMemory case in body", "case .retrievingMemory:"),
                ("processingWithAI case in body", "case .processingWithAI"),
                ("retrievingMemory dynamic width", re.compile(r"case \.retrievingMemory.*:")),
                ("processingWithAI dynamic width", re.compile(r"case \.processingWithAI.*:")),
            ],
            "HaloView state rendering"
        ),
        (
            "Aether/Sources/Themes/Theme.swift",
            [
                ("retrievingMemoryView protocol method", "func retrievingMemoryView()"),
                ("processingWithAIView protocol method", "func processingWithAIView"),
                ("retrievingMemoryView default impl", re.compile(r"func retrievingMemoryView\(\) -> AnyView \{")),
                ("processingWithAIView default impl", re.compile(r"func processingWithAIView\(.*\) -> AnyView \{")),
            ],
            "Theme protocol updates"
        ),
        (
            "Aether/Sources/Generated/aether.swift",
            [
                ("retrievingMemory enum case", "case retrievingMemory"),
                ("processingWithAi enum case", "case processingWithAi"),
                ("onAiProcessingStarted protocol", "func onAiProcessingStarted"),
                ("onAiResponseReceived protocol", "func onAiResponseReceived"),
            ],
            "UniFFI generated bindings"
        ),
    ]

    all_passed = True
    for filepath, patterns, description in checks:
        if not check_file_contains(filepath, patterns, description):
            all_passed = False

    print("\n" + "="*60)
    if all_passed:
        print("✓ All syntax checks passed!")
        return 0
    else:
        print("✗ Some checks failed. Please review the output above.")
        return 1

if __name__ == "__main__":
    sys.exit(main())
