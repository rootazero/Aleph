# Implementation Tasks

## Phase 1: Foundation Components (不影响现有功能)

### 1.1 Create TrafficLightButton Component
- [ ] 1.1.1 Create `Aether/Sources/Components/Window/TrafficLightButton.swift`
- [ ] 1.1.2 Implement circular button view with 13pt diameter
- [ ] 1.1.3 Add gradient fill support (`.fill(color.gradient)`)
- [ ] 1.1.4 Implement hover state management (`@State private var isHovering`)
- [ ] 1.1.5 Add symbol icons for hover state (xmark, minus, arrow icons)
- [ ] 1.1.6 Configure font size (7pt bold) and color (`.black.opacity(0.7)`)
- [ ] 1.1.7 Use `.buttonStyle(.plain)` to avoid default styling
- [ ] 1.1.8 Add Xcode Preview for visual verification

**Validation:** Preview shows correct button appearance in both normal and hover states.

### 1.2 Create WindowController Bridge
- [ ] 1.2.1 Create `Aether/Sources/Components/Window/WindowController.swift`
- [ ] 1.2.2 Implement singleton pattern (`static let shared = WindowController()`)
- [ ] 1.2.3 Add `private func keyWindow() -> NSWindow?` helper using `NSApp.keyWindow`
- [ ] 1.2.4 Implement `func close()` using `window.performClose(nil)`
- [ ] 1.2.5 Implement `func minimize()` using `window.miniaturize(nil)`
- [ ] 1.2.6 Implement `func toggleFullscreen()` using `window.toggleFullScreen(nil)`
- [ ] 1.2.7 Add debug logging for nil window cases
- [ ] 1.2.8 Add inline documentation comments

**Validation:** Unit test or manual verification that methods call correct NSWindow APIs.

### 1.3 Create SidebarWithTrafficLights Component
- [ ] 1.3.1 Create `Aether/Sources/Components/Window/SidebarWithTrafficLights.swift`
- [ ] 1.3.2 Implement ZStack layout with `alignment: .topLeading`
- [ ] 1.3.3 Add RoundedRectangle background (cornerRadius: 18, style: .continuous)
- [ ] 1.3.4 Configure padding (.leading: 8, .vertical: 8)
- [ ] 1.3.5 Add strokeBorder overlay (`.separator.opacity(0.25)`)
- [ ] 1.3.6 Implement adaptive background color (Dark/Light Mode)
- [ ] 1.3.7 Add VStack for content layout
- [ ] 1.3.8 Position traffic light buttons (top: 14pt, leading: 18pt, spacing: 8pt)
- [ ] 1.3.9 Add placeholder navigation items (使用现有的 SidebarItem 或创建简化版本)
- [ ] 1.3.10 Set fixed width to 220pt
- [ ] 1.3.11 Add Xcode Preview for visual verification

**Validation:** Preview shows rounded sidebar with traffic lights correctly positioned.

### 1.4 Create RootContentView Component
- [ ] 1.4.1 Create `Aether/Sources/Components/Window/RootContentView.swift`
- [ ] 1.4.2 Implement HStack(spacing: 0) layout
- [ ] 1.4.3 Add SidebarWithTrafficLights() as left panel
- [ ] 1.4.4 Add Divider() as separator
- [ ] 1.4.5 Add MainContentView() as right panel (placeholder or integrate existing SettingsView logic)
- [ ] 1.4.6 Apply `.background(.windowBackground)` to root HStack
- [ ] 1.4.7 Add @State or @Binding for selectedTab navigation
- [ ] 1.4.8 Implement tab content switching logic (if not delegated to MainContentView)
- [ ] 1.4.9 Add Xcode Preview for full layout verification

**Validation:** Preview shows complete two-panel layout with working navigation.

## Phase 2: Window Scene Migration (Feature Flag Controlled)

