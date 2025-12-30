# Tasks: Redesign Permission Authorization

## Phase 1: Swift Layer - Permission Monitoring Redesign

### Task 1.1: Create new PermissionManager class
- [x] Create `Aether/Sources/Utils/PermissionManager.swift`
- [x] Implement `@Published` properties: `accessibilityGranted`, `inputMonitoringGranted`
- [x] Implement `Timer`-based polling (1-second interval)
- [x] Implement `checkPermissions()` method (passive monitoring, no restart)
- [x] Implement `checkInputMonitoringViaHID()` using IOHIDManager
- [x] Implement `requestAccessibility()` method
- [x] Implement `requestInputMonitoring()` method
- [x] Add `startMonitoring()` and `stopMonitoring()` lifecycle methods
- [ ] Validation: Run unit tests to verify timer polling and state updates

**Files created:**
- `Aether/Sources/Utils/PermissionManager.swift`

**Acceptance criteria:**
- ✅ Timer polls every 1 second
- ✅ `accessibilityGranted` updates when AXIsProcessTrusted() changes
- ✅ `inputMonitoringGranted` updates when IOHIDManagerOpen() changes
- ✅ NO calls to `exit()`, `NSApp.terminate()`, or restart methods

### Task 1.2: Rewrite PermissionGateView with waterfall design
- [x] Backup existing `Aether/Sources/Components/PermissionGateView.swift`
- [x] Replace `@StateObject var monitor: PermissionStatusMonitor` with `PermissionManager`
- [x] Implement waterfall flow: Step 1 (Accessibility) → Step 2 (Input Monitoring)
- [x] Add `isEnabled` logic for Step 2 (depends on Step 1 completion)
- [x] Remove all automatic restart logic from `startMonitoring()` callback
- [x] Add "进入 Aether" button (shown when both permissions granted)
- [x] Implement user-triggered `restartApp()` method
- [x] Simplify `checkInitialPermissions()` (0.3s delay, no debounce)
- [x] **FIX**: Call `manager.startMonitoring()` to activate timer polling
- [x] **FIX**: Request permissions before opening System Settings
- [ ] Validation: Manual test permission grant flow

**Files modified:**
- `Aether/Sources/Components/PermissionGateView.swift`

**Acceptance criteria:**
- ✅ Step 2 button disabled until Step 1 completed
- ✅ Accessibility grant does NOT trigger automatic restart
- ✅ Input Monitoring grant shows "进入 Aether" button (user clicks to restart)
- ✅ No automatic restart logic in entire view

### Task 1.3: Enhance PermissionChecker with HID detection
- [x] Modify `Aether/Sources/Utils/PermissionChecker.swift`
- [x] Add `hasInputMonitoringViaHID()` static method
- [x] Implement IOHIDManager creation, device matching, and open/close
- [x] Update `hasInputMonitoringPermission()` to call HID method
- [x] Add `openSystemSettings(for:)` method with deep link URLs
- [x] Remove retry/sleep logic from `hasAccessibilityPermission()`
- [ ] Validation: Run unit tests to verify HID detection accuracy

**Files modified:**
- `Aether/Sources/Utils/PermissionChecker.swift`

**Acceptance criteria:**
- ✅ `hasInputMonitoringViaHID()` accurately detects permission status
- ✅ Returns true if IOHIDManagerOpen() succeeds
- ✅ Returns false if IOHIDManagerOpen() fails with kIOReturnNotPermitted
- ✅ Opens System Settings to correct privacy pane

### Task 1.4: Delete deprecated PermissionStatusMonitor
- [x] Remove all references to `PermissionStatusMonitor` in codebase
- [x] Search for imports: `grep -r "PermissionStatusMonitor" Aether/Sources/`
- [x] Update `PermissionGateView` to use `PermissionManager` instead
- [x] Delete file: `Aether/Sources/Utils/PermissionStatusMonitor.swift`
- [x] Validation: Build succeeds without errors

**Files deleted:**
- `Aether/Sources/Utils/PermissionStatusMonitor.swift`

**Acceptance criteria:**
- ✅ No references to `PermissionStatusMonitor` in codebase
- ✅ Project compiles successfully

### Task 1.5: Update AppDelegate permission gate logic
- [ ] Modify `Aether/Sources/AppDelegate.swift`
- [ ] Use `PermissionChecker.hasAllRequiredPermissions()` at startup
- [ ] Show `PermissionGateView` if permissions missing
- [ ] Initialize `AetherCore` only after permissions granted
- [ ] Remove any old restart logic from permission callbacks
- [ ] Validation: Launch app without permissions, verify gate appears

**Files modified:**
- `Aether/Sources/AppDelegate.swift`

**Acceptance criteria:**
- ✅ App shows permission gate when permissions missing
- ✅ App skips gate when permissions already granted
- ✅ `AetherCore` initialized only after permissions confirmed

## Phase 2: Rust Layer - Panic Protection & Permission Pre-Check

