# Proposal: Simplify Halo Visual System

## Change ID
`simplify-halo-visual-system`

## Summary
1. Remove all multi-theme support (Cyberpunk, Zen, Jarvis) - use a single, unified Halo visual style
2. Simplify single-turn mode: use a minimal thinking spinner that tracks cursor/mouse position
3. Remove success state icon - AI response completes silently
4. Preserve multi-turn conversation mode with existing SubPanel display logic

## Problem Statement

### Issue 1: Theme Complexity Without User Value
The current Halo system supports three visual themes (Cyberpunk, Zen, Jarvis), each with 700+ lines of custom view code. However:
- Users rarely change themes after initial setup
- Theme maintenance creates significant code overhead
- The visual differences are subtle and don't provide functional value
- Theme-related bugs require fixes across multiple files

**Current Theme Files:**
- `Themes/Theme.swift` (266 lines) - HaloTheme protocol
- `Themes/ThemeEngine.swift` (59 lines) - Theme management
- `Themes/ZenTheme.swift` (355 lines) - Zen theme views
- `Themes/CyberpunkTheme.swift` (409 lines) - Cyberpunk theme views
- `Themes/JarvisTheme.swift` (486 lines) - Jarvis theme views
- `Themes/Effects/GlitchOverlay.swift` - Cyberpunk effects
- `Themes/Shapes/HexSegment.swift` - Jarvis hex shapes

### Issue 2: Success State Icon is Unnecessary
After AI processing completes:
- Single-turn mode: Result is immediately pasted to the target app
- Multi-turn mode: Result appears in SubPanel CLI output

In both cases, showing a "success checkmark" icon provides no value:
- The success is already visible through the result itself
- The icon adds an extra visual step that slows down the interaction
- Users don't need confirmation that the paste succeeded

### Issue 3: Processing Indicator Needs Better Position Tracking
Current behavior lacks clear visual feedback during AI processing:
- The Halo appears at a fixed position
- Some apps support cursor position tracking, others don't

**Desired Behavior:**
- Use Accessibility API to track text caret position
- Fall back to mouse position when caret is unavailable
- Show a minimal, unobtrusive thinking spinner

## Proposed Solution

### Part 1: Remove Multi-Theme System

Delete all theme-specific implementations and use a single, hardcoded visual style:

**Delete Files:**
- `Themes/ZenTheme.swift`
- `Themes/CyberpunkTheme.swift`
- `Themes/JarvisTheme.swift`
- `Themes/Effects/GlitchOverlay.swift`
- `Themes/Shapes/HexSegment.swift`
- `Themes/ThemeEngine.swift` (optional - can keep for Light/Dark mode)

**Simplify:**
- `Themes/Theme.swift` → Convert to simple utility with default views
- Remove theme selection from `SettingsView.swift`
- Remove `ThemeEngine` dependency from `HaloWindow` and `HaloView`

**New Unified Style:**
- Processing: Simple rotating arc spinner (like SF Symbols "progress.indicator")
- Error: Red icon with action buttons (keep current ErrorActionView)
- Toast: Keep current HaloToastView design

### Part 2: Remove Success State

Modify `HaloState` to remove the success case:
```swift
enum HaloState: Equatable {
    case idle
    case listening
    case processing(providerColor: Color, streamingText: String?)
    case typewriting(progress: Float)
    // case success(finalText: String?)  // REMOVE
    case error(type: ErrorType, message: String, suggestion: String?)
    case toast(...)
    case conversationInput(...)
    // ... other cases
}
```

**Behavior Change:**
- After AI response completes → Immediately transition to `idle`
- No success animation or delay
- Result is visible through the paste or SubPanel output

### Part 3: Smart Position Tracking for Processing Indicator

**Position Detection Priority:**
1. Try `CaretPositionHelper.getBestPosition()` (uses Accessibility API)
2. If caret position is invalid (some apps like WeChat return 0,0), fall back to mouse position
3. The indicator follows the determined position

**Minimal Thinking Spinner:**
- Tiny (16x16 px) rotating arc
- Uses purple color (Aleph brand color)
- No text, no progress bar
- Click-through (ignoresMouseEvents = true)

## Scope

### In Scope
- Theme file deletion and cleanup
- Success state removal
- Processing indicator with smart positioning
- Single-turn mode simplification

### Out of Scope
- Multi-turn conversation window behavior (preserve existing logic)
- SubPanel CLI output styling (preserve existing logic)
- Error handling and retry logic (preserve existing logic)
- System theme (Light/Dark/Auto) - keep ThemeManager for app-level appearance

## Impact Assessment

### Files to Delete
- `Aleph/Sources/Themes/ZenTheme.swift`
- `Aleph/Sources/Themes/CyberpunkTheme.swift`
- `Aleph/Sources/Themes/JarvisTheme.swift`
- `Aleph/Sources/Themes/Effects/GlitchOverlay.swift`
- `Aleph/Sources/Themes/Shapes/HexSegment.swift`
- `Aleph/Sources/Themes/ThemeEngine.swift`

### Files to Evaluate for Deletion
- `Aleph/Sources/HaloWindow.swift` - Evaluate if HaloWindow is still needed after simplification
- `Aleph/Sources/HaloView.swift` - May be merged into a simpler component
- `Aleph/Sources/Controllers/HaloWindowController.swift` - Delete if HaloWindow is removed

### Files to Modify Significantly
- `Aleph/Sources/Themes/Theme.swift` - Remove protocol, add unified views (or delete entirely)
- `Aleph/Sources/HaloState.swift` - Remove success case

### Files to Modify Minor
- `Aleph/Sources/SettingsView.swift` - Remove theme selector
- `Aleph/Sources/DI/DependencyContainer.swift` - Remove ThemeEngine if unused
- `Aleph/Sources/AppDelegate.swift` - Update ThemeEngine references

### Risk Level
**Medium** - Removes significant code but changes are primarily deletions. Careful testing needed for state transitions.

## Relationship to Existing Changes

This proposal **supersedes** parts of `enhance-processing-indicator-and-multiturn-visibility`:
- Both address processing indicator positioning
- This proposal has a simpler scope (no multi-turn visibility settings)
- Multi-turn mode behavior is preserved as-is

## Success Criteria
1. All theme files deleted, no compilation errors
2. Processing indicator appears at cursor position (or mouse fallback)
3. No success icon shown after AI response
4. Multi-turn mode continues to work with SubPanel CLI output
5. ESC key dismisses Halo in all states
