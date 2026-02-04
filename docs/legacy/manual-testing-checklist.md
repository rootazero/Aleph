# Manual Testing Checklist - Modernized Settings UI

**Document Version**: 2.0 (Updated for modernize-settings-ui change)
**Last Updated**: 2025-12-26

## Important Note

This document has been superseded by more comprehensive testing documentation. For complete testing coverage, please refer to:

1. **`modernize-settings-ui-testing-plan.md`** - Overall test plan with all categories
2. **`visual-testing-guide.md`** - Detailed visual testing (Light/Dark/Auto modes)
3. **`performance-testing-guide.md`** - Performance and Instruments testing
4. **`accessibility-testing-checklist.md`** - VoiceOver and accessibility testing

This checklist remains as a quick reference for core functionality testing.

---

## Quick Start Testing Guide

This checklist covers the most critical end-to-end tests for the modernized Settings UI. It focuses on user-facing features and basic functionality verification.

## Pre-Test Setup

- [ ] Build Rust core: `cd Aleph/core && cargo build`
- [ ] Generate Xcode project: `xcodegen generate`
- [ ] Build and run Aleph app: Open in Xcode and press Cmd+R
- [ ] Verify app launches without errors
- [ ] Verify menu bar icon appears

## 1. Provider Management

### 1.1 Add OpenAI Provider

- [ ] Open Settings → Providers tab
- [ ] Click "Configure" button for OpenAI
- [ ] Enter the following details:
  - Provider Name: `openai`
  - API Key: `sk-test-key-12345` (or real key)
  - Model: `gpt-4o`
  - Base URL: (leave empty for default)
  - Color: `#10a37f` (green)
  - Timeout: `30` seconds
  - Max Tokens: `4096`
  - Temperature: `0.7`
- [ ] Click "Save"
- [ ] Verify provider appears in list with "✓ Configured" status
- [ ] Verify API key is NOT visible in config.toml (only stored in Keychain)
  - Check: `cat ~/.aleph/config.toml | grep "sk-test-key"`
  - Expected: No matches (key should not be in file)

### 1.2 Test Provider Connection

- [ ] Click "Test Connection" button for OpenAI provider
- [ ] Verify loading spinner appears during test
- [ ] Expected outcomes:
  - **With valid API key**: Success toast: "✓ Connection successful"
  - **With invalid API key**: Error toast: "✗ Authentication failed"
- [ ] Verify last tested timestamp is updated

### 1.3 Add Claude Provider

- [ ] Click "Configure" for Claude
- [ ] Enter details:
  - Provider Name: `claude`
  - API Key: `sk-ant-test-key` (or real key)
  - Model: `claude-3-5-sonnet-20241022`
  - Color: `#d97757` (orange)
- [ ] Save and verify provider appears in list

### 1.4 Add Ollama Provider (Local)

- [ ] Click "Add Provider"
- [ ] Enter details:
  - Provider Name: `ollama`
  - Provider Type: `ollama`
  - Model: `llama3.2`
  - API Key: (leave empty - not required for local)
  - Base URL: `http://localhost:11434`
  - Color: `#0000ff` (blue)
