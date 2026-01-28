# Components Index - Modernize Settings UI

## Overview

This document catalogs all components created for the Modernize Settings UI change, organized by atomic design hierarchy. Each component is documented with its purpose, dependencies, and usage examples.

## Component Hierarchy

```
Design System
├── DesignTokens (Foundation)
├── ThemeManager (Foundation)
├── Atoms (Basic Building Blocks)
├── Molecules (Composed Components)
└── Organisms (Complex Components)
```

---

## Foundation Layer

### DesignTokens.swift
**Location**: `Aether/Sources/DesignSystem/DesignTokens.swift`

**Purpose**: Centralized design system constants ensuring visual consistency across the application.

**Provides**:
- **Colors**: Semantic color definitions with automatic light/dark mode support
  - Background colors (sidebar, card, content)
  - Accent colors (blue, gray)
  - Status colors (active, inactive, warning, error, info)
  - Text colors (primary, secondary, tertiary, disabled)
- **Spacing**: Consistent spacing scale
  - xs (4pt), sm (8pt), md (16pt), lg (24pt), xl (32pt), xxl (48pt)
- **Corner Radius**: Rounded corner standards
  - small (6pt), medium (10pt), large (16pt)
- **Typography**: Font hierarchy
  - title, heading, subheading, body, caption, code
- **Shadows**: Elevation and depth
  - card, elevated, dropdown
- **Animation**: Timing standards
  - fast (200ms), normal (300ms), slow (500ms)

**Usage**:
```swift
// Colors
Text("Provider Name").foregroundColor(DesignTokens.Colors.textPrimary)
RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
  .fill(DesignTokens.Colors.cardBackground)

// Spacing
VStack(spacing: DesignTokens.Spacing.md) { ... }
.padding(DesignTokens.Spacing.lg)

// Typography
Text("Title").font(DesignTokens.Typography.title)
```

**Dependencies**: None (Foundation)

---

### ThemeManager.swift
**Location**: `Aether/Sources/DesignSystem/ThemeManager.swift`

**Purpose**: Manages application theme state (Light/Dark/Auto) with persistence and system appearance tracking.

**Features**:
- Three theme modes: `.light`, `.dark`, `.auto`
- UserDefaults persistence (key: "aether_theme_preference")
- Automatic system appearance following in Auto mode
- Real-time NSAppearance application

**Usage**:
```swift
// In App or View
@StateObject private var themeManager = ThemeManager()

// Set theme
themeManager.setTheme(.dark)

// Observe current theme
themeManager.currentTheme // .light, .dark, or .auto
```

**Dependencies**:
- Foundation (UserDefaults)
- AppKit (NSApplication, NSAppearance)

---

## Atoms (Basic Building Blocks)

### SearchBar.swift
**Location**: `Aether/Sources/Components/Atoms/SearchBar.swift`

**Purpose**: Reusable search input component with clear functionality.

**Features**:
- Magnifying glass icon
- Placeholder text
- Clear button (appears when text exists)
- Two-way binding via `@Binding<String>`

**Props**:
- `searchText: Binding<String>` - Bound search text
- `placeholder: String` - Placeholder text (default: "Search...")

**Usage**:
```swift
@State private var searchQuery = ""

SearchBar(searchText: $searchQuery, placeholder: "Search providers")
```

**Dependencies**:
- DesignTokens (styling)

---

### StatusIndicator.swift
**Location**: `Aether/Sources/Components/Atoms/StatusIndicator.swift`

**Purpose**: Visual status indicator with color-coded states.

**Features**:
- Circular indicator
- Three states: Active (green), Inactive (gray), Warning (yellow)
- Optional text label
- Optional blinking animation

**Props**:
- `status: Status` - enum: `.active`, `.inactive`, `.warning`
- `label: String?` - Optional text label
- `animated: Bool` - Enable blinking animation (default: false)

**Usage**:
```swift
StatusIndicator(status: .active, label: "Online")
StatusIndicator(status: .warning, label: "Limited", animated: true)
```

**Dependencies**:
- DesignTokens (colors)

---

### ActionButton.swift
**Location**: `Aether/Sources/Components/Atoms/ActionButton.swift`

