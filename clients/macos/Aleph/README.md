# Aleph - macOS Client

**Native AI Middleware for macOS** - Brings AI intelligence directly to your cursor in ANY application.

## Overview

Aleph is a system-level AI middleware that acts as an invisible "ether" connecting user intent with AI models through a frictionless, native interface. No webviews, no permanent windows—only ephemeral UI that appears when summoned.

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│                     User Application                     │
│              (Notes, Safari, VSCode, etc.)              │
└──────────────────┬──────────────────────────────────────┘
                   │ Cmd + ~ (Global Hotkey)
                   ▼
┌─────────────────────────────────────────────────────────┐
│                    macOS Swift Client                    │
│  ┌──────────────────────────────────────────────────┐  │
│  │  AppDelegate (Menu Bar + Lifecycle)              │  │
│  │  EventHandler (AlephEventHandler protocol)      │  │
│  │  HaloWindow (Transparent NSWindow overlay)       │  │
│  │  SettingsView (SwiftUI configuration UI)         │  │
│  └──────────────────┬───────────────────────────────┘  │
└─────────────────────┼───────────────────────────────────┘
                      │ UniFFI Bindings
                      ▼
┌─────────────────────────────────────────────────────────┐
│                  Rust Core (libalephcore.dylib)         │
│  ┌──────────────────────────────────────────────────┐  │
│  │  AlephCore (Main orchestrator)                  │  │
│  │  Hotkey Detection (rdev)                         │  │
│  │  Clipboard Management (arboard)                  │  │
│  │  Input Simulation (enigo)                        │  │
│  │  Router (Smart AI provider selection)            │  │
│  └──────────────────┬───────────────────────────────┘  │
└─────────────────────┼───────────────────────────────────┘
                      │ HTTP/CLI
                      ▼
        ┌──────────────────────────────────┐
        │   AI Providers (Future Phase 4)  │
        │   - OpenAI                        │
        │   - Claude (Anthropic)            │
        │   - Gemini (Google)               │
        │   - Ollama (Local)                │
        └──────────────────────────────────┘
```

## System Requirements

- **macOS**: 13.0 (Ventura) or later
- **Xcode**: 15.0 or later (for building)
- **Rust**: 1.70 or later (for building Rust core)
- **Swift**: 5.9+ (bundled with Xcode)

## Building from Source

### 1. Build Rust Core

```bash
cd Aleph/core
cargo build --release
```

The output library will be at: `Aleph/core/target/release/libalephcore.dylib`

### 2. Generate UniFFI Bindings (if needed)

```bash
cd Aleph/core
cargo run --bin uniffi-bindgen generate src/aleph.udl \
  --language swift \
  --out-dir ../Sources/Generated/
```

### 3. Build macOS Client with Xcode

Option A: **Using Xcode IDE** (Recommended)
```bash
open Aleph.xcodeproj
# Press Cmd+B to build, or Cmd+R to run
```

Option B: **Command Line Build**
```bash
xcodebuild -project Aleph.xcodeproj \
  -scheme Aleph \
  -configuration Release \
  build
```

The built app will be at: `DerivedData/Aleph/Build/Products/Release/Aleph.app`

### Automated Build Script

The project includes `Scripts/copy_rust_libs.sh` which automatically:
1. Copies `libalephcore.dylib` to the app bundle's Frameworks folder
2. Fixes the library's install name for runtime loading
3. Verifies the integration

This script runs automatically as an Xcode build phase.

## Permissions

### Accessibility Permission (Required)

Aleph requires **Accessibility** permission to simulate keyboard input (for pasting AI responses).

**How to grant permission:**
1. Launch Aleph
2. A permission prompt will appear automatically
3. Click "Open System Settings"
4. In **Privacy & Security → Accessibility**, enable Aleph
5. The app will automatically detect the permission grant

**Why this permission is needed:**
- To simulate `Cmd+C` (copy selected text)
- To simulate `Cmd+V` (paste AI response)
- To simulate keyboard input for "typewriter" output mode

**Note**: Aleph never records keystrokes or monitors your activity. It only simulates specific keyboard events when explicitly triggered.

## User Guide

### Basic Usage

1. **Launch Aleph**: The app appears in the menu bar (no Dock icon)
2. **Select text** in any application
3. **Press `Cmd + ~`** to trigger Aleph
4. **Halo overlay appears** at your cursor showing processing state
5. **AI response** is pasted back (Phase 4 feature - coming soon)

### Menu Bar Options

- **Settings**: Configure providers, routing rules, shortcuts
- **About**: Version information
- **Quit**: Exit Aleph

### Settings

Currently, the Settings UI shows placeholders for future features:

- **General**: Theme selection, sound effects (Phase 5)
- **Providers**: AI provider configuration (Phase 4)
- **Routing**: Smart routing rules (Phase 4)
- **Shortcuts**: Hotkey customization (Phase 5)

## Development

### Project Structure

```
Aleph/
├── Sources/                     # Swift source files
│   ├── AlephApp.swift          # App entry point
│   ├── AppDelegate.swift        # Menu bar + Rust integration
│   ├── HaloWindow.swift         # Transparent overlay window
│   ├── HaloView.swift           # SwiftUI Halo animations
│   ├── HaloState.swift          # State machine
│   ├── EventHandler.swift       # Rust callback implementation
│   ├── PermissionManager.swift  # Accessibility permissions
│   ├── SettingsView.swift       # Settings UI (simplified, opens Dashboard)
│   ├── RoutingView.swift        # Routing rules (stub)
│   ├── ShortcutsView.swift      # Shortcuts config (stub)
│   └── Generated/
│       └── aleph.swift         # UniFFI bindings
├── Resources/
│   └── Info.plist               # App metadata
├── Assets.xcassets/             # App icons
├── Frameworks/
│   └── libalephcore.dylib       # Rust core (embedded)
└── core/                        # Rust core library
    ├── src/
    │   ├── lib.rs               # UniFFI exports
    │   ├── aleph.udl           # UniFFI interface definition
    │   ├── core.rs              # AlephCore struct
    │   ├── event_handler.rs     # Callback trait
    │   ├── hotkey/              # Global hotkey detection
    │   ├── clipboard/           # Clipboard management
    │   └── input/               # Keyboard simulation
    └── Cargo.toml               # Rust dependencies
