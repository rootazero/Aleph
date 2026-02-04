# Platform-Specific Notes

This document contains platform-specific configuration, permissions, and setup instructions for Aleph.

## macOS (Primary Target)

### Required Entitlements

Configure in `Aleph.entitlements`:

```xml
<key>com.apple.security.automation.apple-events</key>
<true/>
```

This entitlement is required for simulating keyboard input via Apple Events.

---

### Info.plist Configuration

Configure in `Aleph/Info.plist`:

```xml
<!-- Hide Dock icon, Menu Bar only -->
<key>LSUIElement</key>
<true/>

<!-- Accessibility permission description -->
<key>NSAppleEventsUsageDescription</key>
<string>Aleph needs to simulate keyboard input to paste AI responses.</string>
```

**Key Behaviors:**
- `LSUIElement = YES`: No Dock icon, app lives in menu bar only
- `NSAppleEventsUsageDescription`: Shown when requesting Accessibility permission

---

### Accessibility Permissions

Aleph requires macOS Accessibility permission to:
- Detect global hotkeys (via `rdev`)
- Simulate keyboard input (via `enigo`)
- Query active window title for memory context

**Permission Request Flow:**
1. User launches Aleph for the first time
2. App detects missing Accessibility permission
3. Shows `PermissionGateView` with instructions
4. User manually grants permission in System Settings > Privacy & Security > Accessibility
5. App detects permission grant and initializes Rust core

**Important Notes:**
- macOS does not allow programmatic permission requests for Accessibility
- App must guide user to System Settings manually
- Permission state is checked on every app launch
- Missing permissions block Rust core initialization

**Files:**
- `Aleph/Sources/Components/PermissionPromptView.swift` - Permission prompt UI
- `Aleph/Sources/AppDelegate.swift` - Permission checking logic (lines 45-68)

---

### Menu Bar Setup

Configure menu bar icon in `AppDelegate`:

```swift
let statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
statusItem.button?.image = NSImage(systemSymbolName: "sparkles", accessibilityDescription: "Aleph")
statusItem.menu = createMenu()
```

**Menu Structure:**
- Settings...
- ---
- Quit Aleph

**Icon:**
- System symbol: `sparkles`
- Adaptive: Auto-adjusts for Dark/Light Mode

---

### Sandbox Considerations

Aleph currently runs **without App Sandbox** to support:
- Global hotkey detection
- Clipboard access across all apps
- Keyboard simulation

**Future Hardening:**
- Consider moving to App Sandbox with specific entitlements
- May require refactoring hotkey/clipboard implementation
- Would improve security posture for App Store distribution

---

### Universal Binary (Intel + Apple Silicon)

Build universal binary for distribution:

```bash
cd Aleph/core/

# Build for Intel
cargo build --release --target x86_64-apple-darwin

# Build for Apple Silicon
cargo build --release --target aarch64-apple-darwin

# Combine into universal binary
lipo -create \
  target/x86_64-apple-darwin/release/libalephcore.dylib \
  target/aarch64-apple-darwin/release/libalephcore.dylib \
  -output libalephcore.universal.dylib

# Copy to Frameworks directory
cp libalephcore.universal.dylib ../Frameworks/libalephcore.dylib
```

---

## Tauri Cross-Platform (Windows & Linux)

Aleph uses Tauri 2.0 for cross-platform support on Windows and Linux.

### Architecture

| Layer | Technology |
|-------|------------|
| **Backend** | Rust (Tauri commands) |
| **Frontend** | React + TypeScript |
| **Bundling** | Tauri 2.0 |

### Development

```bash
cd platforms/tauri

# Install dependencies
pnpm install

# Run development mode
pnpm tauri dev

# Build release
pnpm tauri build
```

### Platform-Specific Notes

**Windows:**
- Uses native Windows APIs via Tauri
- System tray integration built-in
- Hotkey registration via Tauri global shortcuts

**Linux:**
- Requires WebKit2GTK for rendering
- System tray via libappindicator

### Required Dependencies (Linux)

```bash
# Ubuntu/Debian
sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev

# Fedora
sudo dnf install webkit2gtk4.1-devel libappindicator-gtk3-devel
```

---

## Windows Native (ARCHIVED)

> **Note**: The Windows native platform (C#/WinUI 3) has been archived.
> Use Tauri for Windows support instead.

See `platforms/windows/ARCHIVED.md` for details.

The archived code may be useful as reference for:
- WinUI 3 transparent window implementation
- Windows global hotkey patterns
- csbindgen FFI patterns

---

## Cross-Platform Abstractions

### Trait-Based Design

Platform-specific components use trait abstractions:

```rust
// Clipboard abstraction
pub trait ClipboardManager {
    fn read_text(&self) -> Result<String>;
    fn write_text(&self, text: &str) -> Result<()>;
}

// Input simulation abstraction
pub trait InputSimulator {
    fn simulate_paste(&self) -> Result<()>;
    fn simulate_cut(&self) -> Result<()>;
}
```

**Platform Implementations:**
- **macOS Native**: Direct implementation via `arboard` and `enigo`
- **Tauri (Windows/Linux)**: Handled by Tauri runtime APIs

---

## Related Documentation

- See `docs/TESTING_GUIDE.md` for platform-specific testing procedures
- See `Aleph/core/src/` for platform abstraction implementations
- See `docs/accessibility-testing-checklist.md` for permission testing
