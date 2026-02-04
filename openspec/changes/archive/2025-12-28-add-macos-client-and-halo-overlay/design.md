# Design: macOS Swift Client and Halo Overlay

## Context

Aleph's "Ghost" aesthetic requires a macOS application that:
1. Has NO permanent windows (only ephemeral Halo overlay)
2. Lives exclusively in the menu bar (no Dock icon)
3. NEVER steals focus from the active application
4. Provides instant visual feedback at the exact cursor location

**Stakeholders:**
- End users - Need intuitive, non-intrusive AI integration
- macOS platform - Must follow system conventions and permissions
- Development team - Need maintainable, testable Swift code

**Constraints:**
- Must use SwiftUI for all UI components (no UIKit/AppKit except NSWindow)
- Must preserve focus in active app during entire Halo lifecycle
- Must work with macOS Sandbox restrictions (no private APIs)
- Must handle permissions gracefully (Accessibility, Keychain)

## Goals / Non-Goals

**Goals:**
- ✅ Create working menu bar application
- ✅ Implement transparent, click-through Halo overlay
- ✅ Integrate Rust core via UniFFI callbacks
- ✅ Build functional Settings UI
- ✅ Request and handle macOS permissions
- ✅ Provide visual feedback for all processing states

**Non-Goals:**
- ❌ Implement full routing rules editor (basic list only)
- ❌ Add keyboard shortcut customization (hardcoded for now)
- ❌ Build AI provider configuration UI (placeholders)
- ❌ Implement launch at login
- ❌ App signing and notarization

## Decisions

### Decision 1: Use NSWindow for Halo, Not Native macOS Effects

**Rationale:**
- Need full control over animation lifecycle
- Must support arbitrary shapes and colors (provider-specific)
- Need to position at exact cursor coordinates
- SwiftUI views can be embedded in NSWindow

**Alternatives considered:**
- NSStatusItem popover: Can't position at cursor, limited styling
- Overlay View in existing window: Can't float above all apps
- Native system alerts: Can't customize appearance

**Trade-offs:**
- ✅ Pro: Full control over appearance and positioning
- ✅ Pro: Can use SwiftUI for animation state machine
- ⚠️ Con: Must carefully manage window level and focus

### Decision 2: SwiftUI for Halo Animation State Machine

**Rationale:**
- Declarative state transitions (Idle → Listening → Processing → Success/Error)
- Built-in animation primitives
- Easy to extend with new states
- Type-safe state enum

**Pattern:**
```swift
enum HaloState {
    case idle
    case listening
    case processing(providerColor: Color)
    case success
    case error
}

struct HaloView: View {
    @State var state: HaloState = .idle

    var body: some View {
        switch state {
        case .idle: EmptyView()
        case .listening: PulsingRingView()
        case .processing(let color): SpinnerView(color: color)
        case .success: CheckmarkView()
        case .error: ErrorView()
        }
    }
}
```

**Trade-offs:**
- ✅ Pro: Clean, maintainable state management
- ✅ Pro: Built-in animation support
- ⚠️ Con: Learning curve for custom animations

### Decision 3: Use DispatchQueue.main for Rust Callback Threading

**Rationale:**
- Rust callbacks execute on arbitrary threads
- SwiftUI requires UI updates on main thread
- Apple's recommended pattern for cross-thread UI updates

**Implementation:**
```swift
class EventHandler: AlephEventHandler {
    func onStateChanged(state: ProcessingState) {
        DispatchQueue.main.async {
            // Update UI safely
        }
    }
}
```

**Trade-offs:**
- ✅ Pro: Thread-safe UI updates
- ✅ Pro: Standard Apple pattern
- ⚠️ Con: Slight latency from thread hop (~1-2ms)

### Decision 4: Separate AppDelegate and HaloWindow

**Rationale:**
- AppDelegate manages menu bar, settings window, lifecycle
- HaloWindow is completely independent (can exist without menu bar)
- Single Responsibility Principle
- Easier to test in isolation

**Architecture:**
```
AppDelegate (NSApplicationDelegate)
├── Menu Bar (NSStatusItem)
├── Settings Window (NSWindow with SwiftUI)
└── AlephCore (Rust)
    └── Callbacks → EventHandler
        └── HaloWindow (NSWindow with SwiftUI)
```

**Trade-offs:**
- ✅ Pro: Clear separation of concerns
- ✅ Pro: Testable components
- ⚠️ Con: More files/classes

### Decision 5: Use NSScreen.main for Cursor Position

**Rationale:**
- Need to position Halo at exact mouse cursor location
- NSEvent.mouseLocation provides global coordinates
- NSScreen.main gives screen bounds for boundary checks

**Implementation:**
```swift
let mouseLocation = NSEvent.mouseLocation
let screenFrame = NSScreen.main?.frame ?? .zero
// Position window at mouseLocation, clamped to screenFrame
```