**Purpose**: Consistent button styles for primary, secondary, and danger actions.

**Features**:
- Three styles: `.primary` (blue), `.secondary` (outlined), `.danger` (red)
- Icon support (SF Symbols)
- Disabled state
- Click scale animation

**Props**:
- `title: String` - Button text
- `icon: String?` - SF Symbol name (optional)
- `style: ButtonStyle` - `.primary`, `.secondary`, or `.danger`
- `disabled: Bool` - Disabled state (default: false)
- `action: () -> Void` - Callback on tap

**Usage**:
```swift
ActionButton(title: "Save", icon: "checkmark", style: .primary) {
    saveConfiguration()
}

ActionButton(title: "Delete", style: .danger, disabled: isReadOnly) {
    deleteProvider()
}
```

**Dependencies**:
- DesignTokens (colors, animation)

---

### VisualEffectBackground.swift
**Location**: `Aether/Sources/Components/Atoms/VisualEffectBackground.swift`

**Purpose**: NSVisualEffectView wrapper for native macOS blur effects.

**Features**:
- Material presets (sidebar, headerView, menu, etc.)
- Automatic light/dark mode adaptation
- Behind-window blending

**Props**:
- `material: NSVisualEffectView.Material` - Blur material type
- `blendingMode: NSVisualEffectView.BlendingMode` - Blending mode (default: .behindWindow)

**Usage**:
```swift
ZStack {
    VisualEffectBackground(material: .sidebar)
    // Content here
}
```

**Dependencies**:
- AppKit (NSVisualEffectView)

---

### ThemeSwitcher.swift
**Location**: `Aether/Sources/Components/Atoms/ThemeSwitcher.swift`

**Purpose**: Three-button theme selector (Light/Dark/Auto) with visual feedback.

**Features**:
- Three icon buttons: Sun (Light), Moon (Dark), Half-circle (Auto)
- Selected state: Blue background highlight
- Smooth transition animations
- Persists selection via ThemeManager

**Props**:
- `themeManager: ObservedObject<ThemeManager>` - Injected theme manager

**Usage**:
```swift
@StateObject private var themeManager = ThemeManager()

ThemeSwitcher(themeManager: themeManager)
  .frame(height: 30)
```

**Dependencies**:
- ThemeManager
- DesignTokens (colors, corner radius, spacing)

---

## Molecules (Composed Components)

### ProviderCard.swift
**Location**: `Aether/Sources/Components/Molecules/ProviderCard.swift`

**Purpose**: Card displaying Provider information with interactive states.

**Features**:
- Provider icon (SF Symbol)
- Provider name and type
- Status indicator
- Hover scale animation (1.0 → 1.02)
- Selected state (blue border)
- Right-click context menu support

**Props**:
- `provider: Provider` - Provider data model
- `isSelected: Bool` - Selection state
- `onSelect: () -> Void` - Selection callback

**Usage**:
```swift
ProviderCard(
    provider: openAIProvider,
    isSelected: selectedProvider == openAIProvider.id
) {
    selectProvider(openAIProvider)
}
```

**Dependencies**:
- DesignTokens (colors, corner radius, shadows, animation)
- StatusIndicator

---

### ProviderDetailPanel.swift
**Location**: `Aether/Sources/Components/Molecules/ProviderDetailPanel.swift`

**Purpose**: Detailed view of selected Provider with configuration and code examples.

**Features**:
- Provider name and status badge
- Collapsible sections (Description, Configuration, Code Example)
- Copy buttons for API endpoint and environment variables
- Edit and Delete actions

**Props**:
- `provider: Provider?` - Selected provider (nil if none)
- `onEdit: () -> Void` - Edit callback
- `onDelete: () -> Void` - Delete callback

**Usage**:
```swift
ProviderDetailPanel(
    provider: selectedProvider,
    onEdit: { editProvider(selectedProvider!) },
    onDelete: { deleteProvider(selectedProvider!) }
)
.frame(width: 350)
```

**Dependencies**:
- DesignTokens (colors, spacing, typography)
- ActionButton

---

### SidebarItem.swift
**Location**: `Aether/Sources/Components/Atoms/SidebarItem.swift`