### 2.1 Update AetherApp.swift
- [ ] 2.1.1 Add conditional compilation flag (`#if DEBUG` or custom flag)
- [ ] 2.1.2 Create new WindowGroup scene alongside existing Settings scene
- [ ] 2.1.3 Configure WindowGroup with RootContentView()
- [ ] 2.1.4 Apply `.windowStyle(.hiddenTitleBar)` modifier
- [ ] 2.1.5 Apply `.windowToolbarStyle(.unifiedCompact)` modifier
- [ ] 2.1.6 Set `.frame(minWidth: 800, minHeight: 500)` constraint
- [ ] 2.1.7 Verify both scenes can coexist (Debug uses new, Release uses old)
- [ ] 2.1.8 Update scene title/identifier if needed

**Validation:** Debug build shows new window style, Release build shows old style.

### 2.2 Update AppDelegate.swift
- [ ] 2.2.1 Locate `showSettings()` method
- [ ] 2.2.2 Update window activation logic to support WindowGroup (if needed)
- [ ] 2.2.3 Add window singleton management to prevent duplicate windows
- [ ] 2.2.4 Ensure menu bar "Settings..." item triggers new window correctly
- [ ] 2.2.5 Test window focus restoration on repeated menu clicks

**Validation:** Clicking "Settings..." in menu bar opens/focuses the new window.

### 2.3 Integrate Existing Settings Tabs
- [ ] 2.3.1 Review current SettingsView.swift tab content rendering logic
- [ ] 2.3.2 Extract tab content switching into MainContentView (or keep in RootContentView)
- [ ] 2.3.3 Pass `core`, `keychainManager`, and other dependencies to MainContentView
- [ ] 2.3.4 Ensure all existing tabs (General, Providers, Routing, etc.) render correctly
- [ ] 2.3.5 Verify @State and @Binding propagation for selectedTab
- [ ] 2.3.6 Test config hot-reload notification handling

**Validation:** All settings tabs display correctly in new window layout.

## Phase 3: Testing & Validation

### 3.1 Traffic Light Functionality Testing
- [ ] 3.1.1 Test red button: verify window closes correctly
- [ ] 3.1.2 Test yellow button: verify window minimizes to Dock
- [ ] 3.1.3 Test green button: verify fullscreen toggle works
- [ ] 3.1.4 Test hover state: verify icons appear on mouseover
- [ ] 3.1.5 Test keyboard shortcuts (Cmd+W, Cmd+M, Cmd+Ctrl+F) still work
- [ ] 3.1.6 Verify traffic lights respect window state (e.g., disabled when appropriate)

**Validation:** All traffic light actions behave identically to native controls.

### 3.2 Visual & Layout Testing
- [ ] 3.2.1 Test sidebar rounded corners in Light Mode
- [ ] 3.2.2 Test sidebar rounded corners in Dark Mode
- [ ] 3.2.3 Test sidebar background color opacity and blur
- [ ] 3.2.4 Test window resizing: ensure sidebar stays 220pt, content area adapts
- [ ] 3.2.5 Test minimum window size: verify 800x500 enforcement
- [ ] 3.2.6 Test divider visibility and positioning
- [ ] 3.2.7 Test traffic light button spacing and alignment
- [ ] 3.2.8 Test window drag area (sidebar non-button regions)

**Validation:** Visual appearance matches macOS 26 design language.

### 3.3 Cross-Version Compatibility Testing
- [ ] 3.3.1 Test on macOS 13 (Ventura) - minimum supported version
- [ ] 3.3.2 Test on macOS 14 (Sonoma)
- [ ] 3.3.3 Test on macOS 15 (Sequoia)
- [ ] 3.3.4 Test on macOS 26 (if available in beta)
- [ ] 3.3.5 Verify `.continuous` rounded corners render correctly on all versions
- [ ] 3.3.6 Verify adaptive background colors work on all versions
- [ ] 3.3.7 Check for any deprecated API warnings

**Validation:** No visual regressions or crashes on any supported macOS version.

### 3.4 Multi-Monitor & Edge Cases
- [ ] 3.4.1 Test window dragging between multiple displays
- [ ] 3.4.2 Test fullscreen mode on secondary monitor
- [ ] 3.4.3 Test window restoration after display disconnect/reconnect
- [ ] 3.4.4 Test with external mouse vs. trackpad hover behavior
- [ ] 3.4.5 Test accessibility: VoiceOver navigation of traffic lights
- [ ] 3.4.6 Test with reduced motion settings (System Preferences)

