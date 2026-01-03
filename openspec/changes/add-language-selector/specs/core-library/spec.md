# core-library Spec Delta

## ADDED Requirements

### Requirement: Language Preference Configuration
The `GeneralConfig` struct SHALL support storing and validating a user's preferred language override.

#### Scenario: Language field in GeneralConfig
- **GIVEN** the `GeneralConfig` struct in `Aether/core/src/config/mod.rs`
- **WHEN** the struct is serialized or deserialized
- **THEN** the struct SHALL have an optional `language: Option<String>` field
- **AND** the field SHALL be annotated with `#[serde(default, skip_serializing_if = "Option::is_none")]`
- **AND** the field SHALL represent the preferred language code (e.g., `"en"`, `"zh_CN"`)
- **AND** if the field is `None`, the system SHALL use the macOS system language

#### Scenario: Valid language codes
- **GIVEN** a `language` value is set in `config.toml`
- **WHEN** the config is loaded
- **THEN** the system SHALL accept the following language codes:
  - `"en"` - English
  - `"zh_CN"` - Simplified Chinese (China)
  - `"zh_TW"` - Traditional Chinese (Taiwan, future)
  - `"zh_HK"` - Traditional Chinese (Hong Kong, future)
  - Any future language codes matching existing `.lproj` directories
- **AND** the system SHALL log a warning if an unrecognized language code is detected
- **AND** the system SHALL fall back to system language if the language code is invalid

#### Scenario: Language preference persisted to config.toml
- **GIVEN** the user selects a language in the Settings UI
- **WHEN** the config is saved
- **THEN** the `config.toml` file SHALL contain a `language` field under `[general]`
- **AND** the field SHALL be formatted as: `language = "en"` or `language = "zh_CN"`
- **AND** if "System Default" is selected, the `language` field SHALL be omitted from the file

#### Scenario: Language override applied on app launch
- **GIVEN** the app is launching
- **WHEN** the config is loaded and `language` is set to a valid value
- **THEN** the app SHALL apply the language override by setting `UserDefaults.standard.set([language], forKey: "AppleLanguages")`
- **AND** the app SHALL call `UserDefaults.standard.synchronize()`
- **AND** all UI elements SHALL render in the specified language

#### Scenario: System language used when no override is set
- **GIVEN** the app is launching
- **WHEN** the config is loaded and `language` is `None`
- **THEN** the app SHALL remove any language override by calling `UserDefaults.standard.removeObject(forKey: "AppleLanguages")`
- **AND** the app SHALL follow the macOS system language setting
- **AND** all UI elements SHALL render in the system language

### Requirement: Language Configuration Validation
The config loading logic SHALL validate language preferences to prevent errors from invalid configurations.

#### Scenario: Invalid language code in config.toml
- **GIVEN** `config.toml` contains `language = "invalid-code"`
- **WHEN** the config is loaded
- **THEN** the system SHALL log a warning: "Invalid language code 'invalid-code', falling back to system language"
- **AND** the system SHALL treat the language as `None` (system default)
- **AND** the app SHALL launch successfully without crashing

#### Scenario: Missing .lproj directory for configured language
- **GIVEN** `config.toml` contains `language = "ja"` (Japanese)
- **AND** the `Aether/Resources/ja.lproj` directory does not exist
- **WHEN** the config is loaded
- **THEN** the system SHALL log a warning: "Localization files for 'ja' not found, falling back to system language"
- **AND** the system SHALL treat the language as `None`
- **AND** the app SHALL launch successfully without crashing

## MODIFIED Requirements

_No existing requirements are modified by this change._

## REMOVED Requirements

_No existing requirements are removed by this change._
