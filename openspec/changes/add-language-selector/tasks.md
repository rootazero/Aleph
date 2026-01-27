# Implementation Tasks

## 1. Backend (Rust Core) - Language Configuration

### Task 1.1: Add `language` field to `GeneralConfig`
- [ ] Add `language: Option<String>` field to `GeneralConfig` struct in `Aether/core/src/config/mod.rs`
- [ ] Add serde attribute: `#[serde(default, skip_serializing_if = "Option::is_none")]`
- [ ] Document field: "Preferred language override (e.g., 'en', 'zh-Hans'). If None, use system language."

### Task 1.2: Validate language configuration on load
- [ ] Add validation logic to ensure `language` value (if set) matches an existing `.lproj` directory
- [ ] Return error or warning if invalid language code is detected
- [ ] Log warning if language is set but localization files are missing

### Task 1.3: Expose language config via UniFFI
- [ ] Verify `GeneralConfig` is already exposed through UniFFI (it is, via `FullConfig`)
- [ ] No changes needed to `aether.udl` (language field auto-exposed via nested struct)

**Validation:**
- Run `cargo build` successfully
- Run `cargo test` for config module
- Verify `language` field appears in generated Swift bindings

---

## 2. Frontend (Swift UI) - Language Selector UI

### Task 2.1: Add language dropdown to `GeneralSettingsView`
- [ ] Open `Aether/Sources/SettingsView.swift`
- [ ] Add new `Section` with header `"settings.general.language"` after the "Sound" section
- [ ] Add `Picker` with label `"settings.general.language_preference"`
- [ ] Add options:
  - "System Default" (value: `nil`)
  - "English" (value: `"en"`)
  - "简体中文" (value: `"zh-Hans"`)
- [ ] Bind picker to `@State private var selectedLanguage: String?`

### Task 2.2: Load current language setting from config
- [ ] Add `onAppear` handler to `GeneralSettingsView`
- [ ] Read `core?.getConfig().general.language` and set `selectedLanguage`
- [ ] Handle `nil` case (display "System Default")

### Task 2.3: Save language preference and show restart alert
- [ ] Add `onChange(of: selectedLanguage)` handler
- [ ] Call `core?.updateConfig()` to persist language change
- [ ] Show `NSAlert` with message: "Language will change after restarting Aether. Restart now?"
- [ ] Add "Restart Now" button that calls `NSApp.terminate(nil)`
- [ ] Add "Later" button that dismisses alert

**Validation:**
- Build and run app
- Verify dropdown appears in General Settings
- Verify selecting a language shows restart alert
- Verify selection persists to `~/.aether/config.toml`

---

## 3. Application Launch - Apply Language Override

### Task 3.1: Apply language override in `AppDelegate` or `AetherApp`
- [ ] Open `Aether/Sources/AetherApp.swift` or `Aether/Sources/AppDelegate.swift`
- [ ] In `applicationDidFinishLaunching` (AppKit) or `init()` (SwiftUI), check for language override
- [ ] Read language from config: `let config = core?.getConfig()`
- [ ] If `config.general.language` is set, apply override:
  ```swift
  if let language = config.general.language {
      UserDefaults.standard.set([language], forKey: "AppleLanguages")
      UserDefaults.standard.synchronize()
  }
  ```
- [ ] Log applied language for debugging

### Task 3.2: Handle "System Default" selection
- [ ] If language is `nil`, remove override:
  ```swift
  UserDefaults.standard.removeObject(forKey: "AppleLanguages")
  UserDefaults.standard.synchronize()
  ```

**Validation:**
- Restart app after selecting English → UI shows in English
- Restart app after selecting 简体中文 → UI shows in Chinese
- Restart app after selecting "System Default" → UI follows macOS system language

---

## 4. Localization - Add New String Keys

### Task 4.1: Add English strings
- [ ] Open `Aether/Resources/en.lproj/Localizable.strings`
- [ ] Add keys:
  ```strings
  /* Settings - General - Language */
  "settings.general.language" = "Language";
  "settings.general.language_preference" = "Preferred Language";
  "settings.general.language_system_default" = "System Default";
  "settings.general.language_restart_title" = "Restart Required";
  "settings.general.language_restart_message" = "Language will change after restarting Aether. Restart now?";
  "settings.general.language_restart_now" = "Restart Now";
  "settings.general.language_restart_later" = "Later";
  ```

### Task 4.2: Add Simplified Chinese strings
- [ ] Open `Aether/Resources/zh-Hans.lproj/Localizable.strings`
- [ ] Add translated keys:
  ```strings
  /* Settings - General - Language */
  "settings.general.language" = "语言";
  "settings.general.language_preference" = "首选语言";
  "settings.general.language_system_default" = "系统默认";
  "settings.general.language_restart_title" = "需要重启";
  "settings.general.language_restart_message" = "语言将在重启 Aether 后生效。是否立即重启?";
  "settings.general.language_restart_now" = "立即重启";
  "settings.general.language_restart_later" = "稍后";
  ```

**Validation:**
- Run `./Scripts/validate_translations.sh` (if available)
- Verify all keys are present in both language files

---

## 5. Testing and Documentation

### Task 5.1: Manual testing
- [ ] Test language selection flow:
  1. Select English → Restart → Verify UI is in English
  2. Select 简体中文 → Restart → Verify UI is in Chinese
  3. Select "System Default" → Restart → Verify UI follows system language
- [ ] Test config persistence:
  1. Check `~/.aether/config.toml` after each selection
  2. Verify `[general]` section contains `language = "en"` or `language = "zh-Hans"` or omits the field for system default

### Task 5.2: Edge case testing
- [ ] Test invalid language code in config.toml (e.g., `language = "invalid"`)
- [ ] Verify app falls back to system language
- [ ] Test rapid language switching (select language, restart immediately)

### Task 5.3: Update documentation
- [ ] Add note to `docs/LOCALIZATION.md` explaining language selector feature
- [ ] Document that language changes require app restart
- [ ] Update user guide (if exists) with language selection instructions

**Validation:**
- All manual tests pass
- No crashes or UI glitches when switching languages
- Documentation is clear and accurate

---

## 6. Code Review and Cleanup

### Task 6.1: Code review checklist
- [ ] Rust code follows project conventions (snake_case, proper error handling)
- [ ] Swift code follows project conventions (camelCase, SwiftUI best practices)
- [ ] All strings are localized (no hardcoded English text)
- [ ] Config validation handles edge cases
- [ ] Alert UX is clear and non-intrusive

### Task 6.2: Commit changes
- [ ] Stage all changes: `git add .`
- [ ] Commit with message: `feat(settings): add language selector to General Settings`
- [ ] Reference this change proposal in commit body

---

## Summary

**Total Tasks**: 16
**Estimated Effort**: 2-3 hours
**Risk Level**: Low (non-breaking change, existing i18n infrastructure)

**Dependencies:**
- Existing i18n files (`en.lproj`, `zh-Hans.lproj`)
- Existing config persistence layer
- Existing UniFFI bindings

**Deliverables:**
- Language selector dropdown in General Settings
- Config persistence for language preference
- Restart alert on language change
- Full localization support for new strings
