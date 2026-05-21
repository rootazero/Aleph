# Proposal: Replace System Alerts with Halo Toast

## Status: Deployed

## Summary

Replace all system NSAlert dialogs (showInfoAlert, showWarningAlert, showErrorAlert) with a new Halo-based toast notification system. The new toast will:

1. Display in a floating window at cursor position (like existing Halo)
2. Use a light, semi-transparent background for readability
3. Dynamically resize based on message content length
4. Feature a small, elegant close button
5. Maintain the "Ghost" aesthetic (no focus stealing)

## Motivation

Current system alerts (NSAlert) break the "Ghost" aesthetic by:
- Stealing focus from the active application
- Creating a jarring modal experience
- Looking out of place with the native macOS system dialogs

A Halo-based toast will:
- Maintain visual consistency with existing Halo overlay
- Preserve focus in the user's current application
- Provide a more elegant, non-intrusive notification experience
- Support the "invisible first" design philosophy

## Scope

### In Scope

1. **New HaloToast Component**: Create a new SwiftUI view for toast notifications
2. **Toast State in HaloState**: Add new toast state case to existing HaloState enum
3. **Toast Styling**: Light/semi-transparent background, dynamic sizing, elegant close button
4. **AlertHelper Replacement**: Refactor showInfoAlert/showWarningAlert/showErrorAlert to use HaloToast
5. **Auto-dismiss**: Optional auto-dismiss timer for informational toasts
6. **Theme Support**: Integrate with existing ThemeEngine for consistent styling

### Out of Scope

- Confirmation dialogs with multiple choices (e.g., Append/Replace/Cancel) - these require user selection
- System-level alerts that appear before Halo is initialized
- Permission prompt dialogs (already using PermissionPromptView)

## Design

### Toast Types

| Type | Icon | Background | Use Case |
|------|------|------------|----------|
| Info | info.circle | Blue tint | Success messages, confirmations |
| Warning | exclamationmark.triangle | Orange tint | Non-critical warnings |
| Error | xmark.circle | Red tint | Errors requiring acknowledgment |

### Visual Design

```
┌────────────────────────────────────────────┐
│ ⓘ  Export Successful                    ✕ │
│                                            │
│    Your rules have been exported to        │
│    routing-rules.json                      │
└────────────────────────────────────────────┘
```

- **Background**: Light (85% white) with 90% opacity, backdrop blur
- **Corner Radius**: 12px
- **Shadow**: Soft drop shadow for depth
- **Close Button**: 16x16px, subtle, top-right corner
- **Max Width**: 400px
- **Min Width**: 200px
- **Padding**: 16px

### Dynamic Sizing

Toast width and height adapt to content:
- Title: Always single line, bold
- Message: Wraps at max width, up to 5 lines
- Height calculated based on actual text content

### Behavior

1. **Appearance**: Fade-in with scale animation (0.3s)
2. **Position**: Centered on screen (not cursor, to avoid covering user's work)
3. **Dismissal**:
   - Click close button
   - Auto-dismiss after 3s for Info type (optional)
   - Manual dismiss required for Warning/Error
4. **Focus**: Never steal focus (ignoresMouseEvents: false for close button area only)

## Migration Plan

### Phase 1: Create Toast Infrastructure

1. Add `toast` case to HaloState enum
2. Create HaloToastView component
3. Add toast methods to EventHandler protocol
4. Update HaloWindow to handle toast state

### Phase 2: Replace Alert Calls

Replace in order of impact:

| File | Function | Toast Type |
|------|----------|------------|
| AppDelegate.swift | showAbout | Info |
| AppDelegate.swift | showWarningAlert (provider) | Warning |
| AppDelegate.swift | showWarningAlert (input mode) | Warning |
| AppDelegate.swift | showErrorAlert (core init) | Error |
| AppDelegate.swift | showWarningAlert (file size) | Warning |
| RoutingView.swift | export success | Info |
| RoutingView.swift | import success (append) | Info |
| RoutingView.swift | import success (replace) | Info |

### Phase 3: Remove AlertHelper

After all usages are migrated, deprecate/remove AlertHelper.swift.

## Risks

1. **Multi-choice Dialogs**: RoutingView import options dialog requires user choice (Append/Replace/Cancel). This will remain as NSAlert for now.
2. **Pre-initialization Alerts**: Alerts shown before HaloWindow is created need fallback handling.
3. **Accessibility**: Must ensure screen readers can announce toast content.

## Alternatives Considered

1. **System Notifications (NSUserNotification)**: Requires user permission, not immediate
2. **Keep NSAlert**: Breaks "Ghost" aesthetic
3. **Floating Panel**: More complex, similar to toast but heavier

## Dependencies

- Existing HaloWindow infrastructure
- ThemeEngine for styling consistency
- EventHandler protocol for Swift-Rust communication

## Success Criteria

1. All applicable system alerts replaced with Halo toast
2. Toast appearance matches design spec
3. No focus stealing occurs
4. Dynamic sizing works correctly for all message lengths
5. Close button is small but easily clickable
6. Accessibility labels are present