- [ ] Save provider
- [ ] Verify no API key validation error (Ollama doesn't need API key)

### 1.5 Edit Provider

- [ ] Select OpenAI provider
- [ ] Click "Edit" button
- [ ] Change model to `gpt-4o-mini`
- [ ] Change color to `#FF0000` (red)
- [ ] Save
- [ ] Verify changes are reflected in list

### 1.6 Delete Provider

- [ ] Select a test provider
- [ ] Click "Delete" button
- [ ] Verify confirmation dialog appears: "Are you sure you want to delete this provider?"
- [ ] Click "Delete"
- [ ] Verify provider is removed from list
- [ ] Verify API key is removed from Keychain
  - Check: Open Keychain Access app → Search for "Aleph" → Verify key deleted

## 2. Routing Rules Editor

### 2.1 Create New Rule

- [ ] Open Settings → Routing tab
- [ ] Click "Add Rule" button
- [ ] Enter the following:
  - Regex Pattern: `^/draw`
  - Provider: `openai`
  - System Prompt: `You are DALL-E. Generate images based on user descriptions.`
- [ ] Verify regex validation shows green checkmark (valid pattern)
- [ ] Click "Save"
- [ ] Verify rule appears in list

### 2.2 Test Regex Pattern Validation

- [ ] Create new rule with invalid regex: `[invalid(`
- [ ] Verify error message appears: "Invalid regex pattern"
- [ ] Verify "Save" button is disabled
- [ ] Fix pattern to valid regex: `.*`
- [ ] Verify error disappears and "Save" button is enabled

### 2.3 Test Live Pattern Tester

- [ ] Open existing rule (e.g., `^/draw`)
- [ ] In "Test Pattern" field, enter: `/draw a sunset`
- [ ] Click "Test" button
- [ ] Verify result shows: "✓ Match" (pattern matches input)
- [ ] Enter non-matching input: `hello world`
- [ ] Verify result shows: "✗ No match"

### 2.4 Drag to Reorder Rules

- [ ] Create 3 rules with different priorities:
  1. `^/code` → claude
  2. `^/draw` → openai
  3. `.*` → openai (catch-all)
- [ ] Drag rule #2 (`^/draw`) to position #1
- [ ] Verify order changes in UI
- [ ] Verify config.toml reflects new order
  - Check: `cat ~/.aleph/config.toml`
  - Expected: Rules appear in dragged order

### 2.5 Edit Rule

- [ ] Select existing rule
- [ ] Click "Edit" button
- [ ] Change regex pattern to `^/test`
- [ ] Change system prompt
- [ ] Save
- [ ] Verify changes are reflected

### 2.6 Delete Rule

- [ ] Select a rule
- [ ] Click "Delete" button
- [ ] Verify confirmation dialog appears
- [ ] Confirm deletion
- [ ] Verify rule is removed from list

### 2.7 Import Rules

- [ ] Click "Import Rules" button
- [ ] Select a JSON file with rules (create test file first):
  ```json
  [
    {
      "regex": "^/translate",
      "provider": "claude",
      "system_prompt": "Translate the following text to English."
    }
  ]
  ```
- [ ] Choose import strategy: "Append" or "Replace"
- [ ] Verify rules are imported correctly

### 2.8 Export Rules

- [ ] Click "Export Rules" button
- [ ] Choose save location
- [ ] Verify exported JSON file contains all rules
- [ ] Verify JSON is valid (can be re-imported)

## 3. Hotkey Customization

### 3.1 Change Global Hotkey

- [ ] Open Settings → Shortcuts tab
- [ ] Current hotkey should show: `⌘ + ~`
- [ ] Click "Change" button for Summon hotkey
- [ ] Key recorder appears: "Press key combination..."
- [ ] Press: `Cmd + Shift + A`
- [ ] Verify recorder displays: `⌘ + ⇧ + A`
- [ ] Click "Save"
- [ ] Verify hotkey is updated in settings

### 3.2 Test New Hotkey

- [ ] Close Settings window
- [ ] Select some text in any app (e.g., "Hello world" in Notes)
- [ ] Press new hotkey: `Cmd + Shift + A`
- [ ] Verify Aleph responds (Halo appears)
- [ ] Expected: Text is cut and processed

### 3.3 Conflict Detection

- [ ] Try setting hotkey to system shortcut: `Cmd + C` (Copy)
- [ ] Verify warning appears: "⚠️ This hotkey may conflict with system shortcuts"
- [ ] Verify user can proceed anyway (with confirmation)
- [ ] Change to non-conflicting hotkey

### 3.4 Preset Shortcuts

- [ ] Click "Presets" dropdown
- [ ] Select preset: `Cmd + ~` (default)
- [ ] Verify hotkey is set to preset
- [ ] Select another preset: `Ctrl + Space`
- [ ] Verify hotkey changes

### 3.5 Reset to Default

- [ ] Change hotkey to custom value: `Cmd + Shift + X`
- [ ] Click "Reset to Default" button
- [ ] Verify hotkey resets to `Cmd + ~`

### 3.6 Cancel Hotkey Configuration

- [ ] Open Settings → Shortcuts tab
- [ ] Current Cancel hotkey: `Escape`
- [ ] Change to: `Cmd + .`
- [ ] Save
- [ ] Test: Start Aleph operation, press `Cmd + .`
- [ ] Verify operation is cancelled

## 4. Behavior Settings

### 4.1 Input Mode: Cut vs Copy

- [ ] Open Settings → Behavior tab
- [ ] Current mode: `Cut` (default)
- [ ] Select text in Notes: "Test input mode"
- [ ] Press hotkey
- [ ] Verify text is **removed** from Notes (cut behavior)
- [ ] Change mode to: `Copy`
- [ ] Select text again: "Test copy mode"
- [ ] Press hotkey
- [ ] Verify text **remains** in Notes (copy behavior)

### 4.2 Output Mode: Typewriter vs Instant

- [ ] Set Output Mode to: `Typewriter`
- [ ] Set Typing Speed: `50` chars/sec
- [ ] Process text: "What is the capital of France?"
- [ ] Verify response is typed character-by-character
- [ ] Observe typing animation (should take ~1-2 seconds for ~50 char response)
- [ ] Change mode to: `Instant`
- [ ] Process same text
- [ ] Verify response appears instantly (no typing animation)

### 4.3 Typing Speed Slider

- [ ] Set Output Mode to: `Typewriter`
- [ ] Adjust slider:
  - Min: `10` chars/sec (slow)
  - Default: `50` chars/sec
  - Max: `200` chars/sec (very fast)
- [ ] Click "Preview" button for each speed
- [ ] Observe sample text typing at different speeds
- [ ] Verify speed changes are saved

### 4.4 PII Scrubbing Configuration

- [ ] Enable toggle: "Enable PII Scrubbing"
- [ ] Select PII types to scrub:
  - [x] Email addresses
  - [x] Phone numbers
  - [ ] SSN (leave unchecked)
  - [x] Credit card numbers
- [ ] Test with text containing PII:
  ```
  Contact: john@example.com
  Phone: (555) 123-4567
  SSN: 123-45-6789
  Card: 4111-1111-1111-1111
  ```
- [ ] Expected result (after scrubbing):
  ```
  Contact: [EMAIL_REDACTED]
  Phone: [PHONE_REDACTED]
  SSN: 123-45-6789  (not scrubbed, checkbox unchecked)
  Card: [CARD_REDACTED]
  ```

### 4.5 Custom PII Regex Patterns

- [ ] Click "Advanced" to show custom regex editor
- [ ] Add custom pattern:
  - Name: `API Keys`
  - Pattern: `sk-[a-zA-Z0-9]{32,}`
- [ ] Save pattern
- [ ] Test with text: `My key is sk-proj-abc123xyz789...`
- [ ] Verify: `My key is [API_KEY_REDACTED]`

## 5. Config Persistence

### 5.1 Config Persists Across Restart

- [ ] Make the following changes:
  - Add OpenAI provider with API key
  - Create routing rule: `^/test → openai`
  - Change hotkey to `Cmd + Shift + A`
  - Set input mode to `Copy`
  - Set typing speed to `100`
- [ ] Quit Aleph app (Cmd + Q)
- [ ] Relaunch Aleph
- [ ] Open Settings
- [ ] Verify ALL settings persisted:
  - [x] OpenAI provider exists with correct config
  - [x] Routing rule exists
  - [x] Hotkey is `Cmd + Shift + A`
  - [x] Input mode is `Copy`
  - [x] Typing speed is `100`

### 5.2 Verify Keychain Persistence

- [ ] Check Keychain Access app
- [ ] Search for "Aleph"
- [ ] Verify API keys are stored with correct provider names:
  - `Aleph:openai` → `sk-test-key-12345`
  - `Aleph:claude` → `sk-ant-test-key`
- [ ] Verify keys are marked as "Application Password"
- [ ] Verify "Where" field shows: "Aleph"

## 6. Config Hot-Reload

### 6.1 External Config Edit Triggers Reload

- [ ] Open Settings window (keep it open)
- [ ] Open Terminal and edit config:
  ```bash
  nano ~/.aleph/config.toml
  ```
- [ ] Change `default_hotkey` from `Command+Grave` to `Command+Shift+B`
- [ ] Save file (Ctrl+O, Enter, Ctrl+X)
- [ ] Switch back to Aleph Settings window
- [ ] Verify toast notification appears: "⚡ Settings updated from file"
- [ ] Verify UI refreshes with new hotkey value
- [ ] Verify update happens within **1 second** of file save

### 6.2 Hot-Reload with Invalid Config

- [ ] Edit config.toml with invalid syntax:
  ```toml
  [providers.openai
  # Missing closing bracket
  ```
- [ ] Save file
- [ ] Verify error notification: "⚠️ Config file has errors, changes ignored"
- [ ] Verify app continues running with previous valid config

## 7. General Tab Updates

### 7.1 Version Display

- [ ] Open Settings → General tab
- [ ] Verify version displays dynamically from Info.plist
- [ ] Expected format: `Version 0.1.0 (Build 1)`

### 7.2 Check for Updates

- [ ] Click "Check for Updates" button
- [ ] Verify one of:
  - **Update available**: Dialog shows new version info
  - **Up to date**: Toast: "You're running the latest version"
  - **Error**: Toast: "Update check failed: [reason]"

### 7.3 Theme Selection

- [ ] Select theme:
  - Cyberpunk (default - neon colors)
  - Zen (minimal, monochrome)
  - Jarvis (Iron Man inspired)
- [ ] Verify Halo color changes to match theme
- [ ] Process text and observe Halo animation
- [ ] Verify theme persists across restart

## 8. End-to-End Integration Test

### 8.1 Complete User Workflow

- [ ] **Setup**: Configure OpenAI with real API key
- [ ] **Setup**: Create routing rule: `^/code → openai` with system prompt: "You are a senior software engineer. Output code only."
- [ ] **Setup**: Set hotkey to `Cmd + Shift + A`
- [ ] **Setup**: Set output mode to Typewriter, speed 100 cps

**Test 1: Code Generation**
- [ ] Open code editor (e.g., VS Code)
- [ ] Type: `/code Write a Python function to reverse a string`
- [ ] Select text, press `Cmd + Shift + A`
- [ ] Expected:
  1. Text is cut (disappears)
  2. Halo appears at cursor (green for OpenAI)
  3. Halo shows "Processing..." animation
  4. AI response is typed back character-by-character
  5. Result is Python function only (no explanation)

**Test 2: General Query**
- [ ] In Notes, type: `What is the capital of Japan?`
- [ ] Select, press hotkey
- [ ] Expected:
  1. Matches catch-all rule `.*` → routes to default provider
  2. Response: `Tokyo` (with brief explanation)

**Test 3: Cancel Operation**
- [ ] Type long query, select, press hotkey
- [ ] While Halo is processing, press Cancel hotkey: `Escape`
- [ ] Expected:
  1. Halo disappears immediately
  2. No text is pasted
  3. Original text is restored (if Cut mode)

### 8.2 Error Handling

**Test: Invalid API Key**
- [ ] Set OpenAI API key to invalid value: `sk-invalid-key`
- [ ] Process text
- [ ] Expected:
  1. Halo shows "Error" state (red color)
  2. Toast notification: "✗ API error: Invalid authentication"
  3. Original text is restored

**Test: Network Timeout**
- [ ] Set provider timeout to very low: `1` second
- [ ] Process text
- [ ] Expected:
  1. Halo shows "Error" state after 1 second
  2. Toast: "✗ Request timeout"

**Test: No Provider Match**
- [ ] Delete all routing rules
- [ ] Set no default provider
- [ ] Process text
- [ ] Expected:
  1. Toast: "⚠️ No matching provider found"

## 9. Memory Module Integration (Already Tested in Phase 4E)

- [ ] Verify memory tab exists and shows:
  - Total interactions stored
  - Last interaction timestamp
  - "View All Memories" button
  - "Clear All Memories" button
- [ ] No new testing needed (covered in Phase 4E)

## 10. Validation Tests

### 10.1 Config Validation Errors

- [ ] Try to save provider without API key (for OpenAI/Claude)
  - Expected: Error: "API key is required for this provider"

- [ ] Try to save rule with invalid regex
  - Expected: Error: "Invalid regex pattern: [error details]"

- [ ] Try to set temperature outside range (e.g., 3.0)
  - Expected: Error: "Temperature must be between 0.0 and 2.0"

- [ ] Try to set timeout to 0
  - Expected: Error: "Timeout must be greater than 0"

### 10.2 UI Input Validation

- [ ] Provider Name: Try empty name
  - Expected: "Save" button disabled

- [ ] Model: Try empty model
  - Expected: "Save" button disabled

- [ ] Color: Try invalid hex (e.g., "not-a-color")
  - Expected: Error or defaults to gray

## Success Criteria

All tests should pass with:

- ✅ All provider operations work (add/edit/delete/test)
- ✅ All routing rule operations work (add/edit/delete/reorder/import/export)
- ✅ Hotkey customization works and persists
- ✅ Behavior settings work and persist
- ✅ Config persists across app restart
- ✅ Hot-reload works within 1 second
- ✅ API keys stored securely in Keychain (not in config.toml)
- ✅ Validation prevents invalid configurations
- ✅ End-to-end workflow works smoothly
- ✅ No crashes or errors during normal operation

## Notes

- If any test fails, document:
  - Test name
  - Expected behavior
  - Actual behavior
  - Steps to reproduce
  - Error messages (if any)

- Performance expectations:
  - Config save: < 100ms
  - Hot-reload detection: < 1s
  - Provider test: < 5s (depends on network)
  - Regex validation: < 10ms
