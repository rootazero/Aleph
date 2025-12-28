# Change: Implement Phase 6 Settings UI

## Why

Aether currently has placeholder Settings UI components (SettingsView, ProvidersView, RoutingView, ShortcutsView) that display hardcoded data with "Coming Soon" alerts. Users cannot configure AI providers, routing rules, shortcuts, or behavior settings. To make Aether production-ready, we need a fully functional settings interface that:

1. Allows users to add/edit/delete AI provider credentials (stored securely in macOS Keychain)
2. Enables users to create and manage routing rules with drag-to-reorder functionality
3. Supports global hotkey customization with visual key recorder
4. Provides behavior configuration (input/output modes, typing speed, PII scrubbing)
5. Persists all changes to `~/.config/aether/config.toml` with hot-reload support

This change directly implements Phase 6 from CLAUDE.md and unblocks user customization, which is critical for production deployment.

## What Changes

### New Capabilities

1. **Provider Configuration UI**
   - Add/Edit/Delete AI providers (OpenAI, Claude, Gemini, Ollama)
   - Secure API key input with macOS Keychain integration
   - Test provider connection with real API calls
   - Display provider status (enabled/disabled, last tested timestamp)
   - Visual color picker for provider theme colors

2. **Routing Rules Editor**
   - Create new rules with regex pattern editor
   - Edit existing rules (pattern, provider, system prompt)
   - Delete rules with confirmation
   - Drag-to-reorder rules (priority-based matching)
   - Syntax highlighting for regex patterns
   - Live pattern tester (test input against regex)
   - Import/Export rules as JSON

3. **Hotkey Customization**
   - Visual key recorder for global hotkey
   - Conflict detection (warn if hotkey conflicts with system/other apps)
   - Preset shortcuts library (popular combinations)
   - Reset to default (Cmd+~)

4. **Behavior Settings**
   - Input mode: Cut vs Copy
   - Output mode: Typewriter vs Instant
   - Typing speed slider (10-200 chars/sec) with preview
   - PII scrubbing toggle with regex pattern editor
   - Enable/disable specific PII types (email, phone, SSN, credit card)

5. **Config Management Backend**
   - TOML serialization/deserialization with validation
   - Hot-reload: Watch config file for external changes
   - Atomic writes: Prevent corruption on concurrent modification
   - Config backup/restore
   - Migration support for config schema changes
   - Validation errors display in UI

### Modified Capabilities

- **General Tab**: Replace placeholder "Check for Updates" with functional auto-update check (using Sparkle framework)
- **Memory Tab**: Already implemented in Phase 4E, no changes needed

### Breaking Changes

None. This is purely additive - existing placeholder UI is replaced with functional components.

## Impact

### Affected Specs

- `settings-ui` (NEW) - Comprehensive Settings UI requirements
- `config-management` (NEW) - Config TOML loading/saving/validation
- `macos-client` (MODIFIED) - Update to integrate with config hot-reload

### Affected Code

**New Files:**
- `Aether/Sources/Settings/ProviderConfigView.swift` - Provider add/edit modal
- `Aether/Sources/Settings/RuleEditorView.swift` - Routing rule editor modal
- `Aether/Sources/Settings/HotkeyRecorderView.swift` - Key recorder component
- `Aether/Sources/Settings/ConfigManager.swift` - Swift wrapper for Rust config API
- `Aether/core/src/config/watcher.rs` - File watcher for hot-reload
- `Aether/core/src/config/keychain.rs` - macOS Keychain integration

**Modified Files:**
- `Aether/Sources/SettingsView.swift` - Remove "Coming Soon" alerts
- `Aether/Sources/ProvidersView.swift` - Connect to real config API
- `Aether/Sources/RoutingView.swift` - Connect to real config API
- `Aether/Sources/ShortcutsView.swift` - Add key recorder
- `Aether/Sources/AppDelegate.swift` - Subscribe to config change notifications
- `Aether/core/src/config.rs` - Add hot-reload support

### Dependencies

- **macOS Security Framework** - Keychain API for secure API key storage
- **FSEvents** (via `notify` crate) - File watcher for config hot-reload
- **Sparkle** - Auto-update framework (optional, for Phase 6.1)

### User Experience

**Before:** Users see placeholder settings with "Coming Soon" alerts. No customization possible.

**After:** Users can fully configure Aether through native macOS UI:
- Add API keys for multiple providers
- Create custom routing rules for different use cases
- Customize global hotkey to avoid conflicts
- Fine-tune behavior (typing speed, input mode, PII scrubbing)
- Changes take effect immediately without restart

### Migration Path

No migration needed - config.toml is created with defaults on first launch. Existing placeholder settings are replaced with functional UI.

## Success Criteria

1. **Provider Management**: User can add OpenAI API key, test connection, and successfully invoke GPT-4o
2. **Routing Rules**: User can create rule "^/draw → OpenAI", test clipboard content "/draw a cat", and confirm routing
3. **Hotkey Customization**: User can change hotkey to Cmd+Shift+A, press new hotkey, and Aether responds
4. **Config Persistence**: User closes settings, quits app, relaunches - all settings persist
5. **Hot-Reload**: User edits config.toml externally, settings UI updates within 1 second
6. **Validation**: User enters invalid regex pattern, UI shows error before saving
7. **Keychain Security**: API keys stored in Keychain, not visible in config.toml (only references)
