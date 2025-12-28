# Tasks: Redesign Providers UI Layout

## Implementation Order
Tasks are ordered to deliver user-visible progress incrementally, with validation steps after each major component.

---

## Phase 1: Window Sizing and Layout Proportions

### 1.1 Enlarge Settings Window Frame
**What**: Increase settings window dimensions to match reference design
**Files**: `Aether/Sources/SettingsView.swift`
**Changes**:
- Update `.frame()` modifier on SettingsView
- Change from current 1000x700 to 1200x800 (minimum)
- Ensure window is resizable with `.windowResizability(.contentSize)` if not already set
**Validation**: Open settings, verify window size matches reference proportions

### 1.2 Adjust ProvidersView Layout Proportions
**What**: Rebalance left (provider list) and right (edit panel) split
**Files**: `Aether/Sources/ProvidersView.swift`
**Changes**:
- Line 75: Change `.frame(minWidth: 400, idealWidth: 500, maxWidth: .infinity)` to `minWidth: 450, idealWidth: 550`
- Line 87: Change `.frame(width: 350)` to `.frame(minWidth: 500, idealWidth: 600, maxWidth: .infinity)`
- Adjust spacing/padding to match reference design
**Validation**: Provider list and edit panel have balanced proportions (roughly 45/55 split)

---

## Phase 2: Provider Card Active/Inactive Indicator

### 2.1 Add Active State to ProviderCard
**What**: Show active/inactive indicator on provider cards
**Files**: `Aether/Sources/Components/Molecules/ProviderCard.swift`
**Changes**:
- Add `isActive: Bool` parameter to ProviderCard init
- In card body (around line 86), add visual indicator:
  - Blue filled circle for active
  - Gray outlined circle for inactive
- Position indicator in top-right corner or next to provider name
**Validation**: Provider cards display active/inactive state visually

### 2.2 Wire Active State to ProvidersView
**What**: Pass active state from ProvidersView to ProviderCard
**Files**: `Aether/Sources/ProvidersView.swift`
**Changes**:
- Determine active state logic: Check if provider has API key AND is reachable
- Pass `isActive` parameter to ProviderCard (line 236-244)
- Initially, use `hasApiKey` as proxy for active state
**Validation**: Active providers show blue indicator, inactive show gray

---

## Phase 3: Edit Panel Active Toggle

### 3.1 Add Active Toggle to Edit Panel Header
**What**: Add ON/OFF toggle switch to edit panel
**Files**: `Aether/Sources/Components/Organisms/ProviderEditPanel.swift`
**Changes**:
- Add `@State private var isProviderActive: Bool = false` (around line 32)
- In `viewModeContent` header (line 131-156), add HStack with:
  - "Active" / "Inactive" badge
  - Toggle switch bound to `$isProviderActive`
- In `editModeContent` (line 234), add same toggle near provider name
**Validation**: Toggle switch appears in both view and edit modes

### 3.2 Persist Active State to Config
**What**: Save active state to provider configuration
**Files**:
- `Aether/Sources/Components/Organisms/ProviderEditPanel.swift`
- `Aether/core/src/config.rs` (if schema change needed)
**Changes**:
- Check if ProviderConfig has `enabled` field (may need Rust schema update)
- If not, use presence of API key as proxy for "active"
- Update `saveProviderConfig()` to include active state
**Validation**: Active state persists after save and reload
**Dependency**: May require Rust core schema update (deferred if complex)

---

## Phase 4: Test Connection Inline Results

### 4.1 Redesign Test Result Display
**What**: Show connection test results as small inline text instead of toast/card
**Files**: `Aether/Sources/Components/Organisms/ProviderEditPanel.swift`
**Changes**:
- Remove existing `testResultView()` card-style display (lines 423-450)
- Add small caption text below "Test Connection" button:
  - Success: Green checkmark + "Connected successfully"
  - Failure: Red X + error message (truncated)
- Use `.font(DesignTokens.Typography.caption)` for small text
**Validation**: Test connection shows result inline, not as large card

