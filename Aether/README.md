# Aether - macOS Client

**Native AI Middleware for macOS** - Brings AI intelligence directly to your cursor in ANY application.

## Overview

Aether is a system-level AI middleware that acts as an invisible "ether" connecting user intent with AI models through a frictionless, native interface. No webviews, no permanent windows—only ephemeral UI that appears when summoned.

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
│  │  EventHandler (AetherEventHandler protocol)      │  │
│  │  HaloWindow (Transparent NSWindow overlay)       │  │
│  │  SettingsView (SwiftUI configuration UI)         │  │
│  └──────────────────┬───────────────────────────────┘  │
└─────────────────────┼───────────────────────────────────┘
                      │ UniFFI Bindings
                      ▼
┌─────────────────────────────────────────────────────────┐
│                  Rust Core (libaethecore.dylib)         │
│  ┌──────────────────────────────────────────────────┐  │
│  │  AetherCore (Main orchestrator)                  │  │
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
cd Aether/core
cargo build --release
```

The output library will be at: `Aether/core/target/release/libaethecore.dylib`

### 2. Generate UniFFI Bindings (if needed)

```bash
cd Aether/core
cargo run --bin uniffi-bindgen generate src/aether.udl \
  --language swift \
  --out-dir ../Sources/Generated/
```

### 3. Build macOS Client with Xcode

Option A: **Using Xcode IDE** (Recommended)
```bash
open Aether.xcodeproj
# Press Cmd+B to build, or Cmd+R to run
```

Option B: **Command Line Build**
```bash
xcodebuild -project Aether.xcodeproj \
  -scheme Aether \
  -configuration Release \
  build
```

The built app will be at: `DerivedData/Aether/Build/Products/Release/Aether.app`

### Automated Build Script

The project includes `Scripts/copy_rust_libs.sh` which automatically:
1. Copies `libaethecore.dylib` to the app bundle's Frameworks folder
2. Fixes the library's install name for runtime loading
3. Verifies the integration

This script runs automatically as an Xcode build phase.

## Permissions

### Accessibility Permission (Required)

Aether requires **Accessibility** permission to simulate keyboard input (for pasting AI responses).

**How to grant permission:**
1. Launch Aether
2. A permission prompt will appear automatically
3. Click "Open System Settings"
4. In **Privacy & Security → Accessibility**, enable Aether
5. The app will automatically detect the permission grant

**Why this permission is needed:**
- To simulate `Cmd+C` (copy selected text)
- To simulate `Cmd+V` (paste AI response)
- To simulate keyboard input for "typewriter" output mode

**Note**: Aether never records keystrokes or monitors your activity. It only simulates specific keyboard events when explicitly triggered.

## User Guide

### Basic Usage

1. **Launch Aether**: The app appears in the menu bar (no Dock icon)
2. **Select text** in any application
3. **Press `Cmd + ~`** to trigger Aether
4. **Halo overlay appears** at your cursor showing processing state
5. **AI response** is pasted back (Phase 4 feature - coming soon)

### Menu Bar Options

- **Settings**: Configure providers, routing rules, shortcuts
- **About**: Version information
- **Quit**: Exit Aether

### Settings

Currently, the Settings UI shows placeholders for future features:

- **General**: Theme selection, sound effects (Phase 5)
- **Providers**: AI provider configuration (Phase 4)
- **Routing**: Smart routing rules (Phase 4)
- **Shortcuts**: Hotkey customization (Phase 5)

## Development

### Project Structure

```
Aether/
├── Sources/                     # Swift source files
│   ├── AetherApp.swift          # App entry point
│   ├── AppDelegate.swift        # Menu bar + Rust integration
│   ├── HaloWindow.swift         # Transparent overlay window
│   ├── HaloView.swift           # SwiftUI Halo animations
│   ├── HaloState.swift          # State machine
│   ├── EventHandler.swift       # Rust callback implementation
│   ├── PermissionManager.swift  # Accessibility permissions
│   ├── SettingsView.swift       # Settings UI
│   ├── ProvidersView.swift      # Provider management (stub)
│   ├── RoutingView.swift        # Routing rules (stub)
│   ├── ShortcutsView.swift      # Shortcuts config (stub)
│   └── Generated/
│       └── aether.swift         # UniFFI bindings
├── Resources/
│   └── Info.plist               # App metadata
├── Assets.xcassets/             # App icons
├── Frameworks/
│   └── libaethecore.dylib       # Rust core (embedded)
└── core/                        # Rust core library
    ├── src/
    │   ├── lib.rs               # UniFFI exports
    │   ├── aether.udl           # UniFFI interface definition
    │   ├── core.rs              # AetherCore struct
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
- `EventHandler`: Implements `AetherEventHandler` protocol for Rust callbacks
- `PermissionManager`: Handles Accessibility permission flow

**Rust Layer (Core Logic)**:
- `AetherCore`: Main orchestrator, exposes API via UniFFI
- `HotkeyDetector`: Listens for `Cmd+~` globally (using `rdev`)
- `ClipboardManager`: Reads/writes clipboard (using `arboard`)
- `InputSimulator`: Simulates keyboard events (using `enigo`)

**Communication**:
- Rust → Swift: Callbacks via `AetherEventHandler` trait
- Swift → Rust: Direct method calls on `AetherCore` instance
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
- Rust core: `cd Aether/core && cargo test`
- Swift UI: Use Xcode Test Navigator (Cmd+U)

### Debugging

**Rust Core Debugging**:
```bash
# Enable verbose logging
RUST_LOG=debug cargo run

# Check UniFFI bindings generation
cargo run --bin uniffi-bindgen generate src/aether.udl --language swift
```

**Swift Debugging**:
- Use Xcode breakpoints in `EventHandler.swift` for callback inspection
- Check Console.app for error logs
- Use `print("[Aether] ...")` for debug output

**FFI Boundary Issues**:
```bash
# Verify dylib is in app bundle
ls -la Aether.app/Contents/Frameworks/

# Check dylib install name
otool -L Aether.app/Contents/Frameworks/libaethecore.dylib

# Verify app links to correct library
otool -L Aether.app/Contents/MacOS/Aether | grep libaethecore
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

**Symptom**: App crashes on launch with `dyld: Library not loaded: libaethecore.dylib`

**Solution**:
1. Verify Rust core is built: `ls Aether/core/target/release/libaethecore.dylib`
2. Rebuild the Xcode project to trigger the copy script
3. Check the dylib is in the bundle: `ls Aether.app/Contents/Frameworks/libaethecore.dylib`

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
2. Add Aether to the list (click the `+` button)
3. Enable the toggle for Aether

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

Copyright © 2025 Aether. All rights reserved.

---

**Built with** 🦀 Rust + Swift + ❤️
