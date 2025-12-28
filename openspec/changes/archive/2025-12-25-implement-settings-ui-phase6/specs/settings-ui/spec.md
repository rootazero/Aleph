# Settings UI Specification

## ADDED Requirements

### Requirement: Provider Configuration UI
The Settings UI **SHALL** provide a fully functional interface for managing AI provider credentials and settings.

#### Scenario: Add new provider
- **WHEN** user clicks "Configure" button for OpenAI provider
- **THEN** a modal dialog opens with form fields: Provider name, API key (masked input), Model, Base URL, Color picker
- **AND** user enters API key "sk-test123"
- **WHEN** user clicks "Save"
- **THEN** API key is stored in macOS Keychain under service "com.aether.openai"
- **AND** config.toml is updated with provider metadata (model, base_url, color)
- **AND** API key field in config.toml shows reference: `api_key = "keychain:com.aether.openai"`
- **AND** provider list updates to show "✓ Configured" status

#### Scenario: Test provider connection
- **WHEN** user clicks "Test Connection" for configured OpenAI provider
- **THEN** Settings UI calls `core.testProviderConnection("openai")` asynchronously
- **AND** loading spinner appears on button
- **WHEN** API responds successfully
- **THEN** toast notification shows "OpenAI connection successful"
- **AND** last tested timestamp updates to current time
- **WHEN** API returns 401 Unauthorized
- **THEN** error toast shows "Authentication failed. Check API key."

#### Scenario: Delete provider
- **WHEN** user clicks "Delete" button for OpenAI provider
- **THEN** confirmation alert appears: "Delete OpenAI configuration?"
- **WHEN** user confirms
- **THEN** provider entry is removed from config.toml
- **AND** API key is deleted from Keychain
- **AND** provider list updates to show "⚠ Not Configured" status

---

### Requirement: Routing Rules Editor
The Settings UI **SHALL** provide a visual editor for creating, editing, and reordering routing rules.

#### Scenario: Create new routing rule
- **WHEN** user clicks "Add Rule" button in Routing tab
- **THEN** RuleEditorView modal opens with empty form
- **AND** user enters pattern: "^/draw"
- **AND** user selects provider: "OpenAI" from dropdown
- **AND** user enters system prompt: "You are DALL-E. Generate images."
- **WHEN** user clicks "Save"
- **THEN** `core.validateRegex("^/draw")` is called
- **AND** validation passes
- **AND** new rule is appended to config.toml `[[rules]]` array
- **AND** routing list updates to display new rule

#### Scenario: Edit existing rule
- **WHEN** user clicks "Edit" button on rule "/code → Claude"
- **THEN** RuleEditorView modal opens pre-filled with existing values
- **AND** user changes pattern to "^/rust|^/python"
- **WHEN** user clicks "Save"
- **THEN** pattern is validated
- **AND** rule is updated in config.toml
- **AND** routing list reflects updated pattern

#### Scenario: Reorder rules via drag-and-drop
- **GIVEN** routing list has 3 rules: [/draw, /code, .*]
- **WHEN** user drags "/code" rule above "/draw" rule
- **AND** drops it
- **THEN** rules array in config.toml is reordered: [/code, /draw, .*]
- **AND** list view updates to match new order
- **AND** subsequent routing matches use new priority order

#### Scenario: Validate invalid regex pattern
- **WHEN** user enters pattern: "^/draw(" (missing closing paren)
- **THEN** `core.validateRegex("^/draw(")` returns error
- **AND** error message appears below pattern field: "Invalid regex: Unclosed group"
- **AND** "Save" button is disabled until pattern is fixed

#### Scenario: Test pattern against input
- **GIVEN** RuleEditorView is open with pattern "^/draw"
- **WHEN** user enters test input "/draw a cat" in pattern tester
- **AND** clicks "Test" button
- **THEN** UI highlights match result: "✓ Match" (green)
- **WHEN** user enters test input "hello world"
- **AND** clicks "Test" button
- **THEN** UI shows "✗ No Match" (red)

---

### Requirement: Hotkey Customization
The Settings UI **SHALL** allow users to customize the global hotkey with visual key recording.

#### Scenario: Record new hotkey
- **WHEN** user clicks "Change Hotkey" button in Shortcuts tab
- **THEN** HotkeyRecorderView appears with text "Press key combination..."
- **AND** recorder enters listening mode
- **WHEN** user presses Cmd+Shift+A
- **THEN** recorder captures key event
- **AND** displays "⌘ + Shift + A" in UI
- **WHEN** user clicks "Save"
- **THEN** `core.updateShortcuts(shortcuts)` is called with new combo
- **AND** config.toml `[shortcuts]` section updates: `summon = "Command+Shift+A"`
- **AND** Rust core re-registers global hotkey listener with new combo

#### Scenario: Detect hotkey conflict
- **WHEN** user records hotkey Cmd+Space (macOS Spotlight default)
- **THEN** conflict detection runs
- **AND** warning alert appears: "This hotkey conflicts with Spotlight. Continue anyway?"
- **WHEN** user clicks "Cancel"
- **THEN** hotkey is not saved
- **WHEN** user clicks "Continue"
- **THEN** hotkey is saved despite conflict

#### Scenario: Reset to default hotkey
- **GIVEN** user has customized hotkey to Cmd+Shift+A
- **WHEN** user clicks "Reset to Default" button
- **THEN** hotkey reverts to "Command+Grave" (Cmd+~)
- **AND** config.toml updates
- **AND** Rust core re-registers hotkey listener