**Purpose**: Sidebar navigation item with icon and text.

**Features**:
- Icon (SF Symbol) + Text layout
- Selected state: Blue background + blue left indicator bar
- Hover state: Subtle background change
- Smooth slide animation for indicator

**Props**:
- `icon: String` - SF Symbol name
- `title: String` - Tab title
- `isSelected: Bool` - Selection state
- `action: () -> Void` - Tap callback

**Usage**:
```swift
SidebarItem(
    icon: "gear",
    title: "General",
    isSelected: selectedTab == .general
) {
    selectedTab = .general
}
```

**Dependencies**:
- DesignTokens (colors, corner radius, spacing, animation)

---

### MemoryEntryCard.swift
**Location**: `Aether/Sources/MemoryView.swift` (nested component)

**Purpose**: Displays individual memory entry with app context and timestamp.

**Features**:
- App name and window title
- Timestamp (relative format: "2 hours ago")
- Input/output preview
- Delete button

**Props**:
- `entry: MemoryEntry` - Memory data
- `onDelete: () -> Void` - Delete callback

**Usage**:
```swift
MemoryEntryCard(entry: memoryEntry) {
    deleteMemory(memoryEntry.id)
}
```

**Dependencies**:
- DesignTokens

---

### SkeletonView.swift
**Location**: `Aether/Sources/Components/Molecules/SkeletonView.swift`

**Purpose**: Loading placeholder with shimmer animation.

**Features**:
- Pulsing shimmer effect
- Customizable size
- Light/dark mode adaptive colors

**Props**:
- `width: CGFloat` - Width of skeleton
- `height: CGFloat` - Height of skeleton
- `cornerRadius: CGFloat` - Corner radius (default: 8)

**Usage**:
```swift
// While loading
if isLoading {
    SkeletonView(width: 300, height: 80)
} else {
    ProviderCard(provider: provider)
}
```

**Dependencies**:
- DesignTokens (colors, animation)

---

### ToastView.swift
**Location**: `Aether/Sources/Components/Molecules/ToastView.swift`

**Purpose**: Temporary notification popup (success/error/info/warning).

**Features**:
- Four types: success (green), error (red), info (blue), warning (yellow)
- Icon + message
- Auto-dismiss after 3 seconds
- Slide-in/slide-out animation from bottom

**Props**:
- `message: String` - Notification text
- `type: ToastType` - `.success`, `.error`, `.info`, `.warning`
- `isShowing: Binding<Bool>` - Show/hide binding

**Usage**:
```swift
@State private var showToast = false
@State private var toastMessage = ""
@State private var toastType: ToastType = .success

// Trigger
showToast = true
toastMessage = "Provider deleted successfully"
toastType = .success

// In view
ToastView(message: toastMessage, type: toastType, isShowing: $showToast)
```

**Dependencies**:
- DesignTokens (colors, animation)

---

## Organisms (Complex Components)

### ModernSidebarView.swift
**Location**: `Aether/Sources/Components/Organisms/ModernSidebarView.swift`

**Purpose**: Full sidebar with navigation tabs and bottom actions.

**Features**:
- Top: App logo and version (optional)
- Middle: Navigation tab list
- Bottom: Import/Export/Reset action buttons
- Visual effect background (sidebar material)

**Props**:
- `selectedTab: Binding<SettingsTab>` - Current selected tab
- `tabs: [SettingsTab]` - List of tabs with icons and titles
- `onImport: () -> Void` - Import action
- `onExport: () -> Void` - Export action
- `onReset: () -> Void` - Reset action

**Usage**:
```swift
@State private var selectedTab: SettingsTab = .general

ModernSidebarView(
    selectedTab: $selectedTab,
    tabs: SettingsTab.allCases,
    onImport: importSettings,
    onExport: exportSettings,
    onReset: resetSettings
)
.frame(width: 200)
```

**Dependencies**:
- SidebarItem
- ActionButton
- VisualEffectBackground
- DesignTokens

---

### ProvidersView.swift (Redesigned)
**Location**: `Aether/Sources/ProvidersView.swift`

**Purpose**: Main Provider management interface with search, list, and detail panel.

