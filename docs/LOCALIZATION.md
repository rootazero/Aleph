# Localization Guide

This guide explains how Aether implements internationalization (i18n) and how contributors can add new language translations.

## Overview

Aether uses Apple's native localization infrastructure to support multiple languages:
- **Base Language**: English (`en.lproj/`)
- **Supported Languages**: Simplified Chinese (`zh-Hans.lproj/`)

## Architecture

### Localization Files

```
Aether/Resources/
├── en.lproj/                       # English (base)
│   ├── Localizable.strings        # UI text
│   └── InfoPlist.strings          # App metadata & permissions
└── zh-Hans.lproj/                 # Simplified Chinese
    ├── Localizable.strings
    └── InfoPlist.strings
```

### String Key Naming Convention

All localization keys follow a hierarchical dot notation:

| Category | Pattern | Example |
|----------|---------|---------|
| Common UI | `common.<element>` | `common.save`, `common.cancel` |
| Settings Tabs | `settings.<tab>.<element>` | `settings.general.title` |
| Providers | `provider.<category>.<field>` | `provider.field.api_key` |
| Menu Bar | `menu.<item>` | `menu.settings`, `menu.quit` |
| Permissions | `permission.<type>.<element>` | `permission.accessibility.title` |
| Errors | `error.<context>.<specific>` | `error.provider.invalid_api_key` |
| Alerts | `alert.<type>` | `alert.confirm_delete` |

### Usage in Code

#### SwiftUI (Recommended)

SwiftUI automatically resolves `LocalizedStringKey`:

```swift
// Text components
Text("settings.general.title")      // Automatically localized
Button("common.save") { }            // Button labels

// Section headers
Section(header: Text("settings.general.sound")) {
    // ...
}
```

#### NSLocalizedString (AppKit/UIKit)

For dynamic strings or NSAlert:

```swift
let alertTitle = NSLocalizedString("common.ok", comment: "OK button")

// String interpolation
String.localizedStringWithFormat(
    NSLocalizedString("settings.general.coming_soon_message", comment: ""),
    featureName
)
```

## Adding a New Language

### Step 1: Create Localization Directory

```bash
mkdir -p Aether/Resources/<lang-code>.lproj
```

**Common Language Codes:**
- `en` - English
- `zh-Hans` - Simplified Chinese
- `zh-Hant` - Traditional Chinese
- `ja` - Japanese
- `ko` - Korean
- `de` - German
- `fr` - French
- `es` - Spanish

### Step 2: Copy Base Strings Files

```bash
cp Aether/Resources/en.lproj/Localizable.strings \
   Aether/Resources/<lang-code>.lproj/Localizable.strings

cp Aether/Resources/en.lproj/InfoPlist.strings \
   Aether/Resources/<lang-code>.lproj/InfoPlist.strings
```

### Step 3: Translate Strings

Edit `<lang-code>.lproj/Localizable.strings`:

```strings
/* Common UI Elements */
"common.save" = "Your Translation";
"common.cancel" = "Your Translation";

/* Settings Window - General Tab */
"settings.general.title" = "Your Translation";
// ...
```

**Translation Guidelines:**
1. **Maintain Tone**: Use consistent formal/informal pronouns
2. **Technical Terms**: Prefer widely-accepted translations over literal
3. **Conciseness**: Keep button labels short to fit UI layouts
4. **Context**: Check comment annotations for usage context

### Step 4: Update XcodeGen Configuration

Edit `project.yml`:

```yaml
info:
  properties:
    CFBundleLocalizations:
      - en
      - zh-Hans
      - <lang-code>  # Add your language code
```

### Step 5: Regenerate Xcode Project

```bash
xcodegen generate
```

### Step 6: Validate Translations

```bash
./Scripts/validate_translations.sh
```

Expected output:
```
🌐 Checking <lang-code> localization:
   ✅ All base keys present
   📊 Coverage: 100.0%
```

## Translation Validation

### Extract Used Strings

Find all localization keys referenced in Swift code:

```bash
./Scripts/extract_strings.sh
```

### Validate Completeness

Check all translations are complete:

```bash
./Scripts/validate_translations.sh
```

This script:
- Compares all `.lproj` directories with base English
- Reports missing keys
- Reports extra keys (not in base)
- Calculates coverage percentage

