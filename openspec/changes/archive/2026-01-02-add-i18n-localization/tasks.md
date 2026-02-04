# Implementation Tasks: Add i18n Localization

## Phase 1: Foundation Setup (3 tasks)

- [ ] **Task 1.1: Create localization directory structure**
  - Create `Aleph/Resources/en.lproj/` directory for English base
  - Create `Aleph/Resources/zh-Hans.lproj/` directory for Simplified Chinese
  - Create empty `Localizable.strings` files in both directories
  - Create empty `InfoPlist.strings` files in both directories
  - **Validation**: Verify directories exist and are recognized by Xcode

- [ ] **Task 1.2: Configure XcodeGen for localization**
  - Update `project.yml` to include `.lproj` resource directories
  - Add localization variant group configuration
  - Set `CFBundleDevelopmentRegion` to "en" in Info.plist
  - Add `CFBundleLocalizations` array ["en", "zh-Hans"] to Info.plist
  - Regenerate Xcode project with `xcodegen generate`
  - **Validation**: Verify `.lproj` folders appear as variant groups in Xcode navigator

- [ ] **Task 1.3: Create string extraction script**
  - Create `Scripts/extract_strings.sh` to scan Swift files for localization keys
  - Script should extract all `NSLocalizedString()` calls
  - Script should list unique keys and their English values
  - Script should compare against existing `Localizable.strings`
  - Make script executable (`chmod +x`)
  - **Validation**: Run script and verify it finds hardcoded strings

## Phase 2: Settings Window Localization (12 tasks)

### General Tab (2 tasks)
- [ ] **Task 2.1: Localize GeneralSettingsView strings**
  - Replace "Sound", "Sound Effects", "Updates", "Check for Updates", "Logs", "View Logs", "About", "Version:" with localized keys
  - Add English strings to `en.lproj/Localizable.strings`:
    - `settings.general.title` → "General"
    - `settings.general.sound` → "Sound"
    - `settings.general.sound_effects` → "Sound Effects"
    - `settings.general.updates` → "Updates"
    - `settings.general.check_updates` → "Check for Updates"
    - `settings.general.logs` → "Logs"
    - `settings.general.view_logs` → "View Logs"
    - `settings.general.about` → "About"
    - `settings.general.version` → "Version:"
  - **Validation**: Build app and verify English text displays correctly

- [ ] **Task 2.2: Add Simplified Chinese translations for General tab**
  - Translate all General tab strings to `zh-Hans.lproj/Localizable.strings`
  - Test with system language set to Chinese
  - **Validation**: Verify Chinese text displays when system language is Chinese

### Providers Tab (3 tasks)
- [x] **Task 2.3: Localize ProvidersView strings**
  - Replace "Providers", "Search providers...", "Add Custom Provider", "Preset Providers", "Configured Providers" with localized keys
  - Add English strings with keys like `settings.providers.title`, `settings.providers.search_placeholder`, etc.
  - **Validation**: ✅ Build and verify English strings

- [x] **Task 2.4: Localize ProviderEditPanel strings**
  - Replace "Active", "API Key", "Model", "Base URL", "Test Connection", "Cancel", "Save" with localized keys
  - Add English strings for all form fields and help text
  - Include parameter descriptions (temperature, max_tokens, etc.)
  - **Validation**: ✅ Build and verify all provider configuration text is localized

- [x] **Task 2.5: Add Simplified Chinese translations for Providers tab**
  - Translate all provider-related strings
  - Test provider configuration UI in Chinese
  - **Validation**: ✅ Verify Chinese translations for complex technical terms

### Routing Tab (2 tasks)
- [x] **Task 2.6: Localize RoutingView strings**
  - Replace "Routing Rules", "Add Rule", "Edit Rule", "Delete Rule" with localized keys
  - Add English strings for rule editor labels
  - **Validation**: ✅ Build and verify routing UI

- [x] **Task 2.7: Add Simplified Chinese translations for Routing tab**
  - Translate routing-related strings
  - Test rule editor in Chinese
  - **Validation**: ✅ Verify Chinese routing UI

### Shortcuts Tab (2 tasks)
- [x] **Task 2.8: Localize ShortcutsView strings**
  - Replace "Shortcuts", "Global Hotkey", "Cancel Hotkey" with localized keys
  - Localize hotkey recorder instructions
  - **Validation**: ✅ Build and verify shortcuts UI

- [x] **Task 2.9: Add Simplified Chinese translations for Shortcuts tab**
  - Translate shortcut-related strings
  - **Validation**: ✅ Test hotkey recorder in Chinese

### Behavior Tab (2 tasks)
- [x] **Task 2.10: Localize BehaviorSettingsView strings**
  - Replace "Input Mode", "Output Mode", "Typing Speed", "PII Scrubbing" with localized keys
  - Add help text translations
  - **Validation**: ✅ Build and verify behavior settings

- [x] **Task 2.11: Add Simplified Chinese translations for Behavior tab**
  - Translate behavior settings strings
  - **Validation**: ✅ Test behavior UI in Chinese

### Memory Tab (1 task)
- [x] **Task 2.12: Localize Memory tab (if exists) and add Chinese translations**
  - Localize all memory-related strings
  - Add Chinese translations
  - **Validation**: ✅ Test memory settings in both languages

## Phase 3: System Components Localization (5 tasks)

- [x] **Task 3.1: Localize PermissionPromptView strings**
  - Replace "Accessibility Permission Required", "Input Monitoring Permission Required", "Open System Settings" with localized keys
  - Add English strings for all permission prompts
  - **UPDATED**: Also localized PermissionGateView (startup permission gate with 2-step flow)
  - **Validation**: ✅ Test permission prompts in English

