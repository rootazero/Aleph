#!/bin/bash
# validate_translations.sh
# Script to validate translation completeness across all .lproj directories

set -e

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RESOURCES_DIR="$PROJECT_DIR/Aether/Resources"
BASE_STRINGS="$RESOURCES_DIR/en.lproj/Localizable.strings"

echo "🔍 Validating translations across all localizations..."
echo ""

if [ ! -f "$BASE_STRINGS" ]; then
    echo "❌ Error: Base strings file not found at: $BASE_STRINGS"
    exit 1
fi

# Extract keys from base English strings file
BASE_KEYS=$(mktemp)
grep -Eo '^"[^"]+"' "$BASE_STRINGS" | sed 's/"//g' | sort > "$BASE_KEYS"
BASE_COUNT=$(wc -l < "$BASE_KEYS" | tr -d ' ')

echo "📋 Base language (English): $BASE_COUNT keys"
echo ""

VALIDATION_FAILED=0

# Check each localization directory
for LPROJ_DIR in "$RESOURCES_DIR"/*.lproj; do
    if [ ! -d "$LPROJ_DIR" ]; then
        continue
    fi

    LANG=$(basename "$LPROJ_DIR" .lproj)

    # Skip base language
    if [ "$LANG" = "en" ]; then
        continue
    fi

    LANG_STRINGS="$LPROJ_DIR/Localizable.strings"

    if [ ! -f "$LANG_STRINGS" ]; then
        echo "⚠️  $LANG: Localizable.strings not found"
        VALIDATION_FAILED=1
        continue
    fi

    # Extract keys from translation file
    LANG_KEYS=$(mktemp)
    grep -Eo '^"[^"]+"' "$LANG_STRINGS" | sed 's/"//g' | sort > "$LANG_KEYS"
    LANG_COUNT=$(wc -l < "$LANG_KEYS" | tr -d ' ')

    echo "🌐 Checking $LANG localization ($LANG_COUNT keys):"

    # Find missing keys
    MISSING_KEYS=$(comm -23 "$BASE_KEYS" "$LANG_KEYS")
    if [ -z "$MISSING_KEYS" ]; then
        MISSING_COUNT=0
    else
        MISSING_COUNT=$(echo "$MISSING_KEYS" | wc -l | tr -d ' ')
    fi

    # Find extra keys
    EXTRA_KEYS=$(comm -13 "$BASE_KEYS" "$LANG_KEYS")
    if [ -z "$EXTRA_KEYS" ]; then
        EXTRA_COUNT=0
    else
        EXTRA_COUNT=$(echo "$EXTRA_KEYS" | wc -l | tr -d ' ')
    fi

    if [ "$MISSING_COUNT" -gt 0 ]; then
        echo "   ❌ Missing $MISSING_COUNT keys:"
        echo "$MISSING_KEYS" | sed 's/^/      - /'
        VALIDATION_FAILED=1
    else
        echo "   ✅ All base keys present"
    fi

    if [ "$EXTRA_COUNT" -gt 0 ]; then
        echo "   ⚠️  Extra $EXTRA_COUNT keys (not in base):"
        echo "$EXTRA_KEYS" | sed 's/^/      - /'
    fi

    # Check for duplicate keys
    DUPLICATE_KEYS=$(grep -Eo '^"[^"]+"' "$LANG_STRINGS" | sed 's/"//g' | sort | uniq -d)
    if [ -n "$DUPLICATE_KEYS" ]; then
        echo "   ❌ Duplicate keys found:"
        echo "$DUPLICATE_KEYS" | sed 's/^/      - /'
        VALIDATION_FAILED=1
    fi

    # Calculate coverage percentage
    PRESENT_COUNT=$((LANG_COUNT - MISSING_COUNT))
    COVERAGE=$(awk "BEGIN {printf \"%.1f\", $PRESENT_COUNT / $BASE_COUNT * 100}")
    echo "   📊 Coverage: $COVERAGE%"
    echo ""

    rm "$LANG_KEYS"
done

rm "$BASE_KEYS"

echo ""
if [ $VALIDATION_FAILED -eq 0 ]; then
    echo "✅ All translations are complete!"
    exit 0
else
    echo "❌ Translation validation failed. Please update missing translations."
    exit 1
fi