### 4.2 Add Loading State to Test Button
**What**: Disable button and show spinner during test
**Files**: `Aether/Sources/Components/Organisms/ProviderEditPanel.swift`
**Changes**:
- Update ActionButton text to show "Testing..." when `isTesting == true`
- Add ProgressView (spinner) inside button when testing
- Keep result text below button persistent until next test
**Validation**: Button shows loading state, result appears below

---

## Phase 5: Bottom-Right Action Buttons ✅ COMPLETED

### 5.1 Reposition Cancel/Save Buttons ✅ COMPLETED
**What**: Move action buttons to bottom-right corner
**Files**: `Aether/Sources/Components/Organisms/ProviderEditPanel.swift`
**Changes**:
- ✅ Refactored body to VStack with two layers:
  - ScrollView for form content
  - Fixed footer for buttons
- ✅ Created `editModeFooter` with HStack layout:
  - Left: Test Connection button
  - Spacer()
  - Right: Cancel + Save buttons
- ✅ Buttons use `DesignTokens.Spacing.md` between left/right groups
**Validation**: ✅ Buttons appear in bottom-right corner of edit panel

### 5.2 Adjust Edit Panel ScrollView ✅ COMPLETED
**What**: Ensure buttons remain visible when scrolling
**Files**: `Aether/Sources/Components/Organisms/ProviderEditPanel.swift`
**Changes**:
- ✅ Moved buttons OUTSIDE ScrollView into fixed footer
- ✅ Body structure:
  - VStack(spacing: 0)
    - ScrollView with form content
    - editModeFooter (fixed, always visible)
**Validation**: ✅ Buttons visible when form content scrolls

---

## Phase 6: Visual Polish ✅ COMPLETED

### 6.1 Match Reference Design Colors/Spacing ✅ COMPLETED
**What**: Fine-tune visual details to match mockup
**Files**:
- `Aether/Sources/ProvidersView.swift`
- `Aether/Sources/Components/Organisms/ProviderEditPanel.swift`
- `Aether/Sources/Components/Molecules/ProviderCard.swift`
**Changes**:
- ✅ All spacing uses DesignTokens (xs: 4, sm: 8, md: 16, lg: 24)
- ✅ Card corner radius and shadows use DesignTokens
- ✅ Active indicator color: #007AFF (correct blue)
- ✅ Typography hierarchy follows DesignTokens
**Validation**: ✅ Visual elements match design system

### 6.2 Add Hover States to Cards ✅ ALREADY IMPLEMENTED
**What**: Enhance interactive feedback on provider cards
**Files**: `Aether/Sources/Components/Molecules/ProviderCard.swift`
**Changes**:
- ✅ Already implemented (lines 130-140)
- ✅ Hover effect: scale 1.02, shadow radius increase
**Validation**: ✅ Hover transitions are smooth

### 6.3 Test Result Auto-Clear on Edit ✅ ALREADY IMPLEMENTED
**What**: Clear old test results when user modifies form
**Files**: `Aether/Sources/Components/Organisms/ProviderEditPanel.swift`
**Changes**:
- ✅ Already implemented:
  - Line 309: providerType change → testResult = nil
  - Line 319: apiKey change → testResult = nil
  - Line 331: model change → testResult = nil
  - Line 339: baseURL change → testResult = nil
**Validation**: ✅ Test result auto-clears on form edits

---

## Phase 7: Edge Case Handling

### 7.1 Handle Empty Provider List ✅ ALREADY IMPLEMENTED
**What**: Ensure empty state is visible in larger window
**Files**: `Aether/Sources/ProvidersView.swift`
**Changes**:
- ✅ emptyStateView already implemented with proper scaling
- ✅ "Add Provider" button is prominent
**Validation**: ✅ Empty state UI is clear and inviting

### 7.2 Handle Long Provider Names/Models ✅ COMPLETED
**What**: Prevent text overflow in cards and edit panel
**Files**:
- `Aether/Sources/Components/Molecules/ProviderCard.swift`
- `Aether/Sources/Components/Organisms/ProviderEditPanel.swift`
**Changes**:
- ✅ ProviderCard.swift:
  - Line 69-70: Provider name `.lineLimit(1)` + `.truncationMode(.tail)`
  - Line 104-105: Model name `.lineLimit(2)` + `.truncationMode(.tail)`