### Task 2.1: Add panic protection to rdev listener
- [x] Modify `Aether/core/src/hotkey/rdev_listener.rs`
- [x] Wrap `rdev::listen()` call in `std::panic::catch_unwind()`
- [x] Implement panic payload extraction (String or &str)
- [x] Log detailed panic message with user guidance
- [x] Return `Err(HotkeyError::PermissionDenied)` on panic
- [ ] Validation: Run unit test simulating panic

**Files modified:**
- `Aether/core/src/hotkey/rdev_listener.rs`

**Acceptance criteria:**
- ✅ `catch_unwind()` successfully captures rdev panic
- ✅ Panic converted to error, no process crash
- ✅ Detailed error log includes user guidance

### Task 2.2: Implement permission pre-check in AetherCore
- [x] Modify `Aether/core/src/core.rs`
- [x] Add `has_input_monitoring_permission: bool` field to `AetherCore` struct
- [x] Implement `set_input_monitoring_permission(granted: bool)` method
- [x] Update `start_listening()` to check permission before calling rdev
- [x] Return `Err(AetherError::PermissionDenied)` if permission not granted
- [x] Call `event_handler.on_error()` with permission error message
- [ ] Validation: Run unit test with permission = false

**Files modified:**
- `Aether/core/src/core.rs`

**Acceptance criteria:**
- ✅ `start_listening()` returns error if permission not granted
- ✅ `rdev::listen()` NOT called when permission missing
- ✅ Swift layer receives error via UniFFI

### Task 2.3: Update UniFFI interface definition
- [ ] Modify `Aether/core/src/aether.udl`
- [ ] Add `set_input_monitoring_permission(boolean granted)` method to AetherCore
- [ ] Add `PermissionDenied` variant to `AetherError` enum
- [ ] Add `PermissionDenied` variant to `HotkeyError` enum
- [ ] Regenerate Swift bindings: `cargo run --bin uniffi-bindgen generate ...`
- [ ] Validation: Swift code compiles with new bindings

**Files modified:**
- `Aether/core/src/aether.udl`
- `Aether/Sources/Generated/aether.swift` (generated)

**Acceptance criteria:**
- ✅ UniFFI generates `setInputMonitoringPermission()` method in Swift
- ✅ Swift can catch `AetherError.permissionDenied`

### Task 2.4: Add error types for permission denial
- [ ] Modify `Aether/core/src/error.rs`
- [ ] Add `PermissionDenied(String)` variant to `AetherError`
- [ ] Add `PermissionDenied(String)` variant to `HotkeyError`
- [ ] Implement `Display` and `Error` traits for new variants
- [ ] Update error conversion logic for UniFFI
- [ ] Validation: Run unit tests for error handling

**Files modified:**
- `Aether/core/src/error.rs`

**Acceptance criteria:**
- ✅ `AetherError::PermissionDenied` can be created and formatted
- ✅ Error messages include actionable user guidance

## Phase 3: Integration & Testing

### Task 3.1: Write Swift unit tests for PermissionManager
- [ ] Create `AetherTests/PermissionManagerTests.swift`
- [ ] Test: Timer starts and stops correctly
- [ ] Test: `checkPermissions()` updates `@Published` properties
- [ ] Test: No restart methods called when permission changes
- [ ] Test: IOHIDManager detection returns correct results
- [ ] Validation: All tests pass

**Files created:**
- `AetherTests/PermissionManagerTests.swift`

**Acceptance criteria:**
- ✅ 100% test coverage for PermissionManager public methods
- ✅ All tests pass

### Task 3.2: Write Rust unit tests for panic protection
- [ ] Modify `Aether/core/tests/hotkey_tests.rs`
- [ ] Test: `catch_unwind()` captures panic in rdev listener
- [ ] Test: Permission pre-check blocks listener start
- [ ] Test: `set_input_monitoring_permission()` updates flag
- [ ] Test: Error propagation via UniFFI
- [ ] Validation: All tests pass

**Files modified:**
- `Aether/core/tests/hotkey_tests.rs`

**Acceptance criteria:**
- ✅ Panic protection test passes (no crash)
- ✅ Permission pre-check test passes (returns error)
- ✅ All tests pass with `cargo test`

### Task 3.3: Integration test - Swift ↔ Rust permission flow
- [ ] Create `AetherTests/PermissionIntegrationTests.swift`
- [ ] Test: Swift calls `core.start_listening()` without permission → receives error
- [ ] Test: Swift updates permission via `core.set_input_monitoring_permission(true)` → listener starts
- [ ] Test: Rust calls `event_handler.on_error()` → Swift receives callback
- [ ] Validation: All tests pass

**Files created:**
- `AetherTests/PermissionIntegrationTests.swift`

**Acceptance criteria:**
- ✅ Swift-Rust communication works correctly for permission flow
- ✅ All integration tests pass