```

### Key Components

**Swift Layer (UI)**:
- `AppDelegate`: Menu bar lifecycle, Rust core initialization
- `HaloWindow`: Borderless, transparent, floating overlay
- `EventHandler`: Implements `AlephEventHandler` protocol for Rust callbacks
- `PermissionManager`: Handles Accessibility permission flow

**Rust Layer (Core Logic)**:
- `AlephCore`: Main orchestrator, exposes API via UniFFI
- `HotkeyDetector`: Listens for `Cmd+~` globally (using `rdev`)
- `ClipboardManager`: Reads/writes clipboard (using `arboard`)
- `InputSimulator`: Simulates keyboard events (using `enigo`)

**Communication**:
- Rust → Swift: Callbacks via `AlephEventHandler` trait
- Swift → Rust: Direct method calls on `AlephCore` instance
- UniFFI handles all FFI binding generation automatically

### Testing

**Manual Testing Checklist**:
- [ ] App launches without Dock icon (menu bar only)
- [ ] Menu bar icon appears and responds to clicks
- [ ] Halo overlay appears at cursor location
- [ ] Halo never steals focus from active app
- [ ] All animation states render smoothly (listening, processing, success, error)
- [ ] Multi-monitor support (Halo appears on correct screen)
- [ ] Permission prompt flow works correctly
- [ ] App runs for 30+ minutes without crashes

**Automated Tests**:
- Rust core: `cd Aleph/core && cargo test`
- Swift UI: Use Xcode Test Navigator (Cmd+U)

### Debugging

**Rust Core Debugging**:
```bash
# Enable verbose logging
RUST_LOG=debug cargo run

# Check UniFFI bindings generation
cargo run --bin uniffi-bindgen generate src/aleph.udl --language swift
```

**Swift Debugging**:
- Use Xcode breakpoints in `EventHandler.swift` for callback inspection
- Check Console.app for error logs
- Use `print("[Aleph] ...")` for debug output

**FFI Boundary Issues**:
```bash
# Verify dylib is in app bundle
ls -la Aleph.app/Contents/Frameworks/

# Check dylib install name
otool -L Aleph.app/Contents/Frameworks/libalephcore.dylib

# Verify app links to correct library
otool -L Aleph.app/Contents/MacOS/Aleph | grep libalephcore
```

## Known Limitations

### Current Phase (Phase 2) Limitations

- **No AI Integration**: The app does not yet connect to AI providers (OpenAI, Claude, etc.). This is planned for Phase 4.
- **Settings Are Placeholders**: Provider configuration, routing rules, and shortcut customization are not yet functional.
- **Hardcoded Hotkey**: Currently fixed to `Cmd + ~`. Customization coming in Phase 5.
- **No Launch at Login**: Auto-start functionality will be added in Phase 3.

### Platform-Specific Quirks

- **Multi-Monitor Edge Cases**: On some multi-monitor setups with different resolutions, the Halo position may need adjustment.
- **Accessibility Permission**: The app cannot function without Accessibility permission. Users must grant this manually.
- **macOS 13+ Only**: Older macOS versions are not supported due to Swift 5.9+ requirements.

## Troubleshooting

### "Library not loaded" Error

**Symptom**: App crashes on launch with `dyld: Library not loaded: libalephcore.dylib`

**Solution**:
1. Verify Rust core is built: `ls Aleph/core/target/release/libalephcore.dylib`
2. Rebuild the Xcode project to trigger the copy script
3. Check the dylib is in the bundle: `ls Aleph.app/Contents/Frameworks/libalephcore.dylib`

### Halo Doesn't Appear

**Possible Causes**:
1. **No Accessibility Permission**: Check System Settings → Privacy & Security → Accessibility
2. **Rust Core Not Initialized**: Check Console.app for error logs
3. **Wrong Hotkey**: Ensure you're pressing `Cmd + ~` (tilde, next to number 1)

### App Not in Menu Bar

**Symptom**: App launches but no menu bar icon appears

**Solution**:
1. Check `Info.plist` has `LSUIElement = YES`
2. Verify `AppDelegate` creates `NSStatusItem` correctly
3. Restart the Mac (sometimes required after Accessibility permission grant)

### Permission Prompt Not Appearing

**Symptom**: No permission prompt on first launch

**Solution**:
1. Manually open System Settings → Privacy & Security → Accessibility
2. Add Aleph to the list (click the `+` button)
3. Enable the toggle for Aleph

## Roadmap

- [x] **Phase 1**: Rust core + UniFFI bindings ✅
- [x] **Phase 2**: macOS client + Halo overlay (CURRENT) ✅
- [ ] **Phase 3**: Halo overlay refinements
- [ ] **Phase 4**: AI provider integration (OpenAI, Claude, Gemini, Ollama)
- [ ] **Phase 5**: Settings UI (provider config, routing rules, shortcuts)
- [ ] **Phase 6**: Production polish (code signing, distribution, updates)

## Contributing

This project is under active development. Contributions are welcome after Phase 2 is complete.

## License

Copyright © 2025 Aleph. All rights reserved.

---

**Built with** 🦀 Rust + Swift + ❤️