### Pre-Commit Hook (Optional)

Automatically validate translations before commits:

```bash
# Create hook
cat > .git/hooks/pre-commit << 'EOF'
#!/bin/bash
./Scripts/validate_translations.sh
EOF

chmod +x .git/hooks/pre-commit
```

## Testing Localizations

### Test in Different Languages

#### Method 1: Change System Language

1. Open **System Settings** > **General** > **Language & Region**
2. Add your target language to **Preferred Languages**
3. Drag it to the top of the list
4. Restart Aether

#### Method 2: Launch with Specific Language (Debug)

```bash
# Launch with Simplified Chinese
open -a Aether --args -AppleLanguages '(zh-Hans)'

# Launch with Japanese
open -a Aether --args -AppleLanguages '(ja)'
```

### Verify UI Layout

Check for:
- **Text Truncation**: Ensure labels don't get cut off
- **Button Overflow**: Verify buttons expand to fit text
- **Line Wrapping**: Confirm multi-line text wraps correctly
- **Alignment**: Check text alignment in RTL languages (future)

## Translation Quality Checklist

### Before Submitting

- [ ] All keys from base English are translated
- [ ] No duplicate keys in `.strings` file
- [ ] Translation validation script passes
- [ ] Tested in app with target language
- [ ] No UI layout issues (truncation, overflow)
- [ ] Technical terms are consistent
- [ ] Tone/formality is consistent
- [ ] Permission descriptions are accurate

### Common Issues

**Issue**: Keys missing from translation file
**Solution**: Run `./Scripts/extract_strings.sh` to find missing keys

**Issue**: Translation validation fails
**Solution**: Check for typos in key names, ensure exact match with base English

**Issue**: Text gets truncated in UI
**Solution**: Use shorter synonyms or abbreviations where appropriate

**Issue**: String interpolation not working
**Solution**: Ensure placeholders (`%@`) are in the correct position

## Contributing Translations

### Pull Request Checklist

1. Add new `.lproj` directory with translated strings
2. Update `project.yml` with language code
3. Run `./Scripts/validate_translations.sh` (must pass)
4. Test app in target language
5. Include screenshots in PR showing translated UI
6. Describe any UI layout adjustments made

### Translation Credits

We welcome community translations! Contributors will be credited in:
- `README.md` - "Supported Languages" section
- `docs/CONTRIBUTORS.md` - Localization section

## Troubleshooting

### Strings Not Updating After Edit

1. Clean Xcode build: `Cmd + Shift + K`
2. Delete DerivedData: `rm -rf ~/Library/Developer/Xcode/DerivedData/Aether-*`
3. Rebuild project

### Wrong Language Displayed

1. Check system language in **System Settings**
2. Verify `.lproj` directory exists for that language
3. Confirm `CFBundleLocalizations` includes the language
4. Restart app after language change

### Permission Prompts in English

macOS caches `InfoPlist.strings`. To refresh:
1. Remove app from **Privacy & Security** settings
2. Delete app from `/Applications`
3. Clean build and reinstall

## Resources

- [Apple Localization Guide](https://developer.apple.com/documentation/xcode/localization)
- [NSLocalizedString Documentation](https://developer.apple.com/documentation/foundation/nslocalizedstring)
- [ISO 639 Language Codes](https://en.wikipedia.org/wiki/List_of_ISO_639-1_codes)

## Examples

### Full Example: Adding Japanese Localization

```bash
# 1. Create directory
mkdir -p Aether/Resources/ja.lproj

# 2. Copy base files
cp Aether/Resources/en.lproj/*.strings Aether/Resources/ja.lproj/

# 3. Translate (edit ja.lproj/Localizable.strings)
# "common.save" = "保存";
# "common.cancel" = "キャンセル";
# ...

# 4. Update project.yml
# CFBundleLocalizations: [en, zh-Hans, ja]

# 5. Regenerate project
xcodegen generate

# 6. Validate
./Scripts/validate_translations.sh

# 7. Test
open -a Aether --args -AppleLanguages '(ja)'
```

---

**Maintained by**: Aether Development Team
**Last Updated**: 2025-12-29
**Status**: ✅ Active (English + Simplified Chinese)
**Translation Keys**: 249 keys (100% coverage)
**Translation Files**: Localizable.strings + InfoPlist.strings