**Features**:
- SearchBar at top
- ProviderCard list (scrollable, filterable)
- ProviderDetailPanel on right (appears when provider selected)
- Add Provider button
- Empty state view (no providers)
- Loading state (SkeletonView)
- Error state with retry
- Toast notifications for actions

**State**:
- `searchText: String` - Search query
- `selectedProvider: Provider?` - Currently selected provider
- `isLoading: Bool` - Loading state
- `showToast: Bool` - Toast visibility

**Layout**:
```
┌─────────────────────────────────────────────────┐
│ SearchBar                          [+ Add]       │
├─────────────────┬──────────────────────────────┤
│  ProviderCard   │                               │
│  ProviderCard   │  ProviderDetailPanel          │
│  ProviderCard   │  (appears when selected)      │
│  ...            │                               │
└─────────────────┴──────────────────────────────┘
```

**Dependencies**:
- SearchBar
- ProviderCard
- ProviderDetailPanel
- SkeletonView
- ToastView
- ActionButton
- DesignTokens

---

### RoutingView.swift (Modernized)
**Location**: `Aether/Sources/RoutingView.swift`

**Purpose**: Routing rules management with card-based layout.

**Features**:
- RuleCard component (nested)
- Drag-to-reorder rules
- Add/Edit/Delete operations
- Regex validation
- Provider tag display

**Dependencies**:
- ActionButton
- DesignTokens

---

### ShortcutsView.swift (Modernized)
**Location**: `Aether/Sources/ShortcutsView.swift`

**Purpose**: Global hotkey configuration and permissions.

**Features**:
- Hotkey recorder card
- Preset shortcuts list
- Permission status card
- Conflict detection warnings

**Dependencies**:
- ActionButton
- DesignTokens

---

### BehaviorSettingsView.swift (Modernized)
**Location**: `Aether/Sources/BehaviorSettingsView.swift`

**Purpose**: Application behavior configuration.

**Features**:
- Input mode card (Cut/Copy)
- Output mode card (Typewriter/Instant)
- Typing speed card with slider and preview
- PII scrubbing card

**Dependencies**:
- ActionButton
- DesignTokens

---

### MemoryView.swift (Modernized)
**Location**: `Aether/Sources/MemoryView.swift`

**Purpose**: Memory/context management interface.

**Features**:
- Configuration card (enable/disable, retention days)
- Statistics card (total entries, storage size)
- Memory browser with app filter
- MemoryEntryCard list
- Clear all functionality

**Dependencies**:
- MemoryEntryCard
- ActionButton
- DesignTokens

---

### SettingsView.swift (Main Container)
**Location**: `Aether/Sources/SettingsView.swift`

**Purpose**: Root settings window coordinating all tabs.

**Features**:
- NavigationSplitView layout
- ModernSidebarView integration
- ThemeSwitcher in toolbar (top-right)
- Tab content routing
- Theme state management

**Layout**:
```
┌────────────────────────────────────────────────────┐
│ Settings                         ☀️🌙◐ (Theme)    │
├────────────┬───────────────────────────────────────┤
│ ModernSide │                                        │
│ barView    │   Tab Content (Providers/Routing/etc) │
│            │                                        │
└────────────┴───────────────────────────────────────┘
```

**Dependencies**:
- ModernSidebarView
- ThemeSwitcher
- ThemeManager
- All tab views (ProvidersView, RoutingView, etc.)

---

## Dependency Graph

```
SettingsView
  ├── ThemeManager ✓
  ├── ThemeSwitcher
  │     └── ThemeManager ✓
  └── ModernSidebarView
        ├── SidebarItem
        │     └── DesignTokens ✓
        ├── ActionButton
        │     └── DesignTokens ✓
        └── VisualEffectBackground ✓

ProvidersView
  ├── SearchBar
  │     └── DesignTokens ✓
  ├── ProviderCard
  │     ├── StatusIndicator
  │     │     └── DesignTokens ✓
  │     └── DesignTokens ✓
  ├── ProviderDetailPanel
  │     ├── ActionButton ✓
  │     └── DesignTokens ✓
  ├── SkeletonView
  │     └── DesignTokens ✓
  └── ToastView
        └── DesignTokens ✓
```

