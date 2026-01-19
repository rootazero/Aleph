# Settings UI User Guide

Welcome to the Aether Settings UI guide! This document will walk you through all the features available in the Settings interface.

## Table of Contents

1. [Opening Settings](#opening-settings)
2. [General Tab](#general-tab)
3. [Providers Tab](#providers-tab)
4. [Routing Tab](#routing-tab)
5. [Shortcuts Tab](#shortcuts-tab)
6. [Behavior Tab](#behavior-tab)
7. [Memory Tab](#memory-tab)
8. [Troubleshooting](#troubleshooting)

---

## Opening Settings

There are two ways to open Aether Settings:

1. **Menu Bar**: Click the Aether icon (✨) in your menu bar → Select "Settings..."
2. **Keyboard**: Press `Cmd+,` when Aether is active

The Settings window will appear with multiple tabs at the top.

---

## General Tab

The General tab displays app information and global preferences.

### Version Information

- **Current Version**: Shows your installed version (e.g., "Version 0.1.0 (Build 1)")
- **Source**: Reads dynamically from `Info.plist` - always up-to-date

### Theme Selection

Choose a visual theme for the Halo overlay:

- **Cyberpunk** (Default): Neon colors, futuristic aesthetic
- **Zen**: Minimal, monochrome design
- **Jarvis**: Iron Man-inspired (blue/gold palette)

**How to change theme:**
1. Click the theme dropdown
2. Select your preferred theme
3. Theme applies immediately (no restart needed)

### Check for Updates

- **Button**: "Check for Updates"
- **Behavior**:
  - Checks GitHub releases for newer versions
  - Shows notification if update available
  - Opens download page in browser

**Note**: Full Sparkle auto-update integration coming in Phase 6.1

---

## Providers Tab

Manage your AI provider configurations (OpenAI, Claude, Ollama, etc.)

### Provider List

Displays all configured providers with:
- **Name**: Provider identifier (e.g., "openai", "claude")
- **Model**: Current model (e.g., "gpt-4o", "claude-3-5-sonnet")
- **Status**: ✓ Configured / ⚠ Not Configured
- **Last Tested**: Timestamp of last connection test

### Adding a New Provider

1. Click **"Configure"** button for a preset provider (OpenAI, Claude, Ollama)
   - OR click **"Add Provider"** to create custom provider

2. Fill in the configuration form:

   **Required Fields:**
   - **Provider Name**: Unique identifier (e.g., "openai", "deepseek")
   - **Model**: Model name (e.g., "gpt-4o", "llama3.2")

   **Optional Fields:**
   - **Provider Type**: Auto-detected from name (openai/claude/ollama)
   - **API Key**: Required for cloud providers (OpenAI, Claude)
     - *Stored securely in macOS Keychain*
     - *Never saved in config.toml file*
   - **Base URL**: Custom API endpoint (for OpenAI-compatible APIs)
   - **Color**: Hex color for UI (e.g., "#10a37f")
   - **Timeout**: Request timeout in seconds (default: 30)
   - **Max Tokens**: Maximum response length (default: 4096)
   - **Temperature**: Response randomness 0.0-2.0 (default: 0.7)

3. Click **"Save"**

**Validation:**
- Empty required fields → "Save" button disabled
- Invalid API key format → Warning shown
- Invalid timeout/temperature → Error message

### Testing Provider Connection

1. Select a provider from the list
2. Click **"Test Connection"** button
3. Wait for result (spinner shows during test)

**Possible outcomes:**
- ✅ **Success**: "Connection successful" (green toast)
- ❌ **Failure**: Error message with reason (red toast)
  - "Invalid API key"
  - "Network timeout"
  - "Model not found"

### Editing a Provider

1. Click the provider row in the list
2. Click **"Edit"** button
3. Modify fields as needed
4. Click **"Save"**

**Note**: API key is pre-filled from Keychain (shown as dots)

### Deleting a Provider

1. Select a provider from the list
2. Click **"Delete"** button
3. Confirm deletion in dialog
4. Provider is removed from:
   - Config file (`config.toml`)
   - Keychain (API key deleted)

**Warning**: Cannot delete if provider is referenced in routing rules

---

## Routing Tab

Create rules to automatically route requests to different AI providers based on input patterns.

### How Routing Works

1. User selects text and presses hotkey
2. Aether checks routing rules **in order (top to bottom)**
3. First matching rule determines which provider to use
4. If no rules match, uses `default_provider` from General settings

### Rule List

Displays all rules with:
- **Pattern**: Regex pattern (e.g., `^/code`, `.*`)
- **Provider**: Target provider name
- **System Prompt**: Custom prompt (if set)

### Creating a New Rule

1. Click **"Add Rule"** button
2. Fill in the rule editor:

   **Pattern** (required):
   - Regex pattern to match against user input
   - Examples:
     - `^/code` → Matches text starting with "/code"
     - `.*` → Matches everything (catch-all)
     - `python|rust|javascript` → Matches programming languages
   - Real-time validation shows ✓ or ✗
   - Invalid patterns disable "Save" button

   **Provider** (required):
   - Dropdown of configured providers
   - Must exist in Providers tab

   **System Prompt** (optional):
   - Custom instruction for AI (multiline text)
   - Overrides default model behavior
   - Example: "You are a senior software engineer. Output code only."

3. **Test Pattern** (optional but recommended):
   - Enter sample text in "Test Input" field
   - Click "Test" button
   - Shows: ✓ Match or ✗ No match

4. Click **"Save"**

### Editing a Rule

1. Click rule row in the list
2. Click **"Edit"** button
3. Modify fields
4. Test pattern (recommended)
5. Click **"Save"**

### Deleting a Rule

1. Select rule from list
2. Click **"Delete"** button
3. Confirm deletion
4. Rule removed from `config.toml`

### Reordering Rules

**Rules are evaluated top-to-bottom → Order matters!**

**How to reorder:**
1. Hover over rule row (drag handle appears on left)
2. Click and drag rule to new position
3. Release to drop
4. Order is automatically saved

**Best Practice:**
- Specific rules at top (e.g., `^/code`)
- Catch-all rule (`.*`) at bottom

### Import/Export Rules

**Export:**
1. Click **"Export Rules"** button
2. Choose save location
3. Rules saved as JSON file

**Import:**
1. Click **"Import Rules"** button
2. Select JSON file
3. Choose strategy:
   - **Append**: Add imported rules to end of list
   - **Replace**: Replace all existing rules
4. Click "Import"

**JSON Format Example:**
```json
[
  {
    "regex": "^/code",
    "provider": "claude",
    "system_prompt": "You are a senior software engineer."
  },
  {
    "regex": ".*",
    "provider": "openai"
  }
]
```

---

## Shortcuts Tab

Customize trigger hotkeys for Aether operations.

### Trigger Hotkeys (New System)

Aether uses a **double-tap modifier key** system for triggering AI operations:

**Replace Trigger**: Double-tap **Left Shift**
- AI response replaces selected text
- Default: `DoubleTap+leftShift`

**Append Trigger**: Double-tap **Right Shift**
- AI response appends after selected text
- Default: `DoubleTap+rightShift`

**Supported Modifiers:**
- `leftShift` / `rightShift`
- `leftControl` / `rightControl`
- `leftOption` / `rightOption`
- `leftCommand` / `rightCommand`

**How to use:**
1. Select text in any app
2. Double-tap the modifier key quickly
3. Aether processes the text
4. Result is pasted back

### Cancel Hotkey

**Purpose**: Stop Aether processing and restore original text

**Default**: `Escape`

### Other Shortcuts

- **Command Prompt**: `Command+Option+/` - Quick command completion
- **OCR Capture**: `Command+Option+O` - Capture screen text

---

## Behavior Tab

Configure how Aether captures input and displays output.

### Input Mode

**Controls how text is captured from source app**

**Options:**
- **Cut** (Default): Text is removed from source (Cmd+X)
  - Visual feedback: text "disappears" into Aether
  - More dramatic, confirms operation started
- **Copy**: Text remains in source (Cmd+C)
  - Safer option, non-destructive
  - Original text stays in app

**How to change:**
1. Select radio button (Cut or Copy)
2. Setting saves automatically

**Test:**
1. Select text in any app
2. Press summon hotkey
3. Observe: Cut removes text, Copy leaves it

### Output Mode

**Controls how AI response is pasted back**

**Options:**
- **Typewriter** (Default): Types character-by-character
  - Smooth animation, cinematic effect
  - Speed controlled by Typing Speed slider
- **Instant**: Pastes entire response immediately
  - Fastest option, no delay
  - Good for long responses

**How to change:**
1. Select radio button (Typewriter or Instant)
2. Setting saves automatically

### Typing Speed

**Only applies when Output Mode = Typewriter**

**Range**: 10-200 characters per second

**Slider positions:**
- **10 cps**: Very slow (dramatic effect)
- **50 cps**: Default (balanced, readable)
- **100 cps**: Fast (quick results)
- **200 cps**: Very fast (minimal delay)

**How to adjust:**
1. Drag slider to desired speed
2. Current value shown above slider
3. Click **"Preview"** to see animation at selected speed
   - Sample text types out in modal window
   - Observe speed before committing

**Recommendation:**
- 50-100 cps for most users
- 10-30 cps for presentations/demos
- 150-200 cps for maximum speed

### PII Scrubbing

**Purpose**: Remove personally identifiable information before sending to AI

**Toggle**: Enable/Disable PII scrubbing

**How it works:**
1. When enabled, Aether scans input text
2. Detects and redacts sensitive information:
   - **Email addresses**: `john@example.com` → `[EMAIL_REDACTED]`
   - **Phone numbers**: `(555) 123-4567` → `[PHONE_REDACTED]`
   - **SSN**: `123-45-6789` → `[SSN_REDACTED]`
   - **Credit cards**: `4111-1111-1111-1111` → `[CARD_REDACTED]`
3. Redacted text sent to AI provider
4. Original text never leaves your device

**Custom Patterns (Advanced):**
1. Click **"Advanced"** to show regex editor
2. Add custom PII patterns:
   - Pattern Name: "API Keys"
   - Regex: `sk-[a-zA-Z0-9]{32,}`
3. Save pattern
4. Custom patterns apply along with built-in ones

**Use Cases:**
- Sharing code snippets with API keys
- Processing documents with sensitive data
- Extra privacy when using cloud AI providers

---

## Memory Tab

View and manage Aether's long-term memory (context-aware interactions).

**Note**: Memory module was implemented in Phase 4E. See [Memory Module Guide](./memory-module-guide.md) for detailed documentation.

### Quick Overview

**What is Memory?**
- Aether remembers past interactions per-app and per-window
- Retrieved context is injected into AI prompts automatically
- All data stored locally (never synced to cloud)

**Features:**
- **View All Memories**: Browse stored interactions
- **Filter by App/Window**: See context for specific apps
- **Delete Memories**: Remove specific entries or clear all
- **Retention Policy**: Auto-delete after N days
- **Privacy Controls**: Exclude apps (e.g., password managers)

**Settings:**
- **Max Context Items**: How many past interactions to retrieve (default: 5)
- **Retention Days**: Auto-delete after N days (default: 90, 0 = never)
- **Similarity Threshold**: Minimum relevance score 0.0-1.0 (default: 0.7)
- **Excluded Apps**: List of app bundle IDs to never remember

---

## Troubleshooting

### Settings Won't Open

**Symptom**: Clicking Settings menu does nothing

**Solutions:**
1. Check Console.app for errors
2. Restart Aether: Quit (Cmd+Q) and relaunch
3. Reset settings: Delete `~/.config/aether/config.toml` and restart

### Changes Not Saving

**Symptom**: Settings revert after closing window

**Solutions:**
1. Check file permissions:
   ```bash
   chmod 644 ~/.config/aether/config.toml
   ```
2. Verify config directory exists:
   ```bash
   mkdir -p ~/.config/aether
   ```
3. Check for TOML syntax errors (use Settings UI validation)

### Hot-Reload Not Working

**Symptom**: External edits to config.toml don't reflect in UI

**Solutions:**
1. File watcher requires macOS FSEvents support (built-in)
2. Don't use vim's in-place edit mode (use `:w` not `:wq`)
3. Save changes fully before expecting reload
4. Wait 1 second for debounce delay

### API Key Not Stored

**Symptom**: Provider test fails with "No API key" after saving

**Solutions:**
1. Grant Keychain access when prompted
2. Check Keychain Access app for "Aether:provider-name" entry
3. Re-enter API key and save again
4. Ensure no special characters in API key

### Hotkey Not Working

**Symptom**: Pressing hotkey doesn't trigger Aether

**Solutions:**
1. **Check Accessibility Permissions**:
   - System Settings → Privacy & Security → Accessibility
   - Ensure Aether is checked
   - If not, click "+" and add Aether

2. **Hotkey Conflict**:
   - Try a different hotkey in Settings
   - Use conflict detector (shows warnings)
   - Avoid: Cmd+C, Cmd+V, Cmd+X (system shortcuts)

3. **Restart Aether**: Quit and relaunch

### Provider Test Fails

**Symptom**: "Test Connection" shows error

**Common Errors:**

**"Invalid API key"**
- Check key is correct (no typos)
- Verify key is active on provider website
- Re-enter key in Settings

**"Network timeout"**
- Increase timeout in provider settings (e.g., 60 seconds)
- Check internet connection
- Try again in a few seconds

**"Model not found"**
- For Ollama: Run `ollama pull model-name`
- For cloud: Verify model name is correct
- Check provider documentation for available models

**"Rate limit exceeded"**
- Wait a few minutes
- Upgrade API plan
- Use local Ollama to avoid rate limits

### Rules Not Matching

**Symptom**: Routing rule doesn't trigger expected provider

**Solutions:**
1. **Check Rule Order**: Specific rules must be above catch-all (`.*`)
2. **Test Regex Pattern**: Use "Test Pattern" in rule editor
3. **Check Provider Exists**: Referenced provider must be configured
4. **Reload Config**: Hot-reload may be delayed (wait 1 second)

### Typewriter Effect Too Fast/Slow

**Symptom**: Text types at wrong speed

**Solutions:**
1. Adjust Typing Speed slider in Behavior tab
2. Use Preview feature to test before applying
3. Recommended: 50-100 cps for most use cases
4. Ensure Output Mode is set to "Typewriter" (not "Instant")

---

## Tips & Best Practices

### 1. Provider Management
- **Test connections** after adding providers to verify API keys
- **Use custom colors** to visually distinguish providers in Halo
- **Keep timeouts reasonable**: 30s for cloud, 60s for local models

### 2. Routing Rules
- **Order matters**: Place specific rules before general ones
- **Test patterns** with real input before saving
- **Use descriptive prompts**: Helps AI understand context
- **Export rules regularly**: Backup before making major changes

### 3. Hotkey Selection
- **Avoid conflicts**: Don't use Cmd+C, Cmd+V, Cmd+X
- **Use Shift modifier**: Cmd+Shift+[Key] rarely conflicts
- **Test immediately**: Press hotkey after saving to verify it works
- **Cancel shortcut**: Keep it simple (Escape is good default)

### 4. Behavior Tuning
- **Start with defaults**: Cut mode + Typewriter at 50 cps
- **Adjust based on preference**: Copy mode feels safer for new users
- **Use PII scrubbing cautiously**: May over-redact in some cases
- **Preview before committing**: Test typing speed with Preview button

### 5. Security & Privacy
- **API keys in Keychain**: Never share config.toml with API keys
- **Exclude sensitive apps**: Add password managers to memory exclusions
- **Review memory regularly**: Check what Aether remembers
- **Use PII scrubbing**: When pasting sensitive documents

### 6. Configuration Management
- **Hot-reload is your friend**: No need to restart after config edits
- **Keep backups**: Export routing rules before major changes
- **Version control**: Track config.toml (without API keys) in git
- **Document custom setups**: Comment your TOML file for future reference

---

## Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| Open Settings | `Cmd+,` |
| Close Settings | `Cmd+W` or `Esc` |
| Switch Tabs | `Cmd+1` through `Cmd+6` |
| Save (in modals) | `Cmd+S` or `Enter` |
| Cancel (in modals) | `Esc` |

---

## Additional Resources

- **Configuration Reference**: See `config.example.toml` for detailed config options
- **Memory Module Guide**: `docs/memory-module-guide.md`
- **Manual Testing Checklist**: `docs/manual-testing-checklist.md`
- **GitHub Issues**: [Report bugs](https://github.com/your-repo/aether/issues)
- **Phase 6 Implementation**: `openspec/changes/implement-settings-ui-phase6/`

---

## Feedback

Have suggestions for improving the Settings UI?

1. Open an issue on GitHub
2. Include screenshots if possible
3. Describe your use case
4. Tag with `enhancement` label

We're constantly improving Aether based on user feedback!
