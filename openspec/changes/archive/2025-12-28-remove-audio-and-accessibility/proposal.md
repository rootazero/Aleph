# Change: Remove Audio Feedback System and Accessibility Features

## Why
Simplify the codebase by removing two non-essential features:
1. **Audio Feedback System**: Sound effects for Halo state transitions add complexity without providing core value for an invisible, frictionless AI middleware
2. **Accessibility Permission Management**: Custom permission handling is over-engineered; users can manage permissions through macOS System Settings directly

These features increase maintenance burden and deviate from the "Ghost" aesthetic philosophy of minimal, invisible operation.

## What Changes
- **REMOVED**: Audio feedback system
  - Delete `Aleph/Sources/Audio/AudioManager.swift`
  - Delete `Aleph/Resources/Sounds/` directory and all sound files
  - Remove `sound_enabled` configuration option from config schema
  - Remove audio-related dependencies from `project.yml`
  - Remove AVFoundation imports from relevant files

- **REMOVED**: Accessibility permission manager
  - Delete `Aleph/Sources/PermissionManager.swift`
  - Remove custom permission checking and prompting logic
  - Remove Accessibility-related code from `AppDelegate.swift` and other files
  - Simplify entitlements to remove unnecessary Accessibility permissions

- **UPDATED**: Documentation
  - Update `CLAUDE.md` to remove references to sound effects and permission management
  - Remove Phase 6 mention of "Sound effects (optional)"
  - Simplify Platform-Specific Notes section
  - Remove PermissionManager from project structure diagram

- **UPDATED**: Configuration schema
  - Remove `sound_enabled` from `[general]` section in example config.toml

## Impact
- **Affected specs**: `macos-client`, `event-handler` (indirectly)
- **Affected code**:
  - `Aleph/Sources/AppDelegate.swift` (remove permission checks)
  - `Aleph/Sources/EventHandler.swift` (remove audio playback calls)
  - `Aleph/Sources/SettingsView.swift` (remove sound toggle if present)
  - `project.yml` (remove deleted files from targets)
  - `CLAUDE.md` (remove documentation sections)
  - Configuration examples in docs

- **Breaking changes**: **NONE** - These are optional features that don't affect core functionality
- **User impact**: Users will no longer hear sounds or see custom permission prompts, but core functionality remains unchanged
- **Testing impact**: Remove audio-related tests if any exist