**Legend**: ✓ = Foundation/already resolved

---

## File Structure

```
Aether/Sources/
├── DesignSystem/
│   ├── DesignTokens.swift        (Foundation)
│   └── ThemeManager.swift         (Foundation)
├── Components/
│   ├── Atoms/
│   │   ├── SearchBar.swift
│   │   ├── StatusIndicator.swift
│   │   ├── ActionButton.swift
│   │   ├── VisualEffectBackground.swift
│   │   ├── ThemeSwitcher.swift
│   │   └── SidebarItem.swift
│   ├── Molecules/
│   │   ├── ProviderCard.swift
│   │   ├── ProviderDetailPanel.swift
│   │   ├── SkeletonView.swift
│   │   └── ToastView.swift
│   └── Organisms/
│       └── ModernSidebarView.swift
├── SettingsView.swift             (Root container)
├── ProvidersView.swift            (Redesigned)
├── RoutingView.swift              (Modernized)
├── ShortcutsView.swift            (Modernized)
├── BehaviorSettingsView.swift     (Modernized)
├── MemoryView.swift               (Modernized)
└── GeneralSettingsView.swift      (To be created)
```

---

## Design Principles

1. **Single Responsibility**: Each component has one clear purpose
2. **Composition over Inheritance**: Complex components built from simple ones
3. **Consistent Styling**: All components use DesignTokens
4. **Reusability**: Atoms and Molecules designed for multiple contexts
5. **Accessibility**: All interactive elements support VoiceOver and keyboard navigation
6. **Performance**: Efficient rendering with minimal state

---

## Testing Strategy

### Unit Tests
- **DesignTokens**: Color values, spacing scale
- **ThemeManager**: Persistence, theme switching logic
- **Component Logic**: Search filtering, drag reordering, validation

### UI Tests
- **Interactive Components**: Button clicks, hover states, animations
- **Accessibility**: VoiceOver labels, keyboard navigation
- **Visual Tests**: Screenshot comparison across themes

### Integration Tests
- **Tab Navigation**: Sidebar → Content
- **Provider CRUD**: Add/Edit/Delete flow
- **Theme Switching**: Real-time appearance updates

---

## Maintenance Guidelines

### Adding a New Component

1. **Determine Layer**: Atom/Molecule/Organism?
2. **Create File**: Place in appropriate `Components/` subdirectory
3. **Use DesignTokens**: Never hardcode colors/spacing/fonts
4. **Add Documentation**: Purpose, props, usage example
5. **Update This Index**: Add to appropriate section
6. **Add Preview**: Include `#Preview` for development
7. **Add Tests**: Unit tests for logic, UI tests for interaction

### Modifying Existing Component

1. **Check Dependencies**: Use Dependency Graph above
2. **Update Documentation**: Reflect changes in this index
3. **Test Dependent Components**: Ensure no regressions
4. **Update Screenshots**: If visual changes made

### Deprecating a Component

1. **Add `@available` annotation**: Mark as deprecated
2. **Provide Alternative**: Suggest replacement component
3. **Update Index**: Move to "Deprecated" section
4. **Remove After Grace Period**: 2-3 release cycles

---

## Common Patterns

### Binding Data to Components

```swift
// Two-way binding
@State private var searchText = ""
SearchBar(searchText: $searchText)

// Callback pattern
ActionButton(title: "Save") {
    saveConfiguration()
}

// Observed object injection
@StateObject private var themeManager = ThemeManager()
ThemeSwitcher(themeManager: themeManager)
```

### Conditional Rendering

```swift
if isLoading {
    SkeletonView(width: 300, height: 80)
} else if providers.isEmpty {
    EmptyStateView()
} else {
    ForEach(providers) { provider in
        ProviderCard(provider: provider)
    }
}
```

### Animation

```swift
// Scale on hover
@State private var isHovered = false

ProviderCard(...)
    .scaleEffect(isHovered ? 1.02 : 1.0)
    .animation(.easeInOut(duration: DesignTokens.Animation.fast), value: isHovered)
    .onHover { isHovered = $0 }
```

---

**Document Version**: 1.0
**Last Updated**: 2025-12-26
**Maintained By**: Aether Development Team