**Trade-offs:**
- ✅ Pro: Accurate cursor tracking
- ✅ Pro: Multi-monitor support
- ⚠️ Con: Must handle edge cases (cursor at screen edge)

## Architecture

### Directory Structure

```
clients/macos/
├── Aleph.xcodeproj/           # Xcode project
├── Sources/
│   ├── AlephApp.swift         # @main entry point
│   ├── AppDelegate.swift       # Menu bar lifecycle
│   ├── EventHandler.swift      # Implements AlephEventHandler
│   ├── HaloWindow.swift        # NSWindow wrapper
│   ├── HaloView.swift          # SwiftUI animation
│   ├── HaloState.swift         # State machine enum
│   ├── SettingsView.swift      # Main settings UI
│   ├── ProvidersView.swift     # AI providers tab
│   ├── RoutingView.swift       # Routing rules tab
│   ├── ShortcutsView.swift     # Hotkey config tab
│   └── Generated/
│       └── AlephFFI.swift     # UniFFI bindings (copied from core/bindings/)
├── Resources/
│   ├── Assets.xcassets/        # App icon, menu bar icons
│   └── Info.plist              # LSUIElement=YES, permissions
├── Frameworks/
│   └── libaethecore.dylib      # Rust library (copied from core/target/release/)
├── Aleph.entitlements         # Accessibility permissions
└── Scripts/
    └── copy_rust_libs.sh       # Build phase script
```

### Component Interaction

```
┌─────────────────────────────────────────────────────────┐
│  User presses Cmd+~                                     │
└────────────────────┬────────────────────────────────────┘
                     ↓
┌─────────────────────────────────────────────────────────┐
│  Rust Core (libaethecore.dylib)                         │
│  - rdev detects hotkey                                  │
│  - Reads clipboard via arboard                          │
│  - Invokes callback: handler.on_hotkey_detected()      │
└────────────────────┬────────────────────────────────────┘
                     ↓ UniFFI FFI
┌─────────────────────────────────────────────────────────┐
│  EventHandler (Swift)                                   │
│  - Receives callback on background thread              │
│  - DispatchQueue.main.async { showHalo() }             │
└────────────────────┬────────────────────────────────────┘
                     ↓
┌─────────────────────────────────────────────────────────┐
│  HaloWindow (NSWindow)                                  │
│  - Get mouse position: NSEvent.mouseLocation           │
│  - Position window at cursor                            │
│  - Set level: .floating (above all apps)               │
│  - Update state: haloView.state = .listening           │
└────────────────────┬────────────────────────────────────┘
                     ↓
┌─────────────────────────────────────────────────────────┐
│  HaloView (SwiftUI)                                     │
│  - Animate pulsing ring                                 │
│  - Transition: listening → processing → success        │
│  - Fade out after 2 seconds                             │
└─────────────────────────────────────────────────────────┘
```

### Halo State Machine

```
                    [Hotkey Detected]
                           ↓
    ┌──────────────────────────────────────┐
    │         Idle (invisible)             │
    └──────────────┬───────────────────────┘
                   ↓
    ┌──────────────────────────────────────┐
    │    Listening (pulsing ring)          │
    │    Duration: 500ms                   │
    └──────────────┬───────────────────────┘
                   ↓
    ┌──────────────────────────────────────┐
    │  Processing (spinning animation)     │
    │  Color: Provider-specific            │
    └──────┬───────────────┬────────────────┘
           │               │
      [Success]        [Error]
           │               │
           ↓               ↓
    ┌──────────┐    ┌──────────┐
    │ Success  │    │  Error   │
    │ (check)  │    │  (X)     │
    └────┬─────┘    └────┬─────┘
         │               │
         └───────┬───────┘
                 ↓
          [Fade out 2s]
                 ↓
          Back to Idle
```

## NSWindow Configuration

### Critical Window Properties

```swift
class HaloWindow: NSWindow {
    override init(...) {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 120, height: 120),
            styleMask: .borderless,  // No title bar, no controls
            backing: .buffered,
            defer: false
        )

        // CRITICAL: These prevent focus theft
        self.level = .floating           // Above all apps
        self.collectionBehavior = [
            .canJoinAllSpaces,           // Visible on all desktops
            .stationary,                 // Don't move with desktop
            .ignoresCycle                // Don't appear in Cmd+Tab
        ]

        // CRITICAL: Transparency and click-through
        self.backgroundColor = .clear
        self.isOpaque = false
        self.hasShadow = false
        self.ignoresMouseEvents = true   // Click-through

        // CRITICAL: Never steal focus
        self.hidesOnDeactivate = false

        // Show window WITHOUT activating
        self.orderFrontRegardless()      // NOT makeKeyAndOrderFront()
    }
}
```

### Why orderFrontRegardless() Not makeKeyAndOrderFront()

