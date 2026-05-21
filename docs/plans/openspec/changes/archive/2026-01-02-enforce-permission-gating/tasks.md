# Implementation Tasks

## 1. Input Monitoring Permission Detection
- [x] 1.1 Research macOS Input Monitoring permission check API (IOHIDRequestAccess)
- [x] 1.2 Add Info.plist entry for Input Monitoring usage description
- [x] 1.3 Create `PermissionChecker` utility class in Swift
- [x] 1.4 Implement `checkInputMonitoringPermission()` method
- [x] 1.5 Add unit tests for permission checker
- [x] 1.6 Test permission detection on macOS 13+

## 2. Permission Status Monitor
- [x] 2.1 Create `Aleph/Sources/Utils/PermissionStatusMonitor.swift`
- [x] 2.2 Implement timer-based polling (1 second interval)
- [x] 2.3 Add methods: `startMonitoring()`, `stopMonitoring()`
- [x] 2.4 Implement callback for permission status changes
- [x] 2.5 Add property to track current permission state
- [x] 2.6 Write unit tests for monitor lifecycle
- [x] 2.7 Test memory management (no retain cycles)

## 3. Permission Gate View Component
- [x] 3.1 Create `Aleph/Sources/Components/PermissionGateView.swift`
- [x] 3.2 Design two-step UI layout (Accessibility → Input Monitoring)
- [x] 3.3 Implement step indicator (Step 1 of 2, Step 2 of 2)
- [x] 3.4 Add visual status indicators (pending vs granted)
- [x] 3.5 Integrate PermissionPromptView for each step
- [x] 3.6 Add "Open System Settings" buttons per step
- [x] 3.7 Implement automatic progression when permission granted
- [x] 3.8 Make view non-dismissible (no close button, no Escape key)
- [x] 3.9 Add animations for state transitions
- [x] 3.10 Write SwiftUI Previews for each state

## 4. AppDelegate Integration
- [x] 4.1 Update `applicationDidFinishLaunching` to check both permissions
- [x] 4.2 Add logic to show PermissionGateView if ANY permission missing
- [x] 4.3 Disable settings menu item when permission gate is active
- [x] 4.4 Delay AlephCore initialization until permissions granted
- [x] 4.5 Add PermissionStatusMonitor integration
- [x] 4.6 Implement callback for permission gate dismissal
- [x] 4.7 Initialize core features only after gate dismissal
- [x] 4.8 Update menu bar icon to show "waiting" state during gate

## 5. Settings Menu Blocking
- [x] 5.1 Add `isPermissionGateActive` state property to AppDelegate
- [x] 5.2 Update `showSettings()` to check permission gate state
- [x] 5.3 Disable menu item when gate is active
- [x] 5.4 Add menu item validation logic
- [x] 5.5 Enable menu item after permissions granted
- [x] 5.6 Test menu item state changes in runtime

## 6. Window Management
- [x] 6.1 Create `permissionGateWindow` property in AppDelegate
- [x] 6.2 Implement window creation for PermissionGateView
- [x] 6.3 Configure window as non-closable, always-on-top
- [x] 6.4 Center window on screen
- [x] 6.5 Prevent window from being hidden (override close button)
- [x] 6.6 Add window lifecycle management (show/hide/dismiss)
- [x] 6.7 Test window behavior across different screens

## 7. Permission Prompt Reuse
- [x] 7.1 Update PermissionPromptView to be more reusable
- [x] 7.2 Remove "Skip Later" button from component (handle in parent)
- [x] 7.3 Add binding for permission type switching
- [x] 7.4 Test integration with PermissionGateView
- [x] 7.5 Verify deep linking to System Settings works correctly

## 8. AlephApp Entry Point Update
- [x] 8.1 Review if AlephApp.swift needs changes for window management
- [x] 8.2 Ensure WindowGroup or Settings scene doesn't conflict with permission gate
- [x] 8.3 Test app launch flow with and without permissions
- [x] 8.4 Verify no duplicate windows appear

## 9. Testing and Validation
- [ ] 9.1 Manual test: Launch with no permissions → see gate
- [ ] 9.2 Manual test: Grant Accessibility → auto-progress to Input Monitoring
- [ ] 9.3 Manual test: Grant Input Monitoring → gate dismisses
- [ ] 9.4 Manual test: Settings menu disabled during gate
- [ ] 9.5 Manual test: Settings menu enabled after gate
- [ ] 9.6 Manual test: Permission status polling works (grant in Settings)
- [ ] 9.7 Manual test: Cannot dismiss gate with Escape or clicking outside
- [ ] 9.8 Manual test: Deep links open correct System Settings pane
- [ ] 9.9 Write UI tests for permission gate flow (if possible)
- [ ] 9.10 Test on macOS 13, 14, 15

## 10. Documentation
- [ ] 10.1 Update CLAUDE.md with permission gate implementation details
- [ ] 10.2 Add section in README about required permissions
- [ ] 10.3 Document PermissionGateView component in code comments
- [ ] 10.4 Add troubleshooting guide for permission issues
- [ ] 10.5 Update Phase tracking in CLAUDE.md (mark as Phase 7.1)
- [ ] 10.6 Document breaking UX change for existing users

## 11. Edge Cases and Error Handling
- [ ] 11.1 Handle case where permissions are revoked during runtime
- [ ] 11.2 Add logging for permission status changes
- [ ] 11.3 Handle system Settings app not responding
- [ ] 11.4 Add fallback if deep linking fails
- [ ] 11.5 Test behavior when System Settings is already open
- [ ] 11.6 Handle rapid permission granting (prevent race conditions)

## 12. Performance Optimization
- [ ] 12.1 Ensure 1-second polling doesn't impact battery
- [ ] 12.2 Stop monitoring immediately when gate is dismissed
- [ ] 12.3 Verify no memory leaks in monitor timer
- [ ] 12.4 Profile app launch time with permission gate
- [ ] 12.5 Optimize window creation/destruction

## 13. User Experience Polish
- [ ] 13.1 Add helpful instructions text to each permission step
- [ ] 13.2 Include screenshots or visual guides in permission prompts
- [ ] 13.3 Add progress indicator (e.g., "1 of 2 complete")
- [ ] 13.4 Ensure animations are smooth (60fps)
- [ ] 13.5 Test Dark Mode appearance
- [ ] 13.6 Test accessibility features (VoiceOver)

## Dependencies

**Sequential Dependencies:**
- 1 (Input Monitoring Detection) must complete before 2, 3, 4
- 2 (Permission Monitor) must complete before 3, 4
- 3 (Permission Gate View) must complete before 4, 6
- 4 (AppDelegate Integration) must complete before 5, 9
- All implementation (1-8) must complete before 9 (Testing)

**Parallelizable Work:**
- 1 (Input Monitoring Detection) and 7 (Permission Prompt Reuse) can be done in parallel
- 10 (Documentation) can be written alongside implementation
- 13 (UX Polish) can start after 3 (View) is functional

**Critical Path:**
1 → 2 → 3 → 4 → 5 → 9 (Testing) → 10 (Documentation)

**Blockers:**
- Testing (9) requires all features implemented
- Documentation (10) requires testing complete
- Performance optimization (12) requires working implementation

**Validation Criteria:**
Each task should include one of:
- Unit test passing
- Manual test checklist item
- Visual verification (SwiftUI Preview)
- Build success without warnings
