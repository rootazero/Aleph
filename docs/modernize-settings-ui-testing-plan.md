# Modernize Settings UI - Phase 6 Testing Plan

## Testing Overview

This document outlines the comprehensive testing strategy for the Modernize Settings UI change proposal. All tests must pass before marking Phase 6 as complete.

## Test Environment

- **macOS Versions**: 13 (Ventura), 14 (Sonoma), 15 (Sequoia)
- **Hardware**: Intel-based Mac, Apple Silicon Mac
- **Xcode Version**: 15.0+
- **Python**: $HOME/.python3/bin/python (for syntax validation)

## 6.1 Functional Testing

### 6.1.1 Test All Settings Tabs

#### General Tab
- [ ] Version number displays correctly
- [ ] Theme switcher shows three modes (Light/Dark/Auto)
- [ ] Theme switcher visual feedback on selection
- [ ] Selected theme persists after app restart
- [ ] Auto mode follows system appearance changes

#### Providers Tab
- [ ] Add new provider (OpenAI)
  - [ ] Modal opens correctly
  - [ ] Form validation works
  - [ ] Provider appears in list after saving
  - [ ] Provider card displays correct info
- [ ] Add new provider (Claude)
  - [ ] Same as above
- [ ] Add new provider (Ollama)
  - [ ] Same as above
- [ ] Edit existing provider
  - [ ] Modal pre-fills with existing data
  - [ ] Changes save correctly
  - [ ] Card updates in real-time
- [ ] Delete provider
  - [ ] Confirmation dialog appears
  - [ ] Provider removed from list
  - [ ] Config file updated
- [ ] Test provider connection
  - [ ] Success scenario: Green status indicator
  - [ ] Failure scenario: Red status with error message
  - [ ] Loading state during test
- [ ] Search providers
  - [ ] Search by name filters correctly
  - [ ] Search by type filters correctly
  - [ ] Clear search restores all providers
  - [ ] Empty state shows when no matches
- [ ] Detail panel
  - [ ] Clicking card shows detail panel
  - [ ] Detail panel displays correct info
  - [ ] Copy buttons work (API endpoint, env vars)
  - [ ] Edit button opens modal
  - [ ] Delete button shows confirmation

#### Routing Tab
- [ ] Add new routing rule
  - [ ] Modal opens
  - [ ] Regex validation works
  - [ ] Rule appears in list
- [ ] Edit routing rule
  - [ ] Modal pre-fills
  - [ ] Changes save
- [ ] Delete routing rule
  - [ ] Confirmation works
  - [ ] Rule removed
- [ ] Drag to reorder rules
  - [ ] Visual feedback during drag
  - [ ] Order persists after save
- [ ] Rule card hover effect
  - [ ] Scale animation smooth
  - [ ] Shadow deepens

#### Shortcuts Tab
- [ ] Global hotkey recorder
  - [ ] Click to record
  - [ ] Captures key combination
  - [ ] Displays formatted shortcut
  - [ ] Saves to config
- [ ] Conflict detection
  - [ ] Warning shown for system shortcuts
  - [ ] Warning card styled correctly
- [ ] Permission card
  - [ ] Shows accessibility permission status
  - [ ] "Open Settings" button works

#### Behavior Tab
- [ ] Input mode selection
  - [ ] Cut mode selectable
  - [ ] Copy mode selectable
  - [ ] Selection persists
- [ ] Output mode selection
  - [ ] Typewriter mode selectable
  - [ ] Instant mode selectable
  - [ ] Selection persists
- [ ] Typing speed slider
  - [ ] Slider adjustable
  - [ ] Value displays correctly
  - [ ] Preview button works
  - [ ] Speed persists
- [ ] PII scrubbing toggle
  - [ ] Toggle works
  - [ ] State persists

#### Memory Tab
- [ ] Memory configuration card
  - [ ] Toggle enable/disable
  - [ ] Retention days slider
  - [ ] Settings save
