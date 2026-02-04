# Implementation Tasks

## 1. Create Provider List Toolbar

### 1.1 Add Toolbar Layout Structure
- [x] 1.1.1 Locate `providerListSection` in ProvidersView.swift (around line 108)
- [x] 1.1.2 Create new `@ViewBuilder` property: `providerListToolbar`
- [x] 1.1.3 Implement toolbar as HStack with:
  - "Add Custom Provider" button (left)
  - `Spacer()`
  - SearchBar component (right, max width ~220pt)
- [x] 1.1.4 Add horizontal padding: `DesignTokens.Spacing.md`
- [x] 1.1.5 Add vertical padding: `DesignTokens.Spacing.sm`

### 1.2 Implement Add Custom Provider Button
- [x] 1.2.1 Add Button with:
  - Icon: `plus.circle` or `plus.square.on.square`
  - Label: "Add Custom Provider"
  - Style: `.plain` or use ActionButton component
- [x] 1.2.2 Implement button action: `addCustomProvider()`
- [x] 1.2.3 Add button styling:
  - Font: `DesignTokens.Typography.body`
  - Foreground color: `DesignTokens.Colors.accentBlue` or primary
  - Padding: `DesignTokens.Spacing.sm`

### 1.3 Move Search Bar to Toolbar
- [x] 1.3.1 Remove SearchBar from current position (line 111)
- [x] 1.3.2 Add SearchBar to toolbar HStack (right side)
- [x] 1.3.3 Set SearchBar frame: `.frame(maxWidth: 220)`
- [x] 1.3.4 Preserve search binding: `$searchText`
- [x] 1.3.5 Preserve placeholder: "Search providers..."

## 2. Implement Add Custom Provider Action

### 2.1 Create Action Handler Method
- [x] 2.1.1 Add method `addCustomProvider()` in ProvidersView
- [x] 2.1.2 Implement logic:
  ```swift
  private func addCustomProvider() {
      // Clear current selection
      selectedProviderId = nil

      // Set to custom preset
      if let customPreset = PresetProviders.find(byId: "custom") {
          selectedPreset = customPreset
          selectedProviderId = "custom"
      }

      // Enter add mode
      isAddingNew = true
  }
  ```
- [x] 2.1.3 Add inline comments explaining each step

### 2.2 Test Add Custom Provider Flow
- [ ] 2.2.1 Click "Add Custom Provider" button
- [ ] 2.2.2 Verify right panel shows empty custom provider form
- [ ] 2.2.3 Verify form fields are editable (Name, Color, Base URL, etc.)
- [ ] 2.2.4 Fill form and save
- [ ] 2.2.5 Verify new custom provider appears in list

## 3. Restructure Provider List Layout

### 3.1 Update Provider List Section Structure
- [x] 3.1.1 Modify `providerListSection` to use VStack:
  - Toolbar (fixed at top)
  - Provider cards (scrollable area)
- [x] 3.1.2 Remove `.padding(.top, DesignTokens.Spacing.md)` from VStack (toolbar handles this)
- [x] 3.1.3 Ensure toolbar does NOT scroll with provider cards

### 3.2 Adjust ScrollView for Provider Cards
- [x] 3.2.1 Wrap only provider cards in ScrollView (not toolbar)
- [x] 3.2.2 Verify toolbar remains fixed when scrolling
- [x] 3.2.3 Preserve existing card layout and spacing
- [ ] 3.2.4 Test with 20+ providers to verify scroll behavior

## 4. Add Visual Container Styling

### 4.1 Add Left Panel (Provider List) Container
- [x] 4.1.1 Wrap `providerListSection` in container modifiers:
  - `.background(DesignTokens.Colors.sidebarBackground)`
  - `.cornerRadius(DesignTokens.CornerRadius.medium)`
- [x] 4.1.2 Add outer padding: `.padding(DesignTokens.Spacing.lg)`
- [x] 4.1.3 Verify corner radius renders correctly (10pt)
- [x] 4.1.4 Verify no shadow effect (flat design)

### 4.2 Add Right Panel (Edit Panel) Container
- [x] 4.2.1 Wrap ProviderEditPanel call in container modifiers:
  - `.background(DesignTokens.Colors.contentBackground)`
  - `.cornerRadius(DesignTokens.CornerRadius.medium)`
- [x] 4.2.2 Add outer padding: `.padding(DesignTokens.Spacing.lg)`
- [x] 4.2.3 Verify corner radius renders correctly (10pt)
- [x] 4.2.4 Verify background color matches design

### 4.3 Adjust Panel Spacing
- [x] 4.3.1 Update HStack(spacing: 0) to HStack(spacing: DesignTokens.Spacing.md)
- [x] 4.3.2 Verify gap between left and right containers
- [x] 4.3.3 Preserve Divider between panels (optional, may remove if containers provide enough separation)
- [ ] 4.3.4 Test responsive behavior on window resize

## 5. Update Custom Provider List Display

