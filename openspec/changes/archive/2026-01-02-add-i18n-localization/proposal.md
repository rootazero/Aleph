# Change: Add Internationalization (i18n) Support

## Metadata
- **ID**: add-i18n-localization
- **Title**: Add Internationalization (i18n) Support
- **Type**: Feature Addition
- **Status**: Deployed
- **Created**: 2025-12-30
- **Deployed**: 2025-12-30

## Why

Aleph currently has all UI text hardcoded in English within Swift source files. This limits accessibility for non-English users and makes the application less inclusive. To support a global user base, we need to:

- Enable UI text to follow the user's system language preference
- Maintain English as fallback when translations are unavailable
- Keep code comments in English for developer clarity
- Support easy addition of new language translations

Without internationalization, we face:
- Limited user base (English speakers only)
- Barrier to entry for non-English macOS users
- Difficulty in community-contributed translations
- Inconsistent localization approach across the codebase

This change implements a robust i18n system using Apple's native localization infrastructure, starting with English and Simplified Chinese support, with an extensible framework for future languages.

## What Changes

- Introduce **`.strings` localization files** for all user-facing text
- Wrap all UI text with `NSLocalizedString()` calls
- Configure **Xcode project localizations** (English as base, Simplified Chinese as first translation)
- Add **system language detection** with graceful fallback to English
- Update **Info.plist** with localized app metadata (CFBundleName, NSHumanReadableCopyright)
- Maintain **English code comments** throughout the codebase

**Deliverables:**
- `en.lproj/Localizable.strings` - English base strings (default)
- `zh-Hans.lproj/Localizable.strings` - Simplified Chinese translations
- Updated all SwiftUI views to use `NSLocalizedString()` or `LocalizedStringKey`
- Xcode project configuration with supported localizations
- Localization documentation for contributors
- String extraction script for developers

**Key Behaviors:**
1. On app launch, detect system language preference via `Locale.preferredLanguages`
2. Load matching `.strings` file (e.g., `zh-Hans.lproj` for Chinese)
3. If translation missing, fallback to English base strings
4. All UI text dynamically updates if user changes system language (requires app restart)
5. Code comments remain in English for international developer collaboration

**Implementation Scope:**
- **Localized Components**:
  - Settings window (all tabs: General, Providers, Routing, Shortcuts, Behavior, Memory)
  - Permission prompts and gates
  - Menu bar items and tooltips
  - Halo overlay status messages (if any)
  - Error messages and alerts
  - Help text and placeholders

- **Not Localized** (intentionally kept in English):
  - Code comments and documentation
  - Debug logs and console output
  - API error responses (from backend)
  - Configuration file keys (TOML)

**Out of Scope (Future Proposals):**
- Right-to-left (RTL) language support (Arabic, Hebrew)
- Plural forms and gender-specific translations
- Dynamic language switching without app restart
- Crowdsourced translation platform integration
- Automated translation validation tests

## Impact

**Affected specs:**
- **MODIFIED**: `settings-ui-layout` - Add localization requirement for all UI text
- **NEW**: `i18n-localization` - Internationalization and localization system

**Affected code:**
- `Aleph/Sources/**/*.swift` - Replace all hardcoded strings with `NSLocalizedString()`
- `Aleph/Resources/en.lproj/Localizable.strings` - **NEW** English base strings
- `Aleph/Resources/zh-Hans.lproj/Localizable.strings` - **NEW** Simplified Chinese strings
- `Aleph/Resources/Info.plist` - Add `CFBundleLocalizations` key
- `Aleph/Info.plist` - Localize app metadata
- `project.yml` - Add localization configuration for XcodeGen

**Dependencies:**
- macOS 13+ Foundation framework (NSLocalizedString API)
- XcodeGen support for localization resources

**Breaking changes:**
- None - purely additive enhancement
- Existing English text preserved as base language
- No API changes or configuration format changes

**Migration:**
- Existing users: UI text may appear in system language on next launch
- No data migration required
- No configuration changes needed