- [ ] Memory statistics card
  - [ ] Displays total entries
  - [ ] Displays storage size
- [ ] Memory browser
  - [ ] App filter dropdown works
  - [ ] Memory entries display
  - [ ] Delete individual entry works
  - [ ] Clear all with confirmation

### 6.1.2 Test Configuration Persistence

- [ ] Modify multiple settings
- [ ] Close settings window
- [ ] Quit and restart Aether
- [ ] Reopen settings window
- [ ] Verify all changes persisted
- [ ] Check `~/.config/aether/config.toml` content matches

### 6.1.3 Test Import/Export

- [ ] Export settings
  - [ ] Button opens file save dialog
  - [ ] JSON file created correctly
  - [ ] All settings included in export
- [ ] Import settings
  - [ ] Button opens file picker
  - [ ] Valid JSON imports successfully
  - [ ] Settings overwrite existing config
  - [ ] UI updates to reflect imported settings
- [ ] Import invalid settings
  - [ ] Error dialog shows
  - [ ] Existing settings unchanged
  - [ ] Error message descriptive

### 6.1.4 Test Reset Function

- [ ] Click "Reset Settings" button
- [ ] Confirmation dialog appears
- [ ] Dialog warns about data loss
- [ ] Cancel button preserves settings
- [ ] Confirm button resets to defaults
- [ ] Verify `config.toml` reset
- [ ] UI reflects default values

## 6.2 Visual Testing

### 6.2.1 Light Mode Testing

- [ ] Switch to Light mode using ThemeSwitcher
- [ ] General tab: All elements visible and readable
- [ ] Providers tab: Cards render correctly
  - [ ] Card backgrounds appropriate
  - [ ] Text has good contrast
  - [ ] Shadows visible
- [ ] Routing tab: Rule cards visible
- [ ] Shortcuts tab: Recorder visible
- [ ] Behavior tab: Controls visible
- [ ] Memory tab: Cards visible
- [ ] Sidebar: Background and icons correct
- [ ] Visual effect blur appropriate for light background
- [ ] No white-on-white text issues
- [ ] Status indicators clearly visible

### 6.2.2 Dark Mode Testing

- [ ] Switch to Dark mode using ThemeSwitcher
- [ ] General tab: All elements visible and readable
- [ ] Providers tab: Cards render correctly
  - [ ] Card backgrounds appropriate
  - [ ] Text has good contrast
  - [ ] Shadows visible against dark background
  - [ ] Borders visible
- [ ] Routing tab: Rule cards visible
- [ ] Shortcuts tab: Recorder visible
- [ ] Behavior tab: Controls visible
- [ ] Memory tab: Cards visible
- [ ] Sidebar: Background and icons correct
- [ ] Visual effect blur appropriate for dark background
- [ ] No black-on-black text issues
- [ ] Status indicators clearly visible

### 6.2.3 Auto Mode Testing

- [ ] Switch to Auto mode using ThemeSwitcher
- [ ] Open System Preferences > General > Appearance
- [ ] Change to Light
  - [ ] Aether switches to Light immediately
  - [ ] No flicker or delay
- [ ] Change to Dark
  - [ ] Aether switches to Dark immediately
  - [ ] No flicker or delay
- [ ] Change back to Auto/System
  - [ ] Aether follows system
- [ ] Verify transition smooth

### 6.2.4 Theme Switcher Interaction

- [ ] Click Sun icon (Light)
  - [ ] Icon highlights with blue background
  - [ ] Other icons deselect
  - [ ] Theme applies immediately
- [ ] Click Moon icon (Dark)
  - [ ] Same as above
- [ ] Click Half-circle icon (Auto)
  - [ ] Same as above
- [ ] Hover over unselected icons
  - [ ] Subtle background change
- [ ] Animation smooth
  - [ ] No jerky transitions
  - [ ] Frame rate appears 60fps
