# Design: Simplify Halo Visual System

## Overview

This document details the architectural changes for simplifying the Halo visual system by removing multi-theme support and streamlining the processing indicator.

## Current Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Theme System                          │
├─────────────────────────────────────────────────────────┤
│  Theme (enum)                                            │
│  ├── .cyberpunk → CyberpunkTheme : HaloTheme            │
│  ├── .zen → ZenTheme : HaloTheme                        │
│  └── .jarvis → JarvisTheme : HaloTheme                  │
│                                                          │
│  ThemeEngine (ObservableObject)                         │
│  ├── selectedTheme: Theme                               │
│  ├── activeTheme: any HaloTheme                         │
│  └── saveThemePreference()                              │
│                                                          │
│  HaloTheme (protocol)                                   │
│  ├── listeningView() → AnyView                         │
│  ├── processingView() → AnyView                        │
│  ├── processingWithAIView() → AnyView                  │
│  ├── successView() → AnyView                           │
│  ├── errorView() → AnyView                             │
│  ├── typewritingView() → AnyView                       │
│  └── toastView() → AnyView                             │
└─────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────┐
│                    HaloWindow                            │
├─────────────────────────────────────────────────────────┤
│  themeEngine: ThemeEngine                               │
│  viewModel: HaloViewModel                               │
│                                                          │
│  HaloView                                               │
│  ├── Uses themeEngine.activeTheme                      │
│  └── Calls theme.xxxView() based on HaloState          │
└─────────────────────────────────────────────────────────┘
```

## Target Architecture

```
┌─────────────────────────────────────────────────────────┐
│                  Unified Halo Views                      │
├─────────────────────────────────────────────────────────┤
│  HaloViews.swift (renamed from Theme.swift)             │
│                                                          │
│  struct HaloListeningView: View { ... }                 │
│  struct HaloProcessingView: View { ... }                │
│  struct HaloTypewritingView: View { ... }               │
│  struct HaloErrorView: View { ... }                     │
│  struct HaloToastView: View { ... } (existing)          │
│  // NO successView - removed                            │
└─────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────┐
│                    HaloWindow                            │
├─────────────────────────────────────────────────────────┤
│  // No themeEngine dependency                           │
│  viewModel: HaloViewModel                               │
│                                                          │
│  HaloView                                               │
│  ├── Switch on HaloState                               │
│  └── Return unified view directly                      │
└─────────────────────────────────────────────────────────┘
```

## State Machine Changes

### Current HaloState

```swift
enum HaloState: Equatable {
    case idle
    case listening
    case retrievingMemory
    case processingWithAI(providerColor: Color, providerName: String?)
    case processing(providerColor: Color, streamingText: String?)
    case typewriting(progress: Float)
    case success(finalText: String?)  // TO BE REMOVED
    case error(type: ErrorType, message: String, suggestion: String?)
    case permissionRequired(type: PermissionType)
    case toast(type: ToastType, title: String, message: String, autoDismiss: Bool)
    case clarification(request: ClarificationRequest)
    case conversationInput(sessionId: String, turnCount: UInt32)
    case toolConfirmation(...)
}
```

### New HaloState

```swift
enum HaloState: Equatable {
    case idle
    case listening
    case retrievingMemory  // Consider merging with processing
    case processing(streamingText: String?)  // Simplified, no providerColor
    case typewriting(progress: Float)
    // success REMOVED
    case error(type: ErrorType, message: String, suggestion: String?)
    case permissionRequired(type: PermissionType)
    case toast(type: ToastType, title: String, message: String, autoDismiss: Bool)
    case clarification(request: ClarificationRequest)
    case conversationInput(sessionId: String, turnCount: UInt32)
    case toolConfirmation(...)
}
```

**Note:** The `providerColor` parameter in processing states was used to show provider-specific colors. Since we're removing themes, we can simplify to a single processing color (purple).

## Processing Indicator Design

### Visual Specification

```
┌───────────┐
│  ╭───╮    │  16 x 16 px
│ ╱     ╲   │  Rotating arc (270° sweep)
│ ╲     ╱   │  Purple color (Color.purple)
│  ╰───╯    │  1 second rotation
└───────────┘
```

### SwiftUI Implementation

```swift
struct HaloProcessingView: View {
    @State private var rotation: Double = 0

    var body: some View {
        Circle()
            .trim(from: 0, to: 0.75)
            .stroke(
                Color.purple,
                style: StrokeStyle(lineWidth: 2, lineCap: .round)
            )
            .frame(width: 16, height: 16)
            .rotationEffect(.degrees(rotation))
            .onAppear {
                withAnimation(.linear(duration: 1).repeatForever(autoreverses: false)) {
                    rotation = 360
                }
            }
    }
}
```

## Position Tracking Logic

### Current Flow

```
Hotkey pressed
    ↓
Fixed position calculation
    ↓
Show Halo
```

### New Flow

```
Hotkey pressed
    ↓
CaretPositionHelper.getBestPosition()
    ├── Try Accessibility API for caret position
    │   └── Validate position (not 0,0, within screen bounds)
    └── If invalid → NSEvent.mouseLocation
    ↓
