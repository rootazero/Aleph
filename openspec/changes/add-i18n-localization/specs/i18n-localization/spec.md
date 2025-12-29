## ADDED Requirements

### Requirement: System Language Detection with Fallback
The system SHALL detect the user's system language preference and load matching localization strings, falling back to English if the language is not supported.

#### Scenario: User has Simplified Chinese system language
- **WHEN** app launches with macOS system language set to Simplified Chinese
- **THEN** the system loads strings from `zh-Hans.lproj/Localizable.strings`
- **AND** all UI text displays in Simplified Chinese
- **AND** code comments remain in English

#### Scenario: User has unsupported language (e.g., Japanese)
- **WHEN** app launches with macOS system language set to Japanese
- **THEN** the system cannot find matching `.lproj` directory
- **AND** falls back to English base strings from `en.lproj/Localizable.strings`
- **AND** all UI text displays in English

#### Scenario: User has English system language
- **WHEN** app launches with macOS system language set to English
- **THEN** the system loads strings from `en.lproj/Localizable.strings`
- **AND** all UI text displays in English (base language)

#### Scenario: Partial translation available
- **WHEN** a translation file exists but is missing specific keys
- **THEN** the system uses the English string for missing keys
- **AND** logs a warning for missing translation keys (debug builds only)

### Requirement: Localization String Key Naming Convention
All localization string keys SHALL follow a hierarchical naming convention for maintainability and discoverability.

#### Scenario: Settings view string keys
- **WHEN** defining strings for settings tabs
- **THEN** keys SHALL use format `settings.<tab>.<element>`
- **AND** examples include:
  - `settings.general.title` → "General"
  - `settings.providers.add_button` → "Add Custom Provider"
  - `settings.routing.rule_editor_title` → "Edit Routing Rule"

#### Scenario: Common UI element string keys
- **WHEN** defining reusable UI strings (buttons, labels)
- **THEN** keys SHALL use format `common.<element>`
- **AND** examples include:
  - `common.save` → "Save"
  - `common.cancel` → "Cancel"
  - `common.test_connection` → "Test Connection"
  - `common.ok` → "OK"

#### Scenario: Error message string keys
- **WHEN** defining error or alert messages
- **THEN** keys SHALL use format `error.<context>.<specific_error>`
- **AND** examples include:
  - `error.provider.invalid_api_key` → "Invalid API key. Please check your credentials."
  - `error.permission.accessibility_denied` → "Accessibility permission denied."

#### Scenario: Help text and placeholder string keys
- **WHEN** defining help text or form placeholders
- **THEN** keys SHALL use format `<context>.help.<field>` or `<context>.placeholder.<field>`
- **AND** examples include:
  - `provider.help.base_url` → "Optional custom API endpoint"
  - `provider.placeholder.model` → "e.g., gpt-4o"

### Requirement: SwiftUI Integration with LocalizedStringKey
All SwiftUI Text, Label, and Button components SHALL use localized strings via `NSLocalizedString()` or `LocalizedStringKey`.

#### Scenario: Text component localization
- **WHEN** displaying static text in SwiftUI view
- **THEN** use `Text("settings.general.title")` with LocalizedStringKey
- **AND** Xcode automatically resolves translation from `Localizable.strings`
- **AND** no manual `NSLocalizedString()` wrapper needed for SwiftUI Text

#### Scenario: Button title localization
- **WHEN** creating button with text label
- **THEN** use `Button("common.save")` with LocalizedStringKey
- **AND** button title updates based on system language

#### Scenario: Dynamic string interpolation
- **WHEN** localized string contains placeholder (e.g., "Version: %@")
- **THEN** use `String.localizedStringWithFormat()` for interpolation
- **AND** example: `String.localizedStringWithFormat(NSLocalizedString("settings.version_info", comment: ""), appVersion)`

#### Scenario: Accessibility labels
- **WHEN** providing accessibility hints or labels
- **THEN** use `.accessibilityLabel(Text("accessibility.close_button"))`
- **AND** ensure screen reader announces localized text

### Requirement: Xcode Project Localization Configuration
The Xcode project SHALL be configured to support multiple localizations through XcodeGen and Info.plist settings.

#### Scenario: Base localization (English)
- **WHEN** project is built
- **THEN** English SHALL be the base/development language
- **AND** `en.lproj/Localizable.strings` SHALL be the source of truth for all keys
- **AND** `CFBundleDevelopmentRegion` in Info.plist SHALL be "en"

#### Scenario: Add Simplified Chinese localization
- **WHEN** project includes Chinese translation
- **THEN** `zh-Hans.lproj/Localizable.strings` SHALL exist with full translation
- **AND** `CFBundleLocalizations` array in Info.plist SHALL include ["en", "zh-Hans"]

#### Scenario: XcodeGen configuration
- **WHEN** running `xcodegen generate`
- **THEN** `project.yml` SHALL include localization resource configuration
- **AND** `.lproj` directories SHALL be recognized as variant groups
- **AND** `Localizable.strings` files SHALL be copied to app bundle

#### Scenario: InfoPlist.strings localization
- **WHEN** localizing app metadata (name, copyright)
- **THEN** each `.lproj` directory SHALL contain `InfoPlist.strings`
- **AND** keys include:
  - `CFBundleName` → "Aether" (no translation needed, but can be localized)
  - `NSHumanReadableCopyright` → Localized copyright notice
  - `NSAppleEventsUsageDescription` → Localized permission description

### Requirement: String Extraction and Validation Tooling
The project SHALL provide tooling to extract, validate, and maintain localization strings.