- [ ] Quit and restart Aether
  - [ ] Theme preference restored
  - [ ] Correct icon highlighted

### 6.2.5 Window Size Testing

#### Minimum Size (800x600)
- [ ] Resize window to 800x600
- [ ] Sidebar visible and not truncated
- [ ] Content area scrollable if needed
- [ ] Detail panel collapses or scrolls
- [ ] ThemeSwitcher visible in toolbar
- [ ] No horizontal scrollbars
- [ ] No overlapping elements
- [ ] Text not truncated mid-word

#### Ideal Size (1200x800)
- [ ] Resize to 1200x800
- [ ] Sidebar: Content: Detail ≈ 1:2:1.5 ratio
- [ ] All content comfortably visible
- [ ] No excessive whitespace
- [ ] Cards use available space well

#### Maximum Size (Fullscreen)
- [ ] Enter fullscreen mode
- [ ] Content doesn't stretch excessively
- [ ] Elements remain centered/aligned
- [ ] Sidebar doesn't become too wide
- [ ] Detail panel doesn't exceed max width

### 6.2.6 Screenshot Comparison

- [ ] Open reference `uisample.png`
- [ ] Take screenshots of each tab
- [ ] Compare visual elements:
  - [ ] Card corner radius matches
  - [ ] Spacing between elements similar
  - [ ] Color palette consistent
  - [ ] Typography hierarchy similar
  - [ ] Shadow depth similar
- [ ] Take screenshots in all three themes:
  - [ ] `screenshots/light-mode-providers.png`
  - [ ] `screenshots/dark-mode-providers.png`
  - [ ] `screenshots/auto-mode-providers.png`
- [ ] Archive screenshots in `docs/screenshots/`

## 6.3 Performance Testing

### 6.3.1 Instruments Profiling

#### Time Profiler
- [ ] Launch Instruments with Time Profiler template
- [ ] Record 30-second session:
  - [ ] Navigate through all tabs
  - [ ] Search providers
  - [ ] Scroll provider list
  - [ ] Toggle theme modes
- [ ] Analyze results:
  - [ ] No single function > 100ms
  - [ ] UI thread not blocked
  - [ ] Identify top 5 hotspots
- [ ] Document findings

#### Core Animation
- [ ] Launch Instruments with Core Animation template
- [ ] Record session:
  - [ ] Hover over provider cards
  - [ ] Click sidebar items
  - [ ] Toggle theme switcher
  - [ ] Open/close detail panel
- [ ] Check frame rate:
  - [ ] All animations maintain 60fps
  - [ ] No dropped frames during transitions
  - [ ] GPU usage reasonable (< 30%)
- [ ] Document findings

#### Allocations
- [ ] Launch Instruments with Allocations template
- [ ] Record session:
  - [ ] Open settings window
  - [ ] Navigate all tabs
  - [ ] Close window
  - [ ] Repeat 10 times
- [ ] Check for leaks:
  - [ ] No persistent memory growth
  - [ ] Deallocation occurs on window close
  - [ ] No leaked NSWindow or NSView instances
- [ ] Run Leaks instrument
  - [ ] Zero leaks reported
- [ ] Document findings

### 6.3.2 Large Dataset Performance

- [ ] Manually create config with 50+ providers
  - [ ] Script to generate providers:
    ```bash
    # Add to config.toml
    for i in {1..50}; do
      echo "[providers.test$i]"
      echo "api_key = \"sk-test-$i\""
      echo "model = \"gpt-4\""
      echo ""
    done >> ~/.config/aether/config.toml
    ```
- [ ] Open Providers tab
- [ ] Measure scroll performance:
  - [ ] Smooth scrolling (no jank)
  - [ ] Card rendering instant
  - [ ] No visible lag
- [ ] Search with term matching 25+ providers
  - [ ] Results appear instantly (< 50ms)
  - [ ] No UI freeze
  - [ ] Filter animation smooth
