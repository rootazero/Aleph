# Design: Halo Toast Notification System

## Overview

This document describes the architecture and visual design for replacing system NSAlert dialogs with a Halo-based toast notification system.

## Architecture

### Component Hierarchy

```
HaloWindow (NSWindow)
    └── NSHostingView
        └── HaloView (SwiftUI)
            ├── ... existing states ...
            └── HaloToastView (NEW)
                ├── ToastIcon
                ├── ToastContent (title + message)
                └── CloseButton
```

### State Machine Extension

```swift
enum ToastType {
    case info      // Blue accent, info.circle icon
    case warning   // Orange accent, exclamationmark.triangle icon
    case error     // Red accent, xmark.circle icon
}

enum HaloState {
    // ... existing cases ...

    /// Toast notification overlay
    case toast(
        type: ToastType,
        title: String,
        message: String,
        autoDismiss: Bool,
        onDismiss: (() -> Void)?
    )
}
```

### EventHandler Protocol Extension

```swift
protocol AlephEventHandler {
    // ... existing methods ...

    /// Show toast notification
    func showToast(type: ToastType, title: String, message: String, autoDismiss: Bool)

    /// Dismiss current toast
    func dismissToast()
}
```

## Visual Design Specification

### Layout

```
┌─────────────────────────────────────────────────────────┐
│                                                    [✕]  │  <- Close button (16x16)
│   🔵  Title Text                                        │  <- Icon (24x24) + Title
│                                                         │
│       Message text that can wrap to multiple lines      │  <- Message (up to 5 lines)
│       if needed based on content length.                │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

### Dimensions

| Property | Value | Notes |
|----------|-------|-------|
| Min Width | 200px | For short messages |
| Max Width | 400px | Text wraps beyond this |
| Min Height | 80px | Title + short message |
| Max Height | ~200px | Title + 5 lines of message |
| Corner Radius | 12px | Consistent with design system |
| Padding | 16px | All sides |
| Icon Size | 24x24 | SF Symbols |
| Close Button | 16x16 | Subtle but clickable |

### Colors

| Element | Light Mode | Dark Mode |
|---------|------------|-----------|
| Background | rgba(255, 255, 255, 0.90) | rgba(40, 40, 40, 0.90) |
| Title | #1a1a1a | #ffffff |
| Message | #666666 | #aaaaaa |
| Close Button | #999999 | #666666 |
| Close Hover | #333333 | #ffffff |
| Shadow | rgba(0, 0, 0, 0.15) | rgba(0, 0, 0, 0.30) |

### Type-Specific Accent Colors

| ToastType | Accent Color | Icon |
|-----------|--------------|------|
| Info | #007AFF (Blue) | info.circle.fill |
| Warning | #FF9500 (Orange) | exclamationmark.triangle.fill |
| Error | #FF3B30 (Red) | xmark.circle.fill |

### Animation

**Appearance:**
```swift
.transition(
    .asymmetric(
        insertion: .scale(scale: 0.9).combined(with: .opacity),
        removal: .opacity
    )
)
.animation(.spring(response: 0.3, dampingFraction: 0.8))
```

**Auto-dismiss Timer:**
- Info toast: 3 seconds (configurable)
- Warning/Error toast: No auto-dismiss (user must click close)

## Positioning Strategy

Unlike Halo overlay (which appears at cursor), toast should appear at a fixed position for predictability:

**Option A: Top-center of screen**
```
┌──────────────────────────────────────┐
│         ┌──────────────┐             │
│         │    Toast     │             │
│         └──────────────┘             │
│                                      │
│                                      │
│                                      │
└──────────────────────────────────────┘
```

**Option B: Center of screen** (Recommended)
```
┌──────────────────────────────────────┐
│                                      │
│                                      │
│         ┌──────────────┐             │
│         │    Toast     │             │
│         └──────────────┘             │
│                                      │
│                                      │
└──────────────────────────────────────┘
```

**Recommendation**: Center of screen, same positioning as existing Halo. This maintains consistency and ensures visibility on all screen sizes.

## Close Button Design

### Visual Specification

```
┌─────┐
│  ✕  │  16x16px circle
└─────┘

- Normal: 50% opacity
- Hover: 100% opacity + slight scale (1.1x)
- Click: 90% scale + accent color
```

### SwiftUI Implementation Concept

```swift
struct ToastCloseButton: View {
    @State private var isHovered = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark")
                .font(.system(size: 10, weight: .medium))
                .foregroundColor(.secondary)
                .frame(width: 16, height: 16)
                .background(
                    Circle()
                        .fill(isHovered ? Color.gray.opacity(0.2) : Color.clear)
                )
        }
        .buttonStyle(.plain)
        .scaleEffect(isHovered ? 1.1 : 1.0)
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.15)) {
                isHovered = hovering
            }
        }
    }
}
```

## Accessibility

### VoiceOver Announcements

```swift
.accessibilityElement(children: .contain)
.accessibilityLabel("\(type.displayName): \(title)")
.accessibilityValue(message)
.accessibilityHint("Press close button to dismiss")
.accessibilityAddTraits(.isStaticText)
```

### Keyboard Support

- Close button should be focusable
- Escape key should dismiss toast
- Tab should move focus to close button if toast is showing

## Theme Integration

Each theme can customize toast appearance while maintaining the core layout:

```swift
protocol HaloTheme {
    // ... existing methods ...

    @ViewBuilder func toastView(
        type: ToastType,
        title: String,
        message: String,
        onDismiss: (() -> Void)?
    ) -> AnyView
}
```

Default implementation provides the standard light background toast. Themes can override for unique styling (e.g., Cyberpunk might add subtle glow effects).

## Migration Strategy

### Before (Current)
```swift
showInfoAlert(title: "Export Complete", message: "Rules exported successfully.")
```

### After (New)
```swift
eventHandler?.showToast(
    type: .info,
    title: "Export Complete",
    message: "Rules exported successfully.",
    autoDismiss: true
)
```

### Fallback Handling

For alerts that may occur before HaloWindow is initialized (e.g., core initialization failure), keep AlertHelper as fallback:

```swift
func showToastOrAlert(type: ToastType, title: String, message: String) {
    if let eventHandler = eventHandler, haloWindow != nil {
        eventHandler.showToast(type: type, title: title, message: message, autoDismiss: type == .info)
    } else {
        // Fallback to system alert
        switch type {
        case .info:
            showInfoAlert(title: title, message: message)
        case .warning:
            showWarningAlert(title: title, message: message)
        case .error:
            showErrorAlert(title: title, message: message)
        }
    }
}
```

## Edge Cases

1. **Multiple Toasts**: Queue subsequent toasts, show one at a time
2. **Very Long Messages**: Truncate at 5 lines with ellipsis
3. **Empty Title**: Use type display name as fallback
4. **Empty Message**: Show title only, reduce height
5. **Rapid Dismissal**: Cancel auto-dismiss timer on manual dismiss
