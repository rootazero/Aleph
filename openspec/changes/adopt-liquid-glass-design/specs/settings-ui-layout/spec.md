# settings-ui-layout Specification Delta

## MODIFIED Requirements

### Requirement: Settings Window Dimensions
The Settings window SHALL provide sufficient space for rich content display with modern Liquid Glass design.

**Changes**: 更新窗口尺寸为固定大小，移除可调整大小功能，以配合 Liquid Glass 浮动侧边栏设计。

#### Scenario: Fixed window size (MODIFIED)
- **GIVEN** the user opens Settings
- **WHEN** the window appears
- **THEN** the window SHALL have a fixed frame size of 1000x700 points
- **AND** the window SHALL NOT be resizable by the user (改为固定大小)
- **AND** the window SHALL use `.fullSizeContentView` style for Liquid Glass integration (新增)
- **AND** the window SHALL have transparent title bar (`titlebarAppearsTransparent: true`) (新增)

#### Scenario: Window positioning (UNCHANGED)
- **GIVEN** the Settings window is opened for the first time
- **WHEN** no saved position exists
- **THEN** the window SHALL appear centered on the main screen
- **AND** subsequent opens SHALL restore the last user-positioned location

---

### Requirement: Floating Sidebar Layout (MODIFIED - 原 "Providers Tab Layout Proportions")
The Settings window SHALL use a floating sidebar layout with ZStack layers.

**Changes**: 完全重新设计布局，使用 ZStack 分层而非 HStack 左右分栏。

#### Scenario: ZStack layer structure (NEW)
- **GIVEN** the Settings window is rendering
- **WHEN** laying out components
- **THEN** the layout SHALL use ZStack with 2 layers
- **AND** Layer 0 (bottom) SHALL contain:
  - Left spacer (200pt width, transparent or subtle background)
  - Right content area (full height, with concentric rounded corners)
- **AND** Layer 1 (top) SHALL contain:
  - Floating sidebar (200pt width, Liquid Glass material, subtle shadow)
  - Sidebar SHALL be positioned at top-left with 12pt padding

#### Scenario: Floating sidebar dimensions and style (MODIFIED)
- **GIVEN** the floating sidebar is rendering
- **WHEN** calculating sidebar properties
- **THEN** sidebar width SHALL = 200pt (UNCHANGED)
- **AND** sidebar height SHALL = window height - 24pt (12pt top + 12pt bottom padding)
- **AND** sidebar corner radius SHALL = 10pt (concentric geometry with minimum)
- **AND** sidebar SHALL use `AdaptiveMaterial(.sidebar)` background (NEW)
- **AND** sidebar SHALL use subtle shadow (color: .black.opacity(0.1), radius: 8, offset: (0, 2)) (NEW)
- **AND** sidebar SHALL NOT use hard border (REMOVED)

#### Scenario: Content area dimensions and style (MODIFIED)
- **GIVEN** the content area is rendering
- **WHEN** calculating content area properties
- **THEN** content area SHALL start from x = 200pt (sidebar width)
- **AND** content area width SHALL = window width - 200pt - 1pt = 799pt
- **AND** content area height SHALL = window height = 700pt
- **AND** content area corner radius SHALL = 12pt (window radius - 0pt padding)
- **AND** content area SHALL use concentric rounded corners (top-right, bottom-right)
- **AND** content area SHALL use `DesignTokens.Colors.contentBackground` or `.windowBackground` material
- **AND** content area SHALL NOT use border (REMOVED)

#### Scenario: Responsive layout (REMOVED)
**This scenario is removed because the window is now fixed size.**

---

### Requirement: Content Extension Behind Sidebar (NEW)
Content SHALL be able to extend behind the floating sidebar for immersive effect.

