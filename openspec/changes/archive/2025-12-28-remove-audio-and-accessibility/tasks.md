## 1. Remove Audio Feedback System
- [x] 1.1 Delete `Aleph/Sources/Audio/AudioManager.swift`
- [x] 1.2 Delete `Aleph/Resources/Sounds/` directory
- [x] 1.3 Remove AudioManager references from `EventHandler.swift`
- [x] 1.4 Remove audio playback calls from `EventHandler.swift`
- [x] 1.5 Remove AVFoundation imports from files that used AudioManager
- [x] 1.6 Remove sound toggle from `SettingsView.swift` if present
- [x] 1.7 Update `project.yml` to remove Audio directory from sources

## 2. Remove Accessibility Permission Manager
- [x] 2.1 Delete `Aleph/Sources/PermissionManager.swift`
- [x] 2.2 Remove PermissionManager initialization from `AppDelegate.swift`
- [x] 2.3 Remove permission check calls from `AppDelegate.swift`
- [x] 2.4 Remove permission-related UI components if any
- [x] 2.5 Update `project.yml` to remove PermissionManager from sources
- [x] 2.6 Simplify `Aleph.entitlements` if over-specified

## 3. Update Documentation
- [x] 3.1 Remove `sound_enabled` from configuration schema in `CLAUDE.md`
- [x] 3.2 Remove "Sound effects (optional)" from Phase 6 in `CLAUDE.md`
- [x] 3.3 Remove PermissionManager from project structure diagram in `CLAUDE.md`
- [x] 3.4 Simplify "Accessibility Permissions" section in Platform-Specific Notes
- [x] 3.5 Remove or simplify permission-related documentation
- [x] 3.6 Update any README files that mention audio or permission features

## 4. Update OpenSpec Specifications
- [x] 4.1 Create spec delta for `macos-client` to remove permission documentation requirement
- [x] 4.2 Validate changes with `openspec validate remove-audio-and-accessibility --strict`

## 5. Testing and Verification
- [x] 5.1 Verify project builds successfully after removals
- [ ] 5.2 Test that Halo overlay still works without audio
- [ ] 5.3 Verify no broken references to deleted files
- [x] 5.4 Run `xcodegen generate` to update Xcode project
- [ ] 5.5 Confirm app runs without crashes related to missing components