#### Scenario: Extract strings from Swift code
- **WHEN** developer runs string extraction script
- **THEN** the system scans all `.swift` files for `NSLocalizedString()` calls
- **AND** generates list of all localization keys used in code
- **AND** outputs missing keys compared to `en.lproj/Localizable.strings`

#### Scenario: Validate translation completeness
- **WHEN** developer runs validation script
- **THEN** the system compares all `.lproj/Localizable.strings` files
- **AND** reports missing keys in non-English translations
- **AND** reports keys in translations that are not in English base
- **AND** exits with error code if translations are incomplete (CI integration)

#### Scenario: Export strings for translators
- **WHEN** preparing for community translation
- **THEN** the system exports `Localizable.strings` to translator-friendly format (CSV or XLIFF)
- **AND** includes string keys, English text, and optional context comments
- **AND** can re-import translated CSV/XLIFF back to `.strings` format

#### Scenario: Prevent untranslated strings in release builds
- **WHEN** building release configuration
- **THEN** the system runs validation script as pre-build step
- **AND** fails build if any supported language has missing translations
- **AND** allows build to continue in debug configuration with warnings only

### Requirement: Code Comment Language Policy
All code comments, docstrings, and developer documentation SHALL remain in English for international collaboration.

#### Scenario: Swift code comments
- **WHEN** writing inline comments or function documentation
- **THEN** comments SHALL be written in English
- **AND** example:
  ```swift
  /// Displays the settings window with localized UI text
  /// - Parameter tab: The initial tab to show
  func showSettings(tab: SettingsTab) {
      // UI text will be localized, but this comment stays in English
  }
  ```

#### Scenario: Markdown documentation
- **WHEN** writing README files or technical docs
- **THEN** primary language SHALL be English
- **AND** optional translations MAY be provided as separate files (e.g., `README.zh-Hans.md`)

#### Scenario: Git commit messages
- **WHEN** committing localization changes
- **THEN** commit messages SHALL be in English
- **AND** example: "Add Simplified Chinese localization for settings UI"

### Requirement: Localized Permission Descriptions
macOS permission prompt descriptions SHALL be localized for each supported language.

#### Scenario: Accessibility permission description in English
- **WHEN** user is prompted for Accessibility permission
- **AND** system language is English
- **THEN** the prompt SHALL display:
  "Aether needs to simulate keyboard input to paste AI responses into your applications and monitor global hotkeys for seamless AI integration."

#### Scenario: Accessibility permission description in Simplified Chinese
- **WHEN** user is prompted for Accessibility permission
- **AND** system language is Simplified Chinese
- **THEN** the prompt SHALL display Chinese translation from `zh-Hans.lproj/InfoPlist.strings`
- **AND** key `NSAppleEventsUsageDescription` contains localized text

#### Scenario: Multiple permission descriptions
- **WHEN** app requests multiple permissions (Accessibility, Input Monitoring)
- **THEN** each permission description SHALL be localized independently
- **AND** keys include:
  - `NSAppleEventsUsageDescription` (Accessibility)
  - `NSAppleEventsUsageDescription` (Input Monitoring - shares same key in this case)

### Requirement: Dynamic Language Updates (Future Consideration)
The localization system SHALL be designed to support runtime language switching in future versions, though initial implementation requires app restart.

#### Scenario: Language change detection (Phase 1 - Current Implementation)
- **WHEN** user changes macOS system language
- **AND** restarts Aether app
- **THEN** the app SHALL load new language strings on next launch
- **AND** all UI updates to new language immediately

#### Scenario: Language change detection (Phase 2 - Future)
- **WHEN** user changes macOS system language
- **AND** app is already running
- **THEN** the system MAY detect language change via NotificationCenter
- **AND** MAY re-render all UI with new language strings
- **AND** MAY NOT require app restart (requires significant refactoring - deferred)

#### Scenario: Language preference override (Future)
- **WHEN** app provides in-app language selector (future feature)
- **THEN** user MAY override system language preference
- **AND** app SHALL persist user's language choice in UserDefaults
- **AND** load that language regardless of system setting

### Requirement: Translation Quality Guidelines
All translations SHALL maintain consistency in tone, terminology, and formatting with the base English text.

#### Scenario: Maintain formal/informal tone
- **WHEN** translating UI text to Simplified Chinese
- **THEN** use consistent formal second-person (您) throughout
- **AND** avoid mixing informal (你) and formal pronouns

#### Scenario: Technical term consistency
- **WHEN** translating technical terms (e.g., "Provider", "Hotkey", "Routing")
- **THEN** maintain consistent translation across all strings
- **AND** prefer widely-accepted technical terms over literal translations
- **AND** examples:
  - "Provider" → "提供商" (consistent across all uses)
  - "Hotkey" → "快捷键" (not "热键")
  - "Routing" → "路由" (technical term, not "路线")

#### Scenario: Button text conciseness
- **WHEN** translating button labels
- **THEN** keep translations concise to fit UI layout
- **AND** ensure translated text does not cause layout overflow
- **AND** test UI with longest supported language (often German, not in initial scope)

#### Scenario: Context preservation in translations
- **WHEN** English string has multiple meanings (e.g., "Save" as button vs "Save settings")
- **THEN** use string key comments to provide context for translators
- **AND** example:
  ```swift
  NSLocalizedString("common.save", comment: "Save button label")
  NSLocalizedString("settings.save_prompt", comment: "Prompt asking user to save changes")
  ```
