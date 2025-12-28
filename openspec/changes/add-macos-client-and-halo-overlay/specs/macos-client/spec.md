# Specification: macOS Client

Swift-based macOS application that integrates with Rust core via UniFFI to provide native UI for Aether AI middleware.

## ADDED Requirements

### Requirement: Menu Bar Application

The macOS client **SHALL** run as a menu bar-only application with no Dock icon.

**Why:** Aligns with "Ghost" aesthetic - invisible until needed.

**Acceptance criteria:**
- LSUIElement=YES in Info.plist
- NSStatusItem created in menu bar
- No window in Dock
- App survives system sleep/wake

#### Scenario: User launches Aether

**Given** Aether.app is installed
**When** user double-clicks Aether.app
**Then** menu bar icon appears with sparkles symbol
**And** no window appears in Dock
**And** app can be quit via menu bar menu

---

### Requirement: AetherEventHandler Implementation

The client **SHALL** implement the AetherEventHandler protocol to receive callbacks from Rust core.

**Why:** Required for Rust → Swift communication via UniFFI.

**Acceptance criteria:**
- EventHandler class conforms to AetherEventHandler
- All callback methods use DispatchQueue.main.async
- Callbacks trigger UI updates (Halo, menu bar icon)
- Thread-safe state management

#### Scenario: Rust core triggers hotkey callback

**Given** AetherCore is initialized with EventHandler
**When** Rust detects Cmd+~ hotkey
**Then** onHotkeyDetected() is called on background thread
**And** DispatchQueue.main.async executes UI update
**And** HaloWindow shows at cursor location

---

### Requirement: Settings UI

The client **SHALL** provide a settings interface with tabs for providers, routing, and shortcuts.

**Why:** Users need to configure AI providers and routing rules.

**Acceptance criteria:**
- SwiftUI-based settings window
- 4 tabs: General, Providers, Routing, Shortcuts
- Window size: 600x500
- Accessible via menu bar "Settings" item

#### Scenario: User opens settings

**Given** app is running
**When** user clicks "Settings" in menu bar
**Then** settings window appears at 600x500 size
**And** General tab is selected by default
**And** window is movable and closable

---

### Requirement: Accessibility Permission Handling

The client **SHALL** request and validate macOS Accessibility permissions on launch.

**Why:** Required for global hotkey detection via rdev.

**Acceptance criteria:**
- Check AXIsProcessTrusted() on launch
- Show alert if permission missing
- Provide "Open System Settings" button
- Poll for permission grant (every 2 seconds)
- Start hotkey listening only when granted

#### Scenario: First launch without permission

**Given** user launches Aether for first time
**When** app checks Accessibility permission
**Then** permission is not granted
**And** alert shows explaining why it's needed
**When** user clicks "Open System Settings"
**Then** System Settings opens to Accessibility pane
**When** user grants permission
**Then** app detects grant within 2 seconds
**And** starts hotkey listening

---

### Requirement: Rust Core Lifecycle Management

The client **SHALL** properly initialize and clean up the Rust AetherCore instance.

**Why:** Prevents resource leaks and ensures clean shutdown.

**Acceptance criteria:**
- AetherCore created in applicationDidFinishLaunching
- startListening() called after permission check
- stopListening() called in applicationWillTerminate
- Error handling for AetherError exceptions
- No crashes on quit

#### Scenario: App shutdown

**Given** app is running with AetherCore listening
**When** user quits via menu bar "Quit"
**Then** applicationWillTerminate is called
**And** core.stopListening() executes successfully
**And** app quits without errors

---

### Requirement: Build Script Integration

The client **SHALL** automatically copy the Rust dylib into the app bundle during build.

**Why:** Ensures libaethecore.dylib is available at runtime.

**Acceptance criteria:**
- Build phase script copies dylib from core/target/release/
- Dylib placed in Frameworks/ folder
- @rpath configured correctly
- App runs without dylib not found errors

#### Scenario: Building the app

**Given** Rust core is built (cargo build --release)
**When** Xcode builds the Swift client
**Then** copy_rust_libs.sh script executes
**And** libaethecore.dylib is copied to Frameworks/
**And** install_name_tool sets correct @rpath
**And** built app can be launched without errors
