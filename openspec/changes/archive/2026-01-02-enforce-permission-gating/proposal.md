# Change: Enforce Mandatory Permission Gating at Startup

## Metadata
- **ID**: enforce-permission-gating
- **Title**: Enforce Mandatory Permission Gating at Startup
- **Type**: Feature Addition / UX Improvement
- **Status**: Deployed
- **Created**: 2025-12-29
- **Deployed**: 2025-12-29

## Why

Aether requires both **Accessibility** and **Input Monitoring** permissions to function correctly. Currently, users can dismiss permission prompts and access settings without granting these permissions, leading to a broken experience where core functionality (global hotkeys, clipboard operations) fails silently.

Without mandatory permission gating, we face:
- Users attempting to use features that require permissions but haven't granted them
- Confusion when hotkeys don't work but no clear indication why
- Support burden from users who skip permission setup
- Inconsistent app state where some features work and others don't

This change enforces a blocking permission setup flow at app launch, ensuring users cannot proceed to settings or use any features until both required permissions are granted.

## What Changes

- Introduce a **mandatory permission gating screen** that appears on app launch
- Block access to settings window until BOTH permissions are granted
- Implement real-time permission status monitoring with automatic progression
- Add visual permission status indicators (pending → granted)
- Prevent dismissal of permission gate until requirements are met
- Integrate with existing PermissionPromptView component
- Add Input Monitoring permission check (currently only Accessibility is checked)

**Deliverables:**
- New `PermissionGateView.swift` component with two-step permission flow
- Updated `AppDelegate.swift` to show permission gate instead of main settings on first launch
- New `PermissionStatusMonitor` class to poll permission status in real-time
- Updated Info.plist with Input Monitoring usage description
- Integration tests for permission flow
- User documentation for permission requirements

**Key Behaviors:**
1. On app launch, check both Accessibility and Input Monitoring permissions
2. If ANY permission is missing, show PermissionGateView (non-dismissible)
3. User clicks "Open System Settings" for each permission sequentially
4. App polls permission status every 1 second
5. When permission is granted, automatically move to next step or dismiss gate
6. Only after BOTH permissions granted, allow access to settings and core features

**Out of Scope (Future Proposals):**
- Permission revocation detection during runtime (currently only handles startup)
- Granular permission fallback modes (e.g., read-only mode without Input Monitoring)
- Background permission monitoring service
- Permission re-request automation (requires system dialog)

## Impact

**Affected specs:**
- **MODIFIED**: `macos-client` - Add permission gating requirement at launch
- **NEW**: `permission-gating` - Mandatory permission setup flow
- **MODIFIED**: `event-handler` - Add permission status monitoring callbacks

**Affected code:**
- `Aether/Sources/AppDelegate.swift` - Add permission gate logic before settings window
- `Aether/Sources/Components/PermissionPromptView.swift` - Reuse for individual permission prompts
- `Aether/Sources/AetherApp.swift` - May need to handle permission gate window
- `Aether/Info.plist` - Add NSAppleEventsUsageDescription for Input Monitoring
- **NEW**: `Aether/Sources/Components/PermissionGateView.swift`
- **NEW**: `Aether/Sources/Utils/PermissionStatusMonitor.swift`

**Dependencies:**
- macOS 13+ (for IOHIDRequestAccess API for Input Monitoring)
- Existing PermissionPromptView component
- ContextCapture module for Accessibility permission check

**Breaking changes:**
- Users will be BLOCKED from using app until permissions granted (intentional UX change)
- First-launch experience changes from optional prompt to mandatory gate
- Existing users may see permission gate on next launch if they previously skipped permissions

**Migration:**
- Existing users who have already granted permissions: No change, app proceeds normally
- Users who skipped permissions: Will see mandatory gate on next launch
- New users: Will see mandatory gate immediately on first launch