- [ ] Measure memory usage:
  - [ ] Activity Monitor: Memory < 200MB
  - [ ] No excessive swap
- [ ] Document findings

### 6.3.3 Animation Smoothness

- [ ] Provider card hover
  - [ ] Scale animation smooth
  - [ ] No stuttering
  - [ ] Shadow transition smooth
- [ ] Sidebar selection
  - [ ] Blue indicator slides smoothly
  - [ ] No jumping
  - [ ] Timing feels natural (300ms)
- [ ] Detail panel appear/disappear
  - [ ] Fade in smooth
  - [ ] Slide transition smooth
  - [ ] No flicker
- [ ] Search results filter
  - [ ] Cards fade out smoothly
  - [ ] Cards move without jumping
  - [ ] Feels responsive
- [ ] Theme switching
  - [ ] Color transitions smooth
  - [ ] No white/black flash
  - [ ] All elements update simultaneously
- [ ] Window resize
  - [ ] Layout updates smoothly
  - [ ] No janky element repositioning
  - [ ] Constraints resolve quickly

### 6.3.4 Low-End Device Testing

- [ ] Test on 2020 Intel MacBook Air (if available)
  - [ ] All animations run at acceptable frame rate
  - [ ] No thermal issues during use
  - [ ] UI remains responsive
  - [ ] Fan doesn't spin up excessively
- [ ] Throttle CPU in Xcode Debug:
  - [ ] Simulator > Debug > Slow Animations
  - [ ] Verify animations still smooth at 10x slowdown
- [ ] Document findings

## 6.4 Compatibility Testing

### 6.4.1 macOS 13 (Ventura) Testing

- [ ] Build on macOS 13 SDK
  - [ ] No build errors
  - [ ] No deprecation warnings
- [ ] Run on macOS 13 device/VM:
  - [ ] App launches successfully
  - [ ] All SwiftUI features available
  - [ ] Visual effects render correctly
  - [ ] ThemeSwitcher works
  - [ ] No API unavailable crashes
  - [ ] SF Symbols render (fallback if needed)
- [ ] Document any version-specific issues

### 6.4.2 macOS 14 (Sonoma) Testing

- [ ] Build on macOS 14 SDK
  - [ ] No build errors
  - [ ] Utilize Sonoma APIs if available
- [ ] Run on macOS 14:
  - [ ] All features work
  - [ ] Performance improvements visible
  - [ ] New SF Symbols available
- [ ] Document enhancements

### 6.4.3 macOS 15 (Sequoia) Testing

- [ ] Build on macOS 15 SDK
  - [ ] No build errors
  - [ ] Utilize latest APIs
- [ ] Run on macOS 15:
  - [ ] Full compatibility
  - [ ] No regressions
  - [ ] Latest design language supported
- [ ] Document any issues

## 6.5 Accessibility Testing

### 6.5.1 VoiceOver Testing

- [ ] Enable VoiceOver (Cmd+F5)
- [ ] Navigate settings window:
  - [ ] Sidebar items announced correctly
  - [ ] "General", "Providers", etc. readable
- [ ] Providers tab:
  - [ ] Provider cards announced
  - [ ] "Provider: OpenAI, Status: Active" format
  - [ ] Search bar announced
  - [ ] Add/Edit/Delete buttons readable
- [ ] Detail panel:
  - [ ] All labels readable
  - [ ] Copy buttons announced
  - [ ] Section headers announced
- [ ] ThemeSwitcher:
  - [ ] "Light mode", "Dark mode", "Auto mode" announced
  - [ ] Selected state announced
- [ ] Tab navigation order:
  - [ ] Logical order (left to right, top to bottom)
  - [ ] No unreachable elements
  - [ ] Focus visible
- [ ] Test all interactive elements:
  - [ ] Buttons activatable via VoiceOver
  - [ ] Sliders adjustable
  - [ ] Toggles switchable

### 6.5.2 Keyboard Navigation Testing