**Validation:** Window behaves correctly in all edge cases.

### 3.5 Functional Regression Testing
- [ ] 3.5.1 Test all settings tabs: General, Providers, Routing, Shortcuts, Behavior, Memory
- [ ] 3.5.2 Test provider add/edit/delete operations
- [ ] 3.5.3 Test routing rule management
- [ ] 3.5.4 Test shortcut customization
- [ ] 3.5.5 Test config import/export
- [ ] 3.5.6 Test config hot-reload on external file changes
- [ ] 3.5.7 Test theme switcher (if applicable)
- [ ] 3.5.8 Verify no existing functionality is broken

**Validation:** All settings features work identically to old implementation.

## Phase 4: Documentation & Finalization

### 4.1 Update Code Documentation
- [ ] 4.1.1 Add header comments to all new Swift files
- [ ] 4.1.2 Document WindowController singleton pattern
- [ ] 4.1.3 Document TrafficLightButton hover logic
- [ ] 4.1.4 Document SidebarWithTrafficLights layout calculations
- [ ] 4.1.5 Add inline comments for non-obvious implementation details

**Validation:** Code is self-documenting for future maintainers.

### 4.2 Update Project Documentation
- [ ] 4.2.1 Update CLAUDE.md: Document window design architecture
- [ ] 4.2.2 Update CLAUDE.md: Explain Settings → WindowGroup migration
- [ ] 4.2.3 Update CLAUDE.md: Add screenshots of new window design (optional)
- [ ] 4.2.4 Update README.md: Mention macOS 26 design language adoption
- [ ] 4.2.5 Update XCODEGEN_README.md: Note new Components/Window directory

**Validation:** Documentation accurately reflects new implementation.

### 4.3 Cleanup & Release Preparation
- [ ] 4.3.1 Remove feature flag (#if DEBUG) after all tests pass
- [ ] 4.3.2 Delete old Settings scene code from AetherApp.swift
- [ ] 4.3.3 Remove any unused imports or dead code
- [ ] 4.3.4 Run `swiftc` syntax validation on all changed files
- [ ] 4.3.5 Run Xcode "Clean Build Folder" and rebuild Release configuration
- [ ] 4.3.6 Verify no compiler warnings in Release build
- [ ] 4.3.7 Tag Git commit with change proposal reference

**Validation:** Clean build with zero warnings.

### 4.4 Archive Change Proposal
- [ ] 4.4.1 Confirm all tasks.md items are completed
- [ ] 4.4.2 Run `openspec validate adopt-macos26-window-design --strict`
- [ ] 4.4.3 Fix any validation errors
- [ ] 4.4.4 Archive change proposal using `openspec archive adopt-macos26-window-design`
- [ ] 4.4.5 Update affected specs (macos-client, settings-ui-layout) in `openspec/specs/`
- [ ] 4.4.6 Commit archived change and updated specs

**Validation:** OpenSpec validation passes, change successfully archived.

## Dependencies & Parallelization

**Sequential Dependencies:**
- Phase 1 → Phase 2 (must create components before integrating)
- Phase 2 → Phase 3 (must integrate before testing)
- Phase 3 → Phase 4 (must test before documenting/releasing)

**Parallelizable Work:**
- Within Phase 1: Tasks 1.1-1.4 can be done in parallel (independent components)
- Within Phase 3: Tasks 3.1-3.4 can be done in parallel (different test categories)

## Estimated Effort

- **Phase 1:** 4-6 hours (component development + previews)
- **Phase 2:** 2-3 hours (scene migration + integration)
- **Phase 3:** 4-6 hours (comprehensive testing across versions)
- **Phase 4:** 1-2 hours (documentation + cleanup)

**Total:** 11-17 hours (约 2-3 个工作日，适合单人完成)

## Rollback Plan

If critical issues are discovered in Phase 3:
1. Re-enable feature flag to switch back to old Settings scene
2. File bug report with detailed reproduction steps
3. Fix issues in separate branch
4. Re-test before removing feature flag

If issues persist after release:
1. Git revert to commit before Phase 2.1
2. Release hotfix with old Settings implementation
3. Schedule fix for next release cycle
