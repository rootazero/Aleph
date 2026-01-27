# Change: Add Language Selector to General Settings

## Status
- **Status**: Proposed
- **Created**: 2026-01-03
- **Author**: AI Assistant
- **Approver**: TBD

## Why

Users need the ability to manually override the system language preference for Aether's UI. While Aether currently follows the macOS system language (supporting English and Simplified Chinese), there are scenarios where users may want to use a different language for Aether than their system-wide setting:

1. **Multilingual users** who prefer different languages for different applications
2. **Testing and development** where developers need to verify different language translations
3. **Preparing for future language expansion** - a UI component is needed to support adding more languages (Japanese, Korean, German, French, Spanish, etc.)

This change provides a foundation for better internationalization support while maintaining backward compatibility with system language detection as the default.

## What Changes

- **ADD** Language selection dropdown to General Settings tab
- **ADD** `language` field to `GeneralConfig` struct in Rust core
- **ADD** Logic to persist language preference in config.toml
- **ADD** Mechanism to override `AppleLanguages` when custom language is selected
- **UPDATE** GeneralSettingsView to include language selector UI

**Key Behaviors:**
- Dropdown will show: "System Default" (auto-detect) + all supported languages (English, 简体中文, and future additions)
- If user selects "System Default": Aether follows macOS system language
- If user selects a specific language: Aether overrides with that language
- **Restart requirement**: Language changes require app restart to take effect (due to Apple's localization caching)
- Show alert to user after saving: "Language will change after restarting Aether"

## Impact

### Affected Specs
- `settings-ui-layout` - Adding new field to General Settings tab
- `core-library` - Extending `GeneralConfig` with language preference

### Affected Code
- **Swift Layer**:
  - `Aether/Sources/SettingsView.swift` (`GeneralSettingsView`)
  - `Aether/Sources/AetherApp.swift` (apply language override on launch)
- **Rust Layer**:
  - `Aether/core/src/config/mod.rs` (`GeneralConfig` struct)
  - `Aether/core/src/aether.udl` (UniFFI interface for language config)

### User Experience Impact
- Minimal - new optional setting in General tab
- Non-breaking - defaults to system language if not configured
- Requires restart alert when language is changed

## Dependencies
- Requires existing i18n infrastructure (already in place)
- Requires existing config persistence layer (already in place)

## Risks and Mitigations

| Risk | Severity | Mitigation |
|------|----------|-----------|
| User confusion about restart requirement | Low | Clear alert message after saving language preference |
| Language change not taking effect | Medium | Validate language override logic in AppDelegate; provide manual restart instructions |
| Unsupported language selected | Low | Only show languages that have `.lproj` directories; validate on config load |

## Success Criteria
- [ ] Language dropdown appears in General Settings tab
- [ ] Selecting a language persists to `~/.aether/config.toml`
- [ ] App restart applies the selected language to all UI elements
- [ ] Selecting "System Default" restores macOS system language behavior
- [ ] Alert notifies user about restart requirement