### 5.1 Review Current Provider Card Logic
- [x] 5.1.1 Verify SimpleProviderCard supports custom providers
- [x] 5.1.2 Check if custom providers show user-defined name
- [x] 5.1.3 Check if custom providers show user-defined color
- [x] 5.1.4 Verify "isConfigured" logic works for custom instances

### 5.2 Ensure Custom Providers Appear in List
- [ ] 5.2.1 Create a custom provider via "Add Custom Provider"
- [ ] 5.2.2 Save the custom provider configuration
- [ ] 5.2.3 Verify it appears in the provider list
- [ ] 5.2.4 Verify it uses user-defined name and color
- [ ] 5.2.5 Verify clicking the card loads its configuration

### 5.3 Test Multiple Custom Provider Instances
- [ ] 5.3.1 Create 3 different custom providers (e.g., "Company API", "Local LLM", "Proxy")
- [ ] 5.3.2 Verify all 3 appear as separate cards in the list
- [ ] 5.3.3 Verify each can be independently selected
- [ ] 5.3.4 Verify each can be independently edited
- [ ] 5.3.5 Delete one custom provider
- [ ] 5.3.6 Verify others remain unaffected

## 6. Testing and Validation

### 6.1 Visual Regression Testing
- [ ] 6.1.1 Compare new layout with current layout side-by-side
- [ ] 6.1.2 Verify corner radius is consistent (10pt on all containers)
- [ ] 6.1.3 Verify spacing is consistent (toolbar, cards, panels)
- [ ] 6.1.4 Verify colors match design tokens
- [ ] 6.1.5 Test in both light and dark mode

### 6.2 Functional Testing
- [ ] 6.2.1 Test "Add Custom Provider" button flow (end-to-end)
- [ ] 6.2.2 Test search functionality in toolbar
- [ ] 6.2.3 Test selecting preset providers
- [ ] 6.2.4 Test selecting custom providers
- [ ] 6.2.5 Test editing existing providers
- [ ] 6.2.6 Test deleting providers

### 6.3 Responsive Behavior Testing
- [ ] 6.3.1 Resize window to minimum width (1200pt)
- [ ] 6.3.2 Verify left panel is 450pt minimum
- [ ] 6.3.3 Verify right panel is 500pt minimum
- [ ] 6.3.4 Resize window to larger sizes (1600pt, 2000pt)
- [ ] 6.3.5 Verify panels grow proportionally
- [ ] 6.3.6 Verify corner radius remains consistent at all sizes

### 6.4 Scroll Behavior Testing
- [ ] 6.4.1 Add 20+ provider cards (mix of preset and custom)
- [ ] 6.4.2 Verify toolbar remains fixed when scrolling provider list
- [ ] 6.4.3 Verify smooth scroll with macOS momentum
- [ ] 6.4.4 Test edit panel scrolling with many form fields expanded
- [ ] 6.4.5 Verify action buttons remain fixed at bottom (footer)

### 6.5 Edge Cases Testing
- [ ] 6.5.1 Test with no configured providers
- [ ] 6.5.2 Test with only preset providers
- [ ] 6.5.3 Test with only custom providers
- [ ] 6.5.4 Test with search query that matches nothing
- [ ] 6.5.5 Test with search query that matches custom providers only
- [ ] 6.5.6 Test rapid clicking of "Add Custom Provider" button

## 7. Code Quality and Documentation

### 7.1 Code Review
- [x] 7.1.1 Run Swift syntax validation: `$HOME/.python3/bin/python verify_swift_syntax.py Aleph/Sources/ProvidersView.swift`
- [x] 7.1.2 Check for unused variables or dead code
- [x] 7.1.3 Verify consistent naming conventions (camelCase for properties)
- [x] 7.1.4 Verify inline comments for complex logic

### 7.2 Generate Xcode Project
- [x] 7.2.1 Run `xcodegen generate`
- [x] 7.2.2 Open Aleph.xcodeproj
- [ ] 7.2.3 Verify no build errors
- [ ] 7.2.4 Verify no warnings (or document acceptable warnings)

### 7.3 Update Documentation (if needed)
- [x] 7.3.1 Check if CLAUDE.md needs updates (likely not)
- [x] 7.3.2 Add inline code comments explaining toolbar layout
- [x] 7.3.3 Document `addCustomProvider()` method behavior

## Implementation Summary

This change restructures the Providers view layout to:
1. Add a unified toolbar with "Add Custom Provider" button and search bar
2. Apply visual container styling (corner radius) to both panels
3. Streamline the custom provider creation workflow

**Key Files Modified:**
- `Aleph/Sources/ProvidersView.swift` - Main layout restructuring

**No Breaking Changes:**
- Configuration file format unchanged
- Existing provider configs fully compatible
- All current functionality preserved

**Estimated Implementation Time:**
- Toolbar creation: 30-45 minutes
- Visual container styling: 20-30 minutes
- Testing and validation: 45-60 minutes
- **Total**: ~2 hours

**Dependencies:**
- Tasks 1-3 should be completed sequentially (toolbar → action → layout)
- Task 4 (visual styling) can be done in parallel with Task 5 (custom provider display)
- Task 6 (testing) depends on all previous tasks