---

### Requirement: Behavior Configuration
The Settings UI **SHALL** provide controls for input mode, output mode, typing speed, and PII scrubbing.

#### Scenario: Change input mode
- **GIVEN** Behavior tab is open
- **WHEN** user selects "Copy" radio button (default is "Cut")
- **THEN** `core.updateBehavior(behavior)` is called
- **AND** config.toml updates: `input_mode = "copy"`
- **AND** subsequent hotkey invocations use Cmd+C instead of Cmd+X

#### Scenario: Adjust typing speed
- **WHEN** user moves typing speed slider to 100 chars/sec
- **THEN** slider value updates in real-time
- **AND** config.toml updates: `typing_speed = 100`
- **WHEN** user clicks "Preview" button
- **THEN** modal opens and types sample text at 100 chars/sec
- **AND** user can observe typing animation

#### Scenario: Enable PII scrubbing
- **WHEN** user toggles "Enable PII Scrubbing" ON
- **THEN** checkboxes for scrubbing types become enabled: Email, Phone, SSN, Credit Card
- **AND** user checks "Email" and "Phone"
- **WHEN** user saves
- **THEN** config.toml updates: `pii_scrubbing = { enabled = true, types = ["email", "phone"] }`
- **AND** subsequent AI requests have emails/phones redacted before sending

---

### Requirement: Config Hot-Reload
The Settings UI **SHALL** automatically reload when config.toml is modified externally.

#### Scenario: External config.toml edit detected
- **GIVEN** Settings window is open displaying current config
- **AND** Rust core file watcher is running
- **WHEN** user edits config.toml in external text editor
- **AND** saves file
- **THEN** Rust watcher detects change within 500ms
- **AND** Rust calls `handler.onConfigChanged(new_config)` callback
- **AND** Swift receives notification on main queue
- **AND** Settings UI re-reads config from core
- **AND** all tabs update to reflect new values
- **AND** toast notification appears: "Settings updated from file"

#### Scenario: Handle concurrent config modification
- **GIVEN** Settings window is saving config
- **WHEN** external editor saves config.toml at same time
- **THEN** Rust atomic write prevents corruption (write to temp → rename)
- **AND** last write wins
- **AND** Settings UI reloads to reflect final state

---

### Requirement: Config Validation
The Settings UI **SHALL** validate all configuration inputs before saving to prevent invalid states.

#### Scenario: Validate regex patterns
- **WHEN** user enters routing rule pattern: "[invalid"
- **THEN** `core.validateRegex("[invalid")` is called
- **AND** returns error: "Unclosed character class"
- **AND** error message displays below pattern field
- **AND** "Save" button is disabled

#### Scenario: Validate provider names
- **WHEN** user attempts to create provider with name "InvalidProvider"
- **AND** provider is not in allowed list: [openai, claude, gemini, ollama]
- **THEN** validation error appears: "Provider name must be one of: openai, claude, gemini, ollama"

#### Scenario: Validate hotkey format
- **WHEN** user records hotkey with invalid modifier combo (e.g., only Shift key)
- **THEN** validation error appears: "Hotkey must include Command or Control modifier"

---

### Requirement: Keychain Integration
The Settings UI **SHALL** store API keys securely in macOS Keychain, not in plain text config files.

#### Scenario: Save API key to Keychain
- **WHEN** user saves OpenAI API key "sk-test123" via ProviderConfigView
- **THEN** Swift calls `Security.SecAddGenericPassword()` with:
  - Service: "com.aether.openai"
  - Account: "api_key"
  - Password: "sk-test123"
- **AND** Keychain stores encrypted key
- **AND** config.toml stores reference: `api_key = "keychain:com.aether.openai"`
- **NOT** plain text key in config.toml

#### Scenario: Load API key from Keychain
- **WHEN** Rust provider needs API key for OpenAI
- **AND** config.toml has `api_key = "keychain:com.aether.openai"`
- **THEN** Rust calls Swift FFI: `loadAPIKey("openai")`
- **AND** Swift calls `Security.SecCopyItemMatching()` to retrieve key
- **AND** returns decrypted key "sk-test123" to Rust

#### Scenario: Delete API key from Keychain
- **WHEN** user deletes OpenAI provider configuration
- **THEN** Swift calls `Security.SecDeleteItemMatching()` for service "com.aether.openai"
- **AND** Keychain entry is removed
- **AND** config.toml entry is removed

---

### Requirement: Settings Window Management
The Settings UI **SHALL** be accessible via menu bar with proper window lifecycle management.

#### Scenario: Open settings window
- **WHEN** user clicks "Settings..." in menu bar
- **THEN** Settings window opens at size 800x550
- **AND** General tab is selected by default
- **AND** window is centered on screen
- **WHEN** user clicks "Settings..." again while window is open
- **THEN** existing window is brought to front (not duplicated)

#### Scenario: Close settings window
- **WHEN** user clicks window close button (red X)
- **THEN** window closes
- **AND** all unsaved changes are lost (no auto-save on close)
- **AND** window can be reopened later

#### Scenario: Resize settings window
- **WHEN** user drags window corner to resize
- **THEN** window can be resized within bounds: min 700x500, max unlimited
- **AND** UI layout adapts responsively (no clipping or overlap)