| Method | Focus Behavior | Use Case |
|--------|---------------|----------|
| `makeKeyAndOrderFront()` | Steals focus ❌ | Normal windows |
| `orderFrontRegardless()` | Preserves focus ✅ | Halo overlay |

## Settings UI Architecture

### Tab Structure

```swift
struct SettingsView: View {
    @State var selectedTab: SettingsTab = .general

    var body: some View {
        TabView(selection: $selectedTab) {
            GeneralSettingsView().tag(.general)
            ProvidersView().tag(.providers)
            RoutingView().tag(.routing)
            ShortcutsView().tag(.shortcuts)
        }
        .frame(width: 600, height: 500)
    }
}
```

### Phase 2 Simplified UI

**Providers Tab:**
- Read-only list of providers (no editing)
- Show API key status (present/missing)
- "Configure" button opens placeholder alert

**Routing Tab:**
- Read-only list of routing rules
- Show rule pattern and provider
- "Add Rule" button disabled with "Coming Soon" tooltip

**Shortcuts Tab:**
- Display current hotkey (Cmd+~)
- "Change" button disabled

## Permission Handling

### Accessibility Permission Flow

```swift
import ApplicationServices

class PermissionManager {
    static func checkAccessibility() -> Bool {
        AXIsProcessTrusted()
    }

    static func requestAccessibility() {
        let options = [
            kAXTrustedCheckOptionPrompt.takeUnretainedValue(): true
        ] as CFDictionary
        AXIsProcessTrustedWithOptions(options)
    }
}
```

### Permission Prompt Timing

1. **On Launch**: Check if Accessibility permission granted
2. **If Missing**: Show alert explaining why it's needed
3. **User Clicks "Grant"**: Call `requestAccessibility()` → Opens System Settings
4. **Polling**: Check permission status every 2 seconds
5. **Granted**: Start Rust core listening

## Testing Strategy

### Manual Testing Checklist

- [ ] Menu bar icon appears and responds to clicks
- [ ] Settings window opens and displays tabs
- [ ] Halo appears at cursor location when hotkey pressed
- [ ] Halo animation transitions smoothly
- [ ] Halo never steals focus from active app
- [ ] Halo is click-through (mouse events pass to app below)
- [ ] Halo works across multiple monitors
- [ ] Permission prompt shows on first launch
- [ ] App quits cleanly
- [ ] App survives Rust callback errors gracefully

### XCTest Coverage

```swift
class HaloStateTests: XCTestCase {
    func testStateTransitions() {
        let view = HaloView()

        view.state = .listening
        XCTAssertEqual(view.state, .listening)

        view.state = .processing(providerColor: .green)
        // Assert animation properties
    }
}
```

## Risks / Trade-offs

### Risk 1: Focus Theft from NSWindow

**Issue:** Incorrectly configured NSWindow will steal focus, breaking core UX.

**Mitigation:**
- Use `orderFrontRegardless()` not `makeKeyAndOrderFront()`
- Set `ignoresMouseEvents = true`
- Set `collectionBehavior = .ignoresCycle`
- Manual testing across multiple apps (Safari, VSCode, WeChat)

### Risk 2: Cursor Position on Multi-Monitor Setup

**Issue:** NSEvent.mouseLocation may return incorrect coordinates on multi-monitor.

**Mitigation:**
- Test on dual-monitor setup
- Clamp window position to current screen bounds
- Use `NSScreen.screens` to find screen containing cursor

### Risk 3: UniFFI Callback Threading

**Issue:** Rust callbacks execute on background thread, SwiftUI updates require main thread.

**Mitigation:**
- Always use `DispatchQueue.main.async` in event handler
- Add thread safety assertions in debug builds
- Test rapid callback firing (spam hotkey)

### Risk 4: Permission Rejection

**Issue:** User denies Accessibility permission, app becomes non-functional.

**Mitigation:**
- Show persistent alert explaining why permission is required
- Provide "Open System Settings" button
- Disable hotkey detection gracefully (show error in menu bar)

## Open Questions

1. **Halo Size:** Should Halo be fixed 120x120 or scale based on content length?
   - **Recommendation:** Fixed size for Phase 2, dynamic in Phase 3

2. **Animation Duration:** How long should success/error states display?
   - **Recommendation:** 1.5 seconds, then fade out over 0.5s

3. **Error Handling:** What should Halo show if AI API fails?
   - **Recommendation:** Red X icon with shake animation

4. **Menu Bar Icon:** Should it reflect Aleph state (idle/listening/processing)?
   - **Recommendation:** Yes, use SF Symbols with color tint

## Success Criteria

✅ **Phase 2 is successful when:**
1. User can launch Aleph.app and see menu bar icon
2. Pressing Cmd+~ shows Halo at cursor location
3. Halo animates through states smoothly
4. Halo never steals focus from active application
5. Settings window opens and displays all tabs
6. Permission prompt shows on first launch
7. App runs without crashes for 30 minutes of normal use
8. App builds and runs on fresh macOS 13+ system
