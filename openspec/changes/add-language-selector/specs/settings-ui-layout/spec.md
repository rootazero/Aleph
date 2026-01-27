# settings-ui-layout Spec Delta

## ADDED Requirements

### Requirement: Language Selection in General Settings
The General Settings tab SHALL provide a language selector to allow users to override the system language preference.

#### Scenario: Language dropdown renders correctly
- **GIVEN** the user opens the Settings window
- **WHEN** the user navigates to the General tab
- **THEN** a "Language" section SHALL be visible
- **AND** the section SHALL contain a dropdown picker labeled "Preferred Language"
- **AND** the dropdown SHALL display the following options:
  - "System Default" (default selection if no override is configured)
  - "English"
  - "简体中文" (Simplified Chinese)
- **AND** future language additions SHALL automatically appear in this list

#### Scenario: Current language preference is loaded on view appear
- **GIVEN** the user has previously selected a language preference
- **WHEN** the General Settings view appears
- **THEN** the dropdown SHALL display the currently configured language
- **AND** if no language is configured, the dropdown SHALL display "System Default"
- **AND** the selection SHALL match the value stored in `config.toml`

#### Scenario: Selecting a language shows restart alert
- **GIVEN** the user is viewing the General Settings tab
- **WHEN** the user selects a language from the dropdown (other than current selection)
- **THEN** the new selection SHALL be saved to `config.toml` immediately
- **AND** an alert dialog SHALL appear with the title "Restart Required"
- **AND** the alert message SHALL read: "Language will change after restarting Aether. Restart now?"
- **AND** the alert SHALL have two buttons:
  - "Restart Now" - terminates the app immediately (`NSApp.terminate(nil)`)
  - "Later" - dismisses the alert and keeps the app running

#### Scenario: Language preference persists across sessions
- **GIVEN** the user has selected a specific language
- **WHEN** the user restarts Aether
- **THEN** the app SHALL launch with the selected language applied to all UI elements
- **AND** the Settings dropdown SHALL show the selected language as active
- **AND** the `~/.aether/config.toml` file SHALL contain the language setting in the `[general]` section

#### Scenario: Selecting "System Default" removes language override
- **GIVEN** the user has previously selected a specific language
- **WHEN** the user selects "System Default" from the dropdown
- **THEN** the language override SHALL be removed from `config.toml`
- **AND** after restart, Aether SHALL follow the macOS system language setting
- **AND** the restart alert SHALL still be shown (since language may change)

### Requirement: Language Dropdown Positioning and Styling
The language selector SHALL follow the existing design system and layout conventions of the General Settings tab.

#### Scenario: Section placement in General Settings
- **GIVEN** the General Settings tab is displayed
- **WHEN** the view is rendered
- **THEN** the Language section SHALL appear after the "Sound" section
- **AND** the Language section SHALL appear before the "Updates" section
- **AND** the section SHALL use the same header style as other sections
- **AND** the section SHALL use `Form` and `Section` components from SwiftUI

#### Scenario: Dropdown styling consistency
- **GIVEN** the language dropdown is rendered
- **WHEN** the user views the picker
- **THEN** the picker SHALL use the native macOS `Picker` style (`.menu`)
- **AND** the picker SHALL have a label "Preferred Language" displayed to the left
- **AND** the current selection SHALL be displayed to the right (standard macOS behavior)
- **AND** the dropdown options SHALL be localized (e.g., "System Default" shows as "系统默认" in Chinese)

## MODIFIED Requirements

_No existing requirements are modified by this change._

## REMOVED Requirements

_No existing requirements are removed by this change._