- [ ] Tab key navigation:
  - [ ] Tab cycles through all controls
  - [ ] Shift+Tab reverses direction
  - [ ] Focus order logical
  - [ ] Focus visible (blue outline)
- [ ] Arrow key navigation:
  - [ ] Up/Down in sidebar changes selection
  - [ ] Up/Down in provider list scrolls
  - [ ] Left/Right in sliders adjusts value
- [ ] Spacebar activation:
  - [ ] Buttons activate
  - [ ] Toggles switch
  - [ ] Checkboxes check/uncheck
- [ ] Return/Enter:
  - [ ] Primary buttons activate
  - [ ] Modals open
  - [ ] Forms submit
- [ ] Escape key:
  - [ ] Modals close
  - [ ] Focus returns to trigger
- [ ] Test keyboard shortcuts:
  - [ ] Cmd+, opens Settings (macOS standard)
  - [ ] Cmd+W closes window
  - [ ] Cmd+Q quits (only if appropriate)

### 6.5.3 Contrast Testing

- [ ] Use Accessibility Inspector:
  - [ ] Xcode > Open Developer Tool > Accessibility Inspector
- [ ] Check contrast ratios in Light mode:
  - [ ] Primary text: >= 4.5:1 (WCAG AA)
  - [ ] Secondary text: >= 4.5:1
  - [ ] Button text: >= 4.5:1
  - [ ] Status indicators: >= 3:1
  - [ ] Card borders: >= 3:1
- [ ] Check contrast ratios in Dark mode:
  - [ ] Same requirements as Light mode
- [ ] Use Color Contrast Analyzer tool
- [ ] Document any failures
- [ ] Fix contrast issues if found

## Test Execution Checklist

### Pre-Testing Setup
- [ ] Clean build directory: `xcodebuild clean`
- [ ] Regenerate Xcode project: `xcodegen generate`
- [ ] Build Rust core: `cd Aether/core && cargo build --release`
- [ ] Generate UniFFI bindings
- [ ] Copy dylib to Frameworks
- [ ] Build Xcode project
- [ ] Reset config file to defaults

### During Testing
- [ ] Record all issues in GitHub Issues
- [ ] Take screenshots of visual bugs
- [ ] Use screen recording for interaction bugs
- [ ] Note macOS version for each bug
- [ ] Note device specs for performance issues

### Post-Testing
- [ ] Review all test results
- [ ] Fix all critical bugs (P0)
- [ ] Fix all high-priority bugs (P1)
- [ ] Document known issues (P2/P3)
- [ ] Update tasks.md with test completion status
- [ ] Archive test artifacts (screenshots, Instruments traces)

## Success Criteria

Phase 6 is considered complete when:
- [ ] All functional tests pass (6.1)
- [ ] All visual tests pass in 3 theme modes (6.2)
- [ ] Performance benchmarks met:
  - [ ] 60fps animations
  - [ ] Search < 50ms
  - [ ] No memory leaks
  - [ ] Smooth on low-end devices
- [ ] Compatible with macOS 13-15 (6.4)
- [ ] All accessibility tests pass:
  - [ ] VoiceOver functional
  - [ ] Keyboard navigable
  - [ ] WCAG AA contrast met
- [ ] Zero critical bugs (P0)
- [ ] < 3 high-priority bugs (P1)

## Test Artifacts

Store in `docs/testing/phase6/`:
- [ ] `functional-test-results.md` - Detailed test results
- [ ] `visual-test-screenshots/` - Screenshots for each theme
- [ ] `performance-test-results.md` - Instruments data
- [ ] `accessibility-test-results.md` - VoiceOver/Keyboard results
- [ ] `compatibility-matrix.md` - OS version compatibility
- [ ] `known-issues.md` - Documented bugs and workarounds

---

**Document Version**: 1.0
**Last Updated**: 2025-12-26
**Maintained By**: Aether Development Team