### Task 3.4: Manual testing - End-to-end permission flow
- [ ] Test: Launch app without permissions → PermissionGateView appears
- [ ] Test: Grant Accessibility → UI updates, no restart
- [ ] Test: Grant Input Monitoring → "进入 Aether" button appears
- [ ] Test: Click "进入 Aether" → App restarts
- [ ] Test: Relaunch app with permissions → Gate skipped, Core initialized
- [ ] Test: Revoke permission mid-session → Error logged, no crash
- [ ] Document results in `docs/permission-flow-testing-results.md`

**Acceptance criteria:**
- ✅ No crashes or restart loops
- ✅ All expected UI states appear correctly
- ✅ Rust Core handles permission errors gracefully

## Phase 4: Documentation & Cleanup

### Task 4.1: Update permission flow documentation
- [ ] Update `docs/permission-gate-troubleshooting.md` with new design
- [ ] Document waterfall flow design
- [ ] Document IOHIDManager detection method
- [ ] Document Rust panic protection mechanism
- [ ] Add troubleshooting section for common issues

**Files modified:**
- `docs/permission-gate-troubleshooting.md`

**Acceptance criteria:**
- ✅ Documentation accurately reflects new design
- ✅ Troubleshooting steps are clear and actionable

### Task 4.2: Update CLAUDE.md with new architecture
- [ ] Update `CLAUDE.md` section on permission handling
- [ ] Document PermissionManager instead of PermissionStatusMonitor
- [ ] Add section on Rust panic protection
- [ ] Update architecture diagrams if needed

**Files modified:**
- `CLAUDE.md`

**Acceptance criteria:**
- ✅ AI assistant guidance reflects new permission architecture

### Task 4.3: Update translation files for new UI strings
- [ ] Update `Aether/Resources/en.lproj/Localizable.strings`
- [ ] Update `Aether/Resources/zh_CN.lproj/Localizable.strings`
- [ ] Add key: `permission.gate.button.enter_aether` = "进入 Aether"
- [ ] Run `Scripts/validate_translations.sh` to verify completeness
- [ ] Validation: Translation coverage 100%

**Files modified:**
- `Aether/Resources/en.lproj/Localizable.strings`
- `Aether/Resources/zh_CN.lproj/Localizable.strings`

**Acceptance criteria:**
- ✅ All new UI strings are localized
- ✅ Translation script passes

### Task 4.4: Code review and cleanup
- [ ] Remove all commented-out old restart logic
- [ ] Remove debug print statements
- [ ] Ensure consistent error logging (use `log::` macros in Rust)
- [ ] Run SwiftLint and fix warnings
- [ ] Run `cargo clippy` and fix warnings
- [ ] Validation: No lint warnings

**Acceptance criteria:**
- ✅ No commented-out code
- ✅ No SwiftLint warnings
- ✅ No clippy warnings

## Phase 5: Deployment & Verification

### Task 5.1: Build release version and test
- [ ] Build release version: `xcodebuild -configuration Release`
- [ ] Test on macOS 13 (Ventura)
- [ ] Test on macOS 14 (Sonoma)
- [ ] Test on macOS 15 (Sequoia)
- [ ] Verify no crashes or restart loops on all platforms

**Acceptance criteria:**
- ✅ Release build succeeds
- ✅ App works correctly on all supported macOS versions

### Task 5.2: Archive enforce-permission-gating change
- [ ] Run `openspec archive enforce-permission-gating`
- [ ] Verify old change proposal archived correctly
- [ ] Update specs to reflect current implementation

**Acceptance criteria:**
- ✅ Old change archived
- ✅ Specs updated to production state

### Task 5.3: Final validation
- [ ] Run full test suite: `xcodebuild test`
- [ ] Run Rust tests: `cargo test`
- [ ] Verify all todos completed
- [ ] Create git commit with all changes

**Acceptance criteria:**
- ✅ All tests pass
- ✅ All todos marked as completed
- ✅ Changes committed to git

---

## Summary

**Total tasks:** 26

**Phases:**
1. Swift Layer (5 tasks) - Permission monitoring redesign
2. Rust Layer (4 tasks) - Panic protection & permission pre-check
3. Integration & Testing (4 tasks) - Unit and integration tests
4. Documentation & Cleanup (4 tasks) - Docs and code cleanup
5. Deployment & Verification (3 tasks) - Release build and final validation

**Estimated effort:** 2-3 days (assuming full-time work)

**Critical path:**
1. Task 1.1 (PermissionManager) → Task 1.2 (PermissionGateView) → Task 1.5 (AppDelegate)
2. Task 2.1 (rdev panic) → Task 2.2 (permission pre-check) → Task 2.3 (UniFFI)
3. Task 3.4 (manual testing) → Task 5.1 (release build) → Task 5.3 (final validation)

**Dependencies:**
- Swift Layer tasks must complete before integration tests
- Rust Layer tasks must complete before integration tests
- All implementation tasks must complete before documentation updates
