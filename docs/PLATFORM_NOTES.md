# Platform-Specific Notes

This document contains platform-specific configuration, permissions, and setup instructions for Aether.

## macOS (Primary Target)

### Required Entitlements

Configure in `Aether.entitlements`:

```xml
<key>com.apple.security.automation.apple-events</key>
<true/>
```

This entitlement is required for simulating keyboard input via Apple Events.

---

### Info.plist Configuration

Configure in `Aether/Info.plist`:

```xml
<!-- Hide Dock icon, Menu Bar only -->
<key>LSUIElement</key>
<true/>

<!-- Accessibility permission description -->
<key>NSAppleEventsUsageDescription</key>
<string>Aether needs to simulate keyboard input to paste AI responses.</string>
```

**Key Behaviors:**
- `LSUIElement = YES`: No Dock icon, app lives in menu bar only
- `NSAppleEventsUsageDescription`: Shown when requesting Accessibility permission

---

### Accessibility Permissions

Aether requires macOS Accessibility permission to:
- Detect global hotkeys (via `rdev`)
- Simulate keyboard input (via `enigo`)
- Query active window title for memory context

**Permission Request Flow:**
1. User launches Aether for the first time
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
- `Aether/Sources/Components/PermissionPromptView.swift` - Permission prompt UI
- `Aether/Sources/AppDelegate.swift` - Permission checking logic (lines 45-68)

---

### Menu Bar Setup

Configure menu bar icon in `AppDelegate`:

```swift
let statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
statusItem.button?.image = NSImage(systemSymbolName: "sparkles", accessibilityDescription: "Aether")
statusItem.menu = createMenu()
```

**Menu Structure:**
- Settings...
- ---
- Quit Aether

**Icon:**
- System symbol: `sparkles`
- Adaptive: Auto-adjusts for Dark/Light Mode

---

### Sandbox Considerations

Aether currently runs **without App Sandbox** to support:
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
cd Aether/core/

# Build for Intel
cargo build --release --target x86_64-apple-darwin

# Build for Apple Silicon
cargo build --release --target aarch64-apple-darwin

# Combine into universal binary
lipo -create \
  target/x86_64-apple-darwin/release/libaethecore.dylib \
  target/aarch64-apple-darwin/release/libaethecore.dylib \
  -output libaethecore.universal.dylib

# Copy to Frameworks directory
cp libaethecore.universal.dylib ../Frameworks/libaethecore.dylib
```

---

## Windows (Future Support)

### Planned Architecture

**UI Layer:** C# + WinUI 3
**Core:** Rust (same codebase, cross-compiled)

### System Tray Integration

```csharp
// System tray icon setup (Windows)
notifyIcon = new NotifyIcon
{
    Icon = new Icon("aether.ico"),
    ContextMenuStrip = CreateContextMenu(),
    Visible = true
};
```

### Required Permissions

- **Accessibility API**: For global hotkeys and keyboard simulation
- **Clipboard Access**: Standard Windows clipboard API

---

## Linux (Future Support)

### Planned Architecture

**UI Layer:** Rust + GTK4
**Core:** Rust (same codebase, native)

### Desktop Integration

```bash
# Install .desktop file for system integration
cp aether.desktop ~/.local/share/applications/

# System tray icon (via libappindicator)
```

### Required Dependencies

```bash
# Ubuntu/Debian
sudo apt install libgtk-4-dev libappindicator3-dev

# Fedora
sudo dnf install gtk4-devel libappindicator-gtk3-devel
```

---

## Cross-Platform Abstractions

### Trait-Based Design

All platform-specific components use trait abstractions:

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
- `clipboard/macos.rs` - macOS-specific clipboard (via `arboard`)
- `clipboard/windows.rs` - Windows-specific clipboard (future)
- `clipboard/linux.rs` - Linux-specific clipboard (future)

---

## Related Documentation

- See `docs/TESTING_GUIDE.md` for platform-specific testing procedures
- See `Aether/core/src/` for platform abstraction implementations
- See `docs/accessibility-testing-checklist.md` for permission testing