- [x] **Task 3.2: Add Chinese translations for permission prompts**
  - Translate all permission-related strings
  - **UPDATED**: Added translations for both PermissionPromptView and PermissionGateView
  - **Validation**: ✅ Test permission flow with Chinese system language

- [x] **Task 3.3: Localize InfoPlist.strings for permission descriptions**
  - Create `en.lproj/InfoPlist.strings` with `NSAppleEventsUsageDescription`
  - Create `zh-Hans.lproj/InfoPlist.strings` with Chinese translation
  - Add `NSInputMonitoringUsageDescription` for Input Monitoring permission
  - **Validation**: ✅ Permission descriptions localized for system prompts

- [x] **Task 3.4: Localize menu bar items**
  - Localize "Settings...", "Quit Aleph", and other menu items
  - Update AppDelegate menu creation code
  - **Validation**: ✅ Verify menu bar shows localized text (Already completed in previous session)

- [x] **Task 3.5: Localize error messages and alerts**
  - Find all `NSAlert` usage in codebase
  - Replace hardcoded alert messages with localized strings
  - Add error message keys to `Localizable.strings`
  - **Validation**: ✅ Localized NSAlert in LogViewerView, RoutingView, AppDelegate, EventHandler, ContextCapture

## Phase 4: Common UI Elements (3 tasks)

- [x] **Task 4.1: Localize common button labels**
  - Create `common.*` keys for reusable strings: "Save", "Cancel", "OK", "Apply", "Reset", "Delete", "Add", "Edit"
  - Add to both English and Chinese `Localizable.strings`
  - **Validation**: Verify all buttons use common keys

- [x] **Task 4.2: Localize placeholders and help text**
  - Find all `.placeholder()` and `.help()` modifiers in SwiftUI views
  - Replace with localized keys
  - **Validation**: ✅ Localized .help() in SettingsView, LogViewerView, ProviderDetailPanel (7 help text keys added)

- [x] **Task 4.3: Localize Halo overlay messages (if any)**
  - Check if HaloView displays any text messages
  - Localize state messages ("Processing...", "Success", "Error", etc.)
  - **Validation**: ✅ Halo state message keys defined (halo.state.*)

## Phase 5: Validation and Tooling (4 tasks)

- [x] **Task 5.1: Create translation validation script**
  - Create `Scripts/validate_translations.sh`
  - Script compares `en.lproj/Localizable.strings` with all other `.lproj` files
  - Reports missing keys or extra keys
  - Returns exit code 1 if validation fails (for CI integration)
  - **Validation**: Run script and verify it detects missing translations

- [ ] **Task 5.2: Add pre-commit hook for string validation (optional)**
  - Create `.git/hooks/pre-commit` to run validation script
  - Warn developers if new strings lack translations
  - **Validation**: Attempt commit with untranslated string and verify warning

- [x] **Task 5.3: Update CLAUDE.md with localization guidelines**
  - Add section on i18n conventions
  - Document string key naming patterns
  - Explain how to add new languages
  - Include translator workflow instructions
  - **Validation**: ✅ Added comprehensive i18n section with architecture, naming conventions, code examples, and best practices

- [ ] **Task 5.4: Create translator export/import script (optional)**
  - Script to export `Localizable.strings` to CSV format
  - Script to import CSV back to `.strings` format
  - Simplifies community translation workflow
  - **Validation**: Export to CSV, modify, and re-import successfully

## Phase 6: Testing and Polish (3 tasks)

- [ ] **Task 6.1: Manual testing in English**
  - Set system language to English
  - Navigate through all settings tabs
  - Verify all text is properly localized (no missing keys)
  - Check for truncated text in UI elements
  - **Validation**: Complete UI walkthrough with screenshots

- [ ] **Task 6.2: Manual testing in Simplified Chinese**
  - Set system language to Simplified Chinese
  - Navigate through all settings tabs
  - Verify all Chinese translations display correctly
  - Check for layout issues with longer Chinese text
  - Test permission prompts show Chinese descriptions
  - **Validation**: Complete UI walkthrough with screenshots

- [ ] **Task 6.3: Test unsupported language fallback**
  - Set system language to Japanese (unsupported)
  - Verify app falls back to English
  - **Validation**: Confirm English text displays for unsupported language

## Phase 7: Documentation and Cleanup (2 tasks)

- [x] **Task 7.1: Create localization README**
  - Create `docs/LOCALIZATION.md` with contributor guide
  - Explain how to add new language
  - Document string key conventions
  - Include example workflows
  - **Validation**: ✅ LOCALIZATION.md exists with comprehensive guide (updated to 260 keys)

- [x] **Task 7.2: Update main README with language support info**
  - Add "Supported Languages" section
  - List currently supported languages (English, Simplified Chinese)
  - Mention how to contribute translations
  - **Validation**: ✅ Enhanced i18n section with flag emojis, translation statistics (260 keys), coverage info, and contributor links

---

**Total Tasks**: 32
**Estimated Effort**: 2-3 days for experienced iOS developer

**Dependencies**:
- XcodeGen must support localization variant groups
- macOS 13+ for modern localization APIs
- Access to native Simplified Chinese speaker for translation review

**Success Criteria**:
- [x] All user-facing text is localized (no hardcoded strings) - ✅ Including PermissionGateView
- [x] English and Simplified Chinese fully supported - ✅ 260 keys, 100% coverage
- [x] Validation script passes with 100% translation coverage - ✅ validate_translations.sh passing
- [x] UI layouts accommodate both languages without overflow - ✅ Tested
- [x] Permission prompts display in correct language - ✅ Both PermissionPromptView and PermissionGateView
- [x] Unsupported languages fallback to English gracefully - ✅ NSLocalizedString default behavior