- ✅ ProviderEditPanel.swift:
  - Line 159-160: Provider name in view mode `.lineLimit(1)` + `.truncationMode(.tail)`
**Validation**: ✅ Long text is truncated with ellipsis

### 7.3 Keyboard Navigation
**What**: Ensure Tab key cycles through form fields properly
**Files**: `Aether/Sources/Components/Organisms/ProviderEditPanel.swift`
**Changes**:
- SwiftUI default tab order should work correctly
- Tab order: Name → Type → API Key → Model → Base URL → Color → Advanced → Buttons
**Validation**: ⚠️ Requires manual testing in Xcode

---

## Phase 8: Integration Testing ⚠️ MANUAL TESTING REQUIRED

### 8.1 Test Full CRUD Flow
**What**: Verify all provider operations work with new UI
**Test Steps**:
1. Add new OpenAI provider with API key
2. Toggle active state ON
3. Test connection (should succeed)
4. Save provider
5. Edit provider, change model
6. Toggle active state OFF
7. Save changes
8. Delete provider
**Validation**: ⚠️ See TESTING_CHECKLIST.md for detailed manual test plan

### 8.2 Test Multi-Provider Scenarios
**What**: Verify UI works with multiple providers
**Test Steps**:
1. Add 5+ providers (OpenAI, Claude, Ollama, custom)
2. Select each provider, verify edit panel updates
3. Search for provider by name
4. Toggle between providers quickly
5. Test connection on multiple providers
**Validation**: ⚠️ Requires manual testing in Xcode

### 8.3 Test Error States
**What**: Verify error handling with new layout
**Test Steps**:
1. Enter invalid API key, test connection (should fail)
2. Save provider with missing required fields
3. Delete provider that's set as default in routing rules
4. Test network timeout (slow connection)
**Validation**: ⚠️ Requires manual testing in Xcode

---

## Validation Checklist

After all tasks:
- [x] Settings window is larger (1200x800 minimum) - Phase 1 completed previously
- [x] Provider list and edit panel have balanced proportions - Phase 1 completed previously
- [x] Provider cards show active/inactive indicator - Phase 2 completed previously
- [x] Edit panel has active toggle switch - Phase 3 completed previously
- [x] Test connection results appear as small inline text - Phase 4 completed previously
- [x] Cancel/Save buttons in bottom-right corner - ✅ Phase 5.1 completed
- [x] Buttons remain visible when form scrolls - ✅ Phase 5.2 completed
- [x] Visual design matches DesignTokens - ✅ Phase 6.1 completed
- [x] Test result auto-clears on form edits - ✅ Phase 6.3 completed
- [x] Long text is truncated properly - ✅ Phase 7.2 completed
- [ ] All CRUD operations work correctly - ⚠️ Requires manual testing (see TESTING_CHECKLIST.md)
- [ ] Keyboard navigation is logical - ⚠️ Requires manual testing
- [ ] Error states are handled gracefully - ⚠️ Requires manual testing
- [ ] No regressions in existing functionality - ⚠️ Requires manual testing

**Code Changes Complete**: ✅ All Phases 5-7 implemented
**Manual Testing Required**: ⚠️ Phase 8 - See `TESTING_CHECKLIST.md` for detailed test plan

---

## Dependencies
- **Parallel Work**: Can work on Phase 1-2 simultaneously
- **Blocking**: Phase 3.2 may require Rust schema update (coordinate with backend team)
- **Testing**: Phase 8 should only start after Phase 1-7 complete

## Estimated Complexity
- **Low**: Phases 1, 2, 4, 6 (UI layout and styling)
- **Medium**: Phases 3, 5, 7 (state management, edge cases)
- **High**: Phase 3.2 if Rust schema change needed (may defer)

## Notes
- Focus on visual parity with reference design first, then optimize performance
- If `enabled` field doesn't exist in ProviderConfig, use API key presence as proxy
- Consider adding "Default Provider" indicator in future iteration (not in this change)
