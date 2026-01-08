# Skills Settings UI Specification

This specification defines the Skills management interface in Aether's Settings UI.

## ADDED Requirements

### Requirement: Skills Settings Tab

The system SHALL provide a dedicated Skills tab in the Settings interface.

#### Scenario: Skills tab in sidebar

- **GIVEN** the Settings window is open
- **WHEN** the user views the sidebar
- **THEN** a "Skills" tab SHALL be visible
- **AND** it SHALL appear after the "Search" tab

#### Scenario: Skills tab content

- **GIVEN** the user clicks on the Skills tab
- **WHEN** the content area updates
- **THEN** the SkillsSettingsView SHALL be displayed

---

### Requirement: Skills List Display

The system SHALL display all installed Skills in a list format.

#### Scenario: Display installed skills

- **GIVEN** skills exist in `~/.config/aether/skills/`
- **WHEN** the Skills tab is viewed
- **THEN** each skill SHALL be displayed as a SkillCard
- **AND** show the skill name and description

#### Scenario: Empty state

- **GIVEN** no skills are installed
- **WHEN** the Skills tab is viewed
- **THEN** an empty state view SHALL be displayed
- **AND** offer an "Install Skills" button

#### Scenario: Search filter

- **GIVEN** multiple skills are installed
- **WHEN** the user types in the search bar
- **THEN** the list SHALL filter to show only matching skills
- **AND** match against name and description

#### Scenario: Loading state

- **GIVEN** skills are being loaded
- **WHEN** the Skills tab is viewed
- **THEN** a loading indicator SHALL be displayed

---

### Requirement: Skill Card Component

The system SHALL display each skill in a card format.

#### Scenario: Card content

- **GIVEN** a skill is displayed
- **THEN** the card SHALL show skill icon, name, and description
- **AND** description SHALL be limited to 2 lines

#### Scenario: Hover actions

- **GIVEN** a skill card is hovered
- **WHEN** the mouse is over the card
- **THEN** Edit and Delete buttons SHALL appear

#### Scenario: Skill-specific icons

- **GIVEN** a skill with id "refine-text"
- **WHEN** the card is displayed
- **THEN** a text-related icon SHALL be shown (e.g., "text.quote")

---

### Requirement: Install Official Skills

The system SHALL provide one-click installation of official Claude Skills.

#### Scenario: Official skills button

- **GIVEN** the Skills tab is viewed
- **WHEN** the user clicks "Install Official Skills"
- **THEN** the system SHALL download from `anthropics/skills` repository
- **AND** extract and install valid skills

#### Scenario: Installation progress

- **GIVEN** official skills are being installed
- **WHEN** the download is in progress
- **THEN** a loading indicator SHALL be shown
- **AND** the button SHALL be disabled

#### Scenario: Installation result

- **GIVEN** official skills installation completes
- **WHEN** skills are installed successfully
- **THEN** a Toast notification SHALL show installed count
- **AND** the skills list SHALL refresh

---

### Requirement: Install from URL

The system SHALL support installing skills from GitHub URLs.

#### Scenario: URL input sheet

- **GIVEN** the user clicks "Install from URL"
- **WHEN** the install sheet opens
- **THEN** a URL input field SHALL be displayed

#### Scenario: Valid URL formats

- **GIVEN** a URL is entered
- **WHEN** the URL format is:
  - `https://github.com/user/repo`
  - `github.com/user/repo`
  - `user/repo`
- **THEN** the system SHALL normalize and accept the URL

#### Scenario: URL installation

- **GIVEN** a valid GitHub URL is submitted
- **WHEN** the install button is clicked
- **THEN** the system SHALL download the repository ZIP
- **AND** extract and install valid skills

---

### Requirement: Upload ZIP File

The system SHALL support installing skills from local ZIP files.

#### Scenario: Upload button

- **GIVEN** the user clicks "Upload ZIP"
- **WHEN** the file picker opens
- **THEN** only `.zip` files SHALL be selectable

#### Scenario: ZIP installation

- **GIVEN** a ZIP file is selected
- **WHEN** the file is processed
- **THEN** the system SHALL extract SKILL.md files
- **AND** install valid skills to the skills directory

#### Scenario: Invalid ZIP

- **GIVEN** a ZIP file with no SKILL.md files
- **WHEN** the file is processed
- **THEN** an error message SHALL be displayed

---

### Requirement: Create New Skill

The system SHALL provide a skill creation interface.

#### Scenario: Create button

- **GIVEN** the Skills tab is viewed
- **WHEN** the user clicks "Create"
- **THEN** the SkillEditorPanel SHALL open as a sheet

#### Scenario: Editor fields

- **GIVEN** the skill editor is open for creation
- **THEN** the following fields SHALL be available:
  - Name (required, editable)
  - Description (required, editable)
  - Instructions (Markdown editor)

#### Scenario: Name validation

- **GIVEN** the user enters a skill name
- **WHEN** the name contains invalid characters
- **THEN** the Save button SHALL be disabled
- **AND** validation error SHALL be shown

#### Scenario: Save new skill

- **GIVEN** all required fields are filled
- **WHEN** the user clicks Save
- **THEN** a SKILL.md file SHALL be generated
- **AND** saved to `~/.config/aether/skills/<name>/SKILL.md`
- **AND** the skills list SHALL refresh

---

### Requirement: Edit Existing Skill

The system SHALL provide skill editing functionality.

#### Scenario: Edit button

- **GIVEN** a skill card is hovered
- **WHEN** the user clicks Edit
- **THEN** the SkillEditorPanel SHALL open with skill data

#### Scenario: Editor pre-populated

- **GIVEN** the editor opens for an existing skill
- **THEN** name, description, and instructions SHALL be pre-filled
- **AND** the name field SHALL be read-only

#### Scenario: Save changes

- **GIVEN** changes are made to an existing skill
- **WHEN** the user clicks Save
- **THEN** the SKILL.md file SHALL be updated
- **AND** the skills list SHALL refresh

---

### Requirement: Delete Skill

The system SHALL provide skill deletion with confirmation.

#### Scenario: Delete button

- **GIVEN** a skill card is hovered
- **WHEN** the user clicks Delete
- **THEN** a confirmation dialog SHALL appear

#### Scenario: Confirm deletion

- **GIVEN** the confirmation dialog is shown
- **WHEN** the user confirms deletion
- **THEN** the skill directory SHALL be removed
- **AND** the skills list SHALL refresh

#### Scenario: Cancel deletion

- **GIVEN** the confirmation dialog is shown
- **WHEN** the user clicks Cancel
- **THEN** the skill SHALL NOT be deleted

---

### Requirement: Skill Editor Preview

The system SHALL provide Markdown preview in the editor.

#### Scenario: Preview toggle

- **GIVEN** the skill editor is open
- **WHEN** the user toggles preview mode
- **THEN** the instructions SHALL render as Markdown

#### Scenario: Raw mode

- **GIVEN** preview mode is off
- **WHEN** viewing instructions
- **THEN** raw Markdown text SHALL be shown in editor

---

### Requirement: Error Handling

The system SHALL display user-friendly error messages.

#### Scenario: Network error

- **GIVEN** the user attempts to install from URL
- **WHEN** the network request fails
- **THEN** an error message SHALL be displayed

#### Scenario: Invalid skill format

- **GIVEN** a skill with invalid SKILL.md format
- **WHEN** parsing fails
- **THEN** the skill SHALL be skipped
- **AND** a warning SHALL be logged

---

## Cross-References

- **skills-capability**: Core skills functionality (registry, installer)
- **settings-ui-layout**: Settings UI architecture
- Existing UI components: `Aether/Sources/Components/`