#### Scenario: Full extension for simple tabs
- **GIVEN** a tab with simple content (General, Memory, Behavior)
- **WHEN** rendering tab content
- **THEN** content SHALL extend to x = 0 (full width)
- **AND** content SHALL be placed on Layer 0 (behind sidebar)
- **AND** sidebar SHALL be semi-transparent, allowing content to show through subtly
- **AND** content zIndex SHALL = -1, sidebar zIndex SHALL = 1

#### Scenario: Partial extension for two-column tabs
- **GIVEN** a tab with two-column layout (Providers, Routing)
- **WHEN** rendering tab content
- **THEN** left column (list) SHALL extend to x = 0
- **AND** right column (editor) SHALL start from x = 200pt (aligned with sidebar edge)
- **AND** left column SHALL be visible behind sidebar
- **AND** right column SHALL NOT overlap with sidebar

#### Scenario: No extension for specific tabs
- **GIVEN** a tab requiring clear visual boundary (Shortcuts)
- **WHEN** rendering tab content
- **THEN** content SHALL start from x = 200pt
- **AND** left spacer (0-200pt) SHALL show background or remain empty
- **AND** content SHALL NOT extend behind sidebar

---

## REMOVED Requirements

### Requirement: Providers Tab Layout Proportions (REMOVED)
**Reason**: This requirement is specific to Providers tab and has been generalized to "Floating Sidebar Layout". The two-panel width specifications are no longer relevant with the new ZStack layout.

**Affected Scenarios** (all REMOVED):
- Left panel (provider list) width
- Right panel (edit panel) width
- Responsive layout

---

### Requirement: ScrollView Behavior (MODIFIED)
ScrollViews SHALL handle overflow content gracefully with scroll edge effects.

**Changes**: 添加滚动边缘效果要求。

#### Scenario: Provider list scrolling (MODIFIED)
- **GIVEN** more than 8 provider cards exist
- **WHEN** the provider list exceeds the visible area
- **THEN** the list SHALL scroll vertically with native macOS scrollbars
- **AND** the search bar and "Add Provider" button SHALL remain fixed at the top (UNCHANGED)
- **AND** the list SHALL apply hard-style scroll edge effect (NEW)
  - Top edge: gradient from clear (0%) to solid (5%)
  - Bottom edge: gradient from solid (95%) to clear (100%)
  - Opacity: 0.6, blur: 12pt

#### Scenario: Edit panel scrolling (MODIFIED)
- **GIVEN** the edit form has many fields (Advanced Settings expanded)
- **WHEN** the form exceeds the visible area
- **THEN** the form content SHALL scroll vertically
- **AND** the action buttons (Test, Cancel, Save) SHALL remain visible at the bottom (UNCHANGED)
- **AND** scrolling SHALL be smooth with momentum (native macOS behavior) (UNCHANGED)
- **AND** the form SHALL apply hard-style scroll edge effect (NEW)

---

### Requirement: Active Toggle Integration in Provider Information Card (UNCHANGED)
No changes to this requirement. Toggle styling remains the same.

---

### Requirement: Custom Provider Support (UNCHANGED)
No changes to this requirement. Custom provider functionality unchanged.

---

### Requirement: Conditional Field Visibility Based on Provider Type (UNCHANGED)
No changes to this requirement. Field visibility logic unchanged.

---

### Requirement: Provider Information Display Card (UNCHANGED)
No changes to this requirement. Card layout unchanged.

---

### Requirement: Form Field Ordering (UNCHANGED)
No changes to this requirement. Field order unchanged.

---

## Summary of Changes

**Modified Requirements**: 3
- Settings Window Dimensions (fixed size, Liquid Glass style)
- Floating Sidebar Layout (ZStack-based, replaces Providers Tab Layout Proportions)
- ScrollView Behavior (added scroll edge effects)

**New Requirements**: 1
- Content Extension Behind Sidebar

**Removed Requirements**: 1
- Providers Tab Layout Proportions (generalized to Floating Sidebar Layout)

**Unchanged Requirements**: 5
- Active Toggle Integration
- Custom Provider Support
- Conditional Field Visibility
- Provider Information Display Card
- Form Field Ordering
