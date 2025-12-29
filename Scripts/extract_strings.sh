#!/bin/bash
# extract_strings.sh
# Script to extract localization strings from Swift source files

set -e

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCES_DIR="$PROJECT_DIR/Aether/Sources"
BASE_STRINGS="$PROJECT_DIR/Aether/Resources/en.lproj/Localizable.strings"

echo "🔍 Extracting localization strings from Swift files..."
echo "📁 Scanning directory: $SOURCES_DIR"
echo ""

# Extract NSLocalizedString calls and Text() with string literals
# This will find patterns like:
# - NSLocalizedString("key", comment: "...")
# - Text("key")
# - Button("key")

TEMP_FILE=$(mktemp)

# Find all Swift files and extract string keys
find "$SOURCES_DIR" -name "*.swift" -type f | while read -r file; do
    # Extract NSLocalizedString keys
    grep -Eo 'NSLocalizedString\("[^"]+' "$file" | sed 's/NSLocalizedString("//' || true

    # Extract Text() keys (simple pattern, may have false positives)
    grep -Eo 'Text\("[a-z]+\.[a-z._]+"\)' "$file" | sed 's/Text("//; s/")$//' || true

    # Extract Button() keys
    grep -Eo 'Button\("[a-z]+\.[a-z._]+"\)' "$file" | sed 's/Button("//; s/")$//' || true
done | sort -u > "$TEMP_FILE"

EXTRACTED_COUNT=$(wc -l < "$TEMP_FILE" | tr -d ' ')

echo "✅ Found $EXTRACTED_COUNT unique localization keys in Swift code"
echo ""

if [ -f "$BASE_STRINGS" ]; then
    echo "📋 Comparing with base strings file: $BASE_STRINGS"

    # Extract keys from Localizable.strings (lines matching "key" = "value";)
    STRINGS_KEYS=$(mktemp)
    grep -Eo '^"[^"]+"' "$BASE_STRINGS" | sed 's/"//g' | sort > "$STRINGS_KEYS"

    STRINGS_COUNT=$(wc -l < "$STRINGS_KEYS" | tr -d ' ')
    echo "   Base strings file contains $STRINGS_COUNT keys"
    echo ""

    # Find keys in code but not in strings file
    MISSING_IN_STRINGS=$(comm -23 "$TEMP_FILE" "$STRINGS_KEYS")
    if [ -n "$MISSING_IN_STRINGS" ]; then
        echo "⚠️  Keys used in code but missing from Localizable.strings:"
        echo "$MISSING_IN_STRINGS"
        echo ""
    else
        echo "✅ All code keys are present in Localizable.strings"
        echo ""
    fi

    # Find keys in strings file but not used in code
    UNUSED_KEYS=$(comm -13 "$TEMP_FILE" "$STRINGS_KEYS")
    if [ -n "$UNUSED_KEYS" ]; then
        echo "ℹ️  Keys in Localizable.strings but not found in code (may be dynamically constructed):"
        echo "$UNUSED_KEYS"
        echo ""
    fi

    rm "$STRINGS_KEYS"
else
    echo "⚠️  Base strings file not found at: $BASE_STRINGS"
    echo "   Creating new base strings file would include these keys:"
    cat "$TEMP_FILE"
fi

rm "$TEMP_FILE"

echo ""
echo "✨ String extraction complete!"
