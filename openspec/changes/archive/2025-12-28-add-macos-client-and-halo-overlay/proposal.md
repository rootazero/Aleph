# Change: Add macOS Swift Client and Halo Overlay

## Why

With the Rust core foundation complete (Phase 1), we need native macOS UI to make Aether usable by end users. This change implements:
1. **Swift client application** - Menu bar app that integrates with Rust core via UniFFI
2. **Halo overlay** - Transparent, animated window that provides visual feedback at cursor location

Without this change, users cannot:
- Launch Aether as a macOS application
- See visual feedback when the hotkey is pressed
- Access settings or control the application
- Experience the "Ghost" aesthetic that defines Aether's UX

This is Phase 2 of the Aether roadmap and enables the core user interaction flow.

## What Changes

### macOS Swift Client
- Create Xcode project for menu bar application (no Dock icon)
- Implement `AetherEventHandler` protocol to receive callbacks from Rust
- Create menu bar icon with status indicator
- Build Settings UI with SwiftUI (providers, routing rules, shortcuts)
- Integrate Rust library (`libaethecore.dylib`) and Swift bindings
- Request macOS Accessibility permissions
- Handle application lifecycle (launch at login, quit)

### Halo Overlay Window
- Create borderless, transparent NSWindow that floats above all apps
- Implement click-through behavior (never steals focus)
- Track mouse cursor position for Halo placement
- Build animated SwiftUI view with state machine:
  - Idle (invisible)
  - Listening (pulsing ring)
  - Processing (spinning animation)
  - Success (fade out with checkmark)
  - Error (shake with X icon)
- Support provider-specific colors (OpenAI green, Claude orange, etc.)

**Deliverables:**
- Xcode project at `clients/macos/Aether.xcodeproj`
- Swift source files for AppDelegate, HaloWindow, HaloView, SettingsView
- Info.plist with LSUIElement=YES and permission descriptions
- Entitlements file for Accessibility permissions
- Build script to copy Rust dylib into app bundle
- Working app bundle that can be launched and used

**Out of Scope (Future Proposals):**
- AI provider integration UI (will use hardcoded placeholders)
- Advanced routing rules editor (basic list only)
- Keyboard shortcut customization (hardcoded Cmd+~)
- Launch at login implementation (Phase 3)
- App signing and distribution (Phase 6)

## Impact

**Affected specs:**
- **NEW**: `macos-client` - Swift application structure and lifecycle
- **NEW**: `halo-overlay` - Transparent overlay window implementation
- **NEW**: `settings-ui` - SwiftUI-based settings interface
- **MODIFIED**: `event-handler` - Now has concrete Swift implementation
- **MODIFIED**: `uniffi-bridge` - Used in production Swift code

**Affected code:**
- Creates new directory: `clients/macos/`
- No modifications to `core/` (only consumption via UniFFI)
- New Xcode project with Swift UI components

**Dependencies:**
- Requires completed Rust core (Phase 1) ✅
- Requires Swift bindings generation ✅
- Requires macOS 13+ for development/testing
- Requires Xcode 15+ for Swift 5.9 features

**Breaking changes:**
- None (new functionality)

**Migration:**
- N/A (initial UI implementation)
