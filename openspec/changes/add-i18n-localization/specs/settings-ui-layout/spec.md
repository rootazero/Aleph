## MODIFIED Requirements

### Requirement: Localized UI Text for All Settings Components
All user-facing text in the Settings UI SHALL be localized using Apple's native localization infrastructure.

#### Scenario: Settings tab navigation labels
- **WHEN** rendering sidebar navigation tabs
- **THEN** all tab labels SHALL use `LocalizedStringKey`
- **AND** keys include:
  - `settings.general.title` → "General"
  - `settings.providers.title` → "Providers"
  - `settings.routing.title` → "Routing"
  - `settings.shortcuts.title` → "Shortcuts"
  - `settings.behavior.title` → "Behavior"
  - `settings.memory.title` → "Memory"

#### Scenario: Form field labels and placeholders
- **WHEN** displaying form fields in provider configuration
- **THEN** all labels and placeholders SHALL be localized
- **AND** examples include:
  - Label: `Text("provider.field.api_key")` → "API Key"
  - Placeholder: `.placeholder(Text("provider.placeholder.model"))` → "e.g., gpt-4o"
  - Help text: `.help("provider.help.base_url")` → "Optional custom API endpoint"

#### Scenario: Button labels
- **WHEN** rendering action buttons (Save, Cancel, Test Connection)
- **THEN** button titles SHALL use common localization keys
- **AND** keys include:
  - `common.save` → "Save"
  - `common.cancel` → "Cancel"
  - `common.test_connection` → "Test Connection"
  - `common.add` → "Add"
  - `common.delete` → "Delete"

#### Scenario: Section headers
- **WHEN** displaying section headers in forms
- **THEN** section titles SHALL be localized
- **AND** examples include:
  - `provider.section.basic_settings` → "Basic Settings"
  - `provider.section.generation_params` → "Generation Parameters"
  - `provider.section.advanced_settings` → "Advanced Settings"

#### Scenario: Error messages in settings UI
- **WHEN** validation errors occur (e.g., invalid API key)
- **THEN** error messages SHALL be localized
- **AND** use keys like:
  - `error.provider.invalid_api_key` → "Invalid API key. Please check your credentials."
  - `error.routing.invalid_regex` → "Invalid regex pattern."

### Requirement: Dynamic Language Updates in Settings Window
Settings window SHALL display in the system's current language, with fallback to English for unsupported languages.

#### Scenario: User opens settings with English system language
- **WHEN** user clicks "Settings..." menu item
- **AND** system language is English
- **THEN** all settings UI text SHALL display in English
- **AND** strings loaded from `en.lproj/Localizable.strings`

#### Scenario: User opens settings with Simplified Chinese system language
- **WHEN** user clicks "Settings..." menu item
- **AND** system language is Simplified Chinese
- **THEN** all settings UI text SHALL display in Simplified Chinese
- **AND** strings loaded from `zh-Hans.lproj/Localizable.strings`

#### Scenario: User opens settings with unsupported language
- **WHEN** user clicks "Settings..." menu item
- **AND** system language is not in supported list (e.g., Japanese)
- **THEN** all settings UI text SHALL fallback to English
- **AND** strings loaded from `en.lproj/Localizable.strings`

### Requirement: Layout Compatibility with Localized Text
Settings UI layouts SHALL accommodate varying text lengths across different languages without truncation or overflow.

#### Scenario: Short language (English) vs long language (Chinese)
- **WHEN** rendering UI in Simplified Chinese (typically longer than English)
- **THEN** text fields and labels SHALL expand to fit content
- **AND** buttons SHALL not truncate text
- **AND** form layouts SHALL maintain readability

#### Scenario: Long button labels
- **WHEN** button label is longer in translation (e.g., "Test Connection" → "测试连接")
- **THEN** button width SHALL auto-adjust to fit text
- **AND** maintain minimum padding of 16 points horizontally

#### Scenario: Multi-line help text
- **WHEN** help text is longer in translation
- **THEN** text SHALL wrap to multiple lines
- **AND** use `fixedSize(horizontal: false, vertical: true)` modifier
- **AND** maintain paragraph spacing for readability

### Requirement: Localization Key Comments for Context
All localization keys SHALL include comment context for translators to understand usage.

#### Scenario: Ambiguous English words
- **WHEN** defining localization key for word with multiple meanings (e.g., "Save")
- **THEN** include comment explaining context
- **AND** example:
  ```swift
  Text("common.save")
      // NSLocalizedString("common.save", comment: "Button to save changes")
  ```

#### Scenario: Technical terms
- **WHEN** defining keys for technical terms (Provider, Routing, Hotkey)
- **THEN** include comment explaining technical context
- **AND** example:
  ```swift
  Text("settings.providers.title")
      // NSLocalizedString("settings.providers.title", comment: "Tab for managing AI service providers")
  ```

#### Scenario: Placeholder examples
- **WHEN** defining placeholder text with example values
- **THEN** comment SHALL clarify that it's an example
- **AND** example:
  ```swift
  .placeholder(Text("provider.placeholder.model"))
      // NSLocalizedString("provider.placeholder.model", comment: "Example model name placeholder, e.g., gpt-4o")
  ```