Show Halo at position
```

### Validation Logic

The existing `CaretPositionHelper.isValidScreenPosition()` already handles:
- Position x < 10 → Invalid (some apps return near-zero)
- Position y < 10 → Invalid
- Position outside all screens → Invalid

This ensures WeChat and similar apps that return invalid coordinates automatically fall back to mouse position.

## File Deletion Summary

| File | Lines | Purpose |
|------|-------|---------|
| `ZenTheme.swift` | 355 | Zen theme views |
| `CyberpunkTheme.swift` | 409 | Cyberpunk theme views |
| `JarvisTheme.swift` | 486 | Jarvis theme views |
| `GlitchOverlay.swift` | ~100 | Cyberpunk effect |
| `HexSegment.swift` | ~50 | Jarvis shape |
| `ThemeEngine.swift` | 59 | Theme management |
| **Total** | **~1459** | Lines removed |

## HaloWindow Deletion Evaluation

The current `HaloWindow.swift` (~660 lines) and related components should be evaluated for deletion:

### Components to Evaluate

| File | Lines | Keep/Delete | Reason |
|------|-------|-------------|--------|
| `HaloWindow.swift` | 660 | **Delete** | Replace with minimal ProcessingIndicatorWindow |
| `HaloView.swift` | 420 | **Delete** | Views inlined into ProcessingIndicatorWindow |
| `HaloWindowController.swift` | ~200 | **Delete** | No longer needed |
| `HaloViewModel` (in HaloWindow) | ~40 | **Delete** | State simplified |
| `Theme.swift` | 266 | **Delete** | Protocol no longer needed |

### New Minimal Component

Replace all of the above with a single `ProcessingIndicatorWindow.swift` (~100 lines):

```swift
/// Minimal processing indicator - 16x16 rotating arc
class ProcessingIndicatorWindow: NSWindow {
    private var rotation: Double = 0
    private var displayLink: CVDisplayLink?

    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 24, height: 24),  // 16px + padding
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )

        level = .floating
        backgroundColor = .clear
        isOpaque = false
        ignoresMouseEvents = true
        hasShadow = false

        // Setup simple spinner view
        let hostingView = NSHostingView(rootView: SpinnerView())
        contentView = hostingView
    }

    func show(at position: NSPoint) {
        setFrameOrigin(NSPoint(x: position.x - 12, y: position.y - 12))
        orderFrontRegardless()
        alphaValue = 1.0
    }

    func hide() {
        alphaValue = 0
        orderOut(nil)
    }
}

private struct SpinnerView: View {
    @State private var rotation: Double = 0

    var body: some View {
        Circle()
            .trim(from: 0, to: 0.75)
            .stroke(Color.purple, style: StrokeStyle(lineWidth: 2, lineCap: .round))
            .frame(width: 16, height: 16)
            .rotationEffect(.degrees(rotation))
            .onAppear {
                withAnimation(.linear(duration: 1).repeatForever(autoreverses: false)) {
                    rotation = 360
                }
            }
    }
}
```

### State Simplification

Before (HaloState with 12 cases):
```swift
enum HaloState {
    case idle, listening, retrievingMemory, processingWithAI, processing,
         typewriting, success, error, permissionRequired, toast,
         clarification, conversationInput, toolConfirmation
}
```

After (processing only needs 2 states for single-turn):
```swift
// ProcessingIndicatorWindow has no enum - just show/hide
// Multi-turn mode uses UnifiedInputWindow with SubPanel (unchanged)
```

### Impact on Other Components

| Component | Change Required |
|-----------|----------------|
| `AppDelegate` | Replace `haloWindow` with `processingIndicator` |
| `EventHandler` | Update callbacks to use new API |
| `UnifiedInputWindow` | No change (handles multi-turn) |
| `SubPanelView` | No change (handles CLI output) |

## Migration Notes

### ThemeManager vs ThemeEngine

There are two "theme" systems in the codebase:

1. **ThemeManager** (`DesignSystem/ThemeManager.swift`)
   - Manages **app-level appearance** (Light/Dark/Auto)
   - Controls NSApp.appearance
   - Used by ThemeSwitcher in settings
   - **KEEP THIS**

2. **ThemeEngine** (`Themes/ThemeEngine.swift`)
   - Manages **Halo visual themes** (Cyberpunk/Zen/Jarvis)
   - Used by HaloWindow/HaloView
   - **DELETE THIS**

### Settings UI Changes

Remove the theme selector section from `SettingsView.swift`:

```swift
// BEFORE
GroupBox {
    VStack {
        Text("Halo Theme")
        ForEach(Theme.allCases, id: \.self) { theme in
            // Theme selection buttons
        }
    }
}

// AFTER
// Remove entire GroupBox
```

### Rust Core Impact

The Rust core sends `on_halo_success` callback. This needs to be:
1. Changed to directly transition to idle, OR
2. The Swift side ignores success callbacks

Reviewing `EventHandler.swift` will determine the exact approach.

## Testing Strategy

### Manual Tests

1. **Single-turn processing:**
   - Select text in TextEdit → Press hotkey → Verify spinner at cursor
   - Select text in WeChat → Press hotkey → Verify spinner at mouse (fallback)

2. **Multi-turn conversation:**
   - Enter conversation mode → Type message → Verify SubPanel shows
   - Verify CLI output during processing
   - Verify ESC dismisses

3. **Error handling:**
   - Disconnect network → Send request → Verify error toast
   - Test retry button

### Automated Tests

No new automated tests required. Existing UI tests should pass after changes.

## Rollback Plan

If issues arise, revert commits and restore deleted files from git history:

```bash
git checkout HEAD~1 -- Aether/Sources/Themes/
```

Since all changes are deletions or simplifications, rollback is straightforward.
