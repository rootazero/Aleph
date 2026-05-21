# Visual Testing Guide - Modernize Settings UI

## Purpose

This document provides detailed instructions for manual visual testing of the Modernized Settings UI across all three theme modes (Light, Dark, Auto) and various window configurations.

## Test Environment Setup

### Required Tools
- macOS device or VM with versions 13, 14, and 15
- Xcode 15.0+
- Screenshot utility (Cmd+Shift+4)
- Color contrast analyzer (optional but recommended)

### Pre-Test Configuration
1. Build Aleph application: `xcodegen generate && xcodebuild -project Aleph.xcodeproj -scheme Aleph build`
2. Reset config to defaults: `rm ~/.aleph/config.toml`
3. Launch Aleph application
4. Open Settings window (Cmd+,)

## Test Procedures

### 6.2.1 Light Mode Visual Tests

#### Setup
1. Locate ThemeSwitcher in top-right toolbar
2. Click the Sun icon (Light mode button)
3. Verify button highlights with blue background
4. Wait 0.5 seconds for theme to fully apply

#### Checklist

**General Tab**
- [ ] Background color: Light gray (#F5F5F7) or similar
- [ ] Text readable: Dark text on light background
- [ ] Version number clearly visible
- [ ] ThemeSwitcher sun icon highlighted in blue
- [ ] No white-on-white text issues

**Providers Tab**
- [ ] Search bar visible with light background
- [ ] Provider cards have white/light background
- [ ] Card shadows visible (subtle but present)
- [ ] Card corners rounded (≈10pt radius)
- [ ] Provider names in dark text
- [ ] Status indicators clearly visible:
  - [ ] Green dot for active
  - [ ] Gray dot for inactive
- [ ] Hover effect visible: Card scales to 1.02, shadow deepens
- [ ] Selected card has blue border (2pt)

**Detail Panel**
- [ ] Panel background light
- [ ] Section headers readable
- [ ] Code blocks have light gray background
- [ ] Copy buttons visible
- [ ] Edit/Delete buttons clear

**Routing Tab**
- [ ] Rule cards have light background
- [ ] Regex patterns readable
- [ ] Provider tags colored correctly
- [ ] Drag handles visible

**Shortcuts Tab**
- [ ] Hotkey recorder has light background
- [ ] Current shortcut displayed clearly
- [ ] Warning cards (if any) have yellow background
- [ ] Permission status clear

**Behavior Tab**
- [ ] Radio buttons/toggles visible
- [ ] Slider track and thumb clear
- [ ] Section dividers subtle but visible

**Memory Tab**
- [ ] Configuration card readable
- [ ] Statistics display clear
- [ ] Memory entries list readable
- [ ] App filter dropdown visible

**Sidebar**
- [ ] Sidebar background slightly darker than content (#E8E8EA or similar)
- [ ] Selected tab has blue accent bar on left
- [ ] Tab icons visible in dark color
- [ ] Tab text readable
- [ ] Bottom action buttons clear

**Visual Effect Blur**
- [ ] Sidebar blur subtle (sidebar material)
- [ ] No excessive blur making text unreadable
- [ ] Blur adapts to light content behind

#### Screenshot Checklist
Take and save the following screenshots to `docs/screenshots/light-mode/`:
- [ ] `01-general.png`
- [ ] `02-providers-list.png`
- [ ] `03-providers-detail.png`
- [ ] `04-routing.png`
- [ ] `05-shortcuts.png`
- [ ] `06-behavior.png`
- [ ] `07-memory.png`
- [ ] `08-fullscreen.png`

### 6.2.2 Dark Mode Visual Tests

#### Setup
1. Click the Moon icon (Dark mode button) in ThemeSwitcher
2. Verify button highlights with blue background
3. Wait 0.5 seconds for theme to fully apply

#### Checklist

**General Tab**
- [ ] Background color: Dark gray (#1C1C1E or similar)
- [ ] Text readable: Light text on dark background
- [ ] Version number clearly visible in light color
- [ ] ThemeSwitcher moon icon highlighted in blue

**Providers Tab**
- [ ] Search bar visible with dark background
- [ ] Provider cards have dark background (#2C2C2E or similar)
- [ ] Card shadows visible against dark background (may need borders)
- [ ] Card borders visible if shadows not sufficient
- [ ] Provider names in light text
- [ ] Status indicators clearly visible:
  - [ ] Green dot bright enough
  - [ ] Gray dot visible against dark background
- [ ] Hover effect visible: Card scales, shadow deepens or border brightens
- [ ] Selected card has blue border (2pt)

**Detail Panel**
- [ ] Panel background dark
- [ ] Section headers readable in light text
- [ ] Code blocks have slightly lighter dark background
- [ ] Copy buttons visible with borders
- [ ] Edit/Delete buttons clear

**Routing Tab**
- [ ] Rule cards have dark background
- [ ] Regex patterns readable in light text
- [ ] Provider tags visible with sufficient contrast
- [ ] Drag handles visible

**Shortcuts Tab**
- [ ] Hotkey recorder has dark background
- [ ] Current shortcut displayed in light text
- [ ] Warning cards have dark yellow/amber background
- [ ] Permission status clear

**Behavior Tab**
- [ ] Radio buttons/toggles visible with light accents
- [ ] Slider track visible against dark background
- [ ] Section dividers visible (light gray)

**Memory Tab**
- [ ] Configuration card readable
- [ ] Statistics display clear in light text
- [ ] Memory entries list readable
- [ ] App filter dropdown visible

**Sidebar**
- [ ] Sidebar background darker than content (#1C1C1E or similar)
- [ ] Selected tab has blue accent bar on left
- [ ] Tab icons visible in light color
- [ ] Tab text readable in light color
- [ ] Bottom action buttons clear with borders

**Visual Effect Blur**
- [ ] Sidebar blur appropriate for dark mode
- [ ] Blur doesn't wash out content
- [ ] Blur adapts to dark content behind

**Critical Dark Mode Checks**
- [ ] No black-on-black text (all text has minimum 4.5:1 contrast)
- [ ] Borders visible where shadows aren't effective
- [ ] No pure black (#000000) backgrounds (use dark grays)
- [ ] Status indicators bright enough to see

#### Screenshot Checklist
Save to `docs/screenshots/dark-mode/`:
- [ ] `01-general.png`
- [ ] `02-providers-list.png`
- [ ] `03-providers-detail.png`
- [ ] `04-routing.png`
- [ ] `05-shortcuts.png`
- [ ] `06-behavior.png`
- [ ] `07-memory.png`
- [ ] `08-fullscreen.png`

### 6.2.3 Auto Mode Visual Tests

#### Setup
1. Click the Half-circle icon (Auto mode button) in ThemeSwitcher
2. Verify button highlights with blue background

#### Checklist

**System Light to Dark Transition**
1. [ ] Open System Preferences > General > Appearance
2. [ ] Select "Light" in Appearance dropdown
3. [ ] Switch back to Aleph
4. [ ] Verify Aleph is in Light mode (check background colors)
5. [ ] Switch to System Preferences
6. [ ] Select "Dark" in Appearance dropdown
7. [ ] Switch back to Aleph within 1 second
8. [ ] Verify Aleph switches to Dark mode:
   - [ ] Transition is immediate (< 0.5s)
   - [ ] No white flash during transition
   - [ ] No black flash during transition
   - [ ] All elements update simultaneously
   - [ ] No elements "left behind" in wrong theme
9. [ ] Verify ThemeSwitcher still shows Half-circle highlighted

**System Dark to Light Transition**
1. [ ] Ensure system is in Dark mode
2. [ ] Verify Aleph is in Dark mode
3. [ ] Switch to System Preferences
4. [ ] Select "Light" in Appearance dropdown
5. [ ] Switch back to Aleph
6. [ ] Verify smooth transition to Light mode:
   - [ ] Transition smooth
   - [ ] No flicker
   - [ ] All elements update

**Auto Mode Persistence**
1. [ ] Verify Auto mode selected in ThemeSwitcher
2. [ ] Quit Aleph (Cmd+Q)
3. [ ] Relaunch Aleph
4. [ ] Verify Auto mode still selected
5. [ ] Verify theme matches system appearance
6. [ ] Change system appearance
7. [ ] Verify Aleph follows

#### Screenshot Checklist
Save to `docs/screenshots/auto-mode/`:
- [ ] `01-auto-light.png` (Auto mode while system is Light)
- [ ] `02-auto-dark.png` (Auto mode while system is Dark)
- [ ] `03-transition-sequence.mov` (Screen recording of transition)

### 6.2.4 Theme Switcher Interaction Tests

#### Visual Feedback
1. [ ] Hover over unselected theme button
   - [ ] Subtle background color change
   - [ ] No jarring color shift
2. [ ] Click Light mode button
   - [ ] Button background turns blue
   - [ ] Other buttons deselect (no blue background)
   - [ ] Icon remains visible in white/light color
3. [ ] Click Dark mode button
   - [ ] Same visual feedback as Light
4. [ ] Click Auto mode button
   - [ ] Same visual feedback
5. [ ] Rapid switching (Light → Dark → Auto → repeat 5x)
   - [ ] All transitions smooth
   - [ ] No animation lag
   - [ ] No visual glitches
   - [ ] Buttons always show correct state

#### Animation Smoothness
- [ ] Use 120Hz or 240fps screen recording to verify 60fps
- [ ] Theme transition completes within 300ms
- [ ] No frame drops visible to naked eye
- [ ] Color transitions use smooth interpolation

### 6.2.5 Window Size Tests

#### Minimum Size (800x600)
1. [ ] Resize window to 800 width, 600 height
2. [ ] Verify layout:
   - [ ] Sidebar width: ~200pt (fixed or minimum)
   - [ ] Content area: Remaining width (min 400pt)
   - [ ] Detail panel: Collapses or becomes scrollable
   - [ ] ThemeSwitcher visible in toolbar
   - [ ] No horizontal scrollbar in content area
   - [ ] No overlapping UI elements
   - [ ] Text not truncated (wraps or scrolls)
3. [ ] Navigate all tabs
4. [ ] Verify all functional

#### Small Window (1000x700)
1. [ ] Resize to 1000x700
2. [ ] Verify comfortable reading
3. [ ] Detail panel visible but may be narrow
4. [ ] All controls accessible

#### Ideal Size (1200x800)
1. [ ] Resize to 1200x800
2. [ ] Verify balanced layout:
   - [ ] Sidebar: ~200pt
   - [ ] Content: ~650pt
   - [ ] Detail: ~350pt
3. [ ] All content comfortably visible
4. [ ] No excessive whitespace
5. [ ] Cards use space well

#### Large Window (1600x1000)
1. [ ] Resize to 1600x1000
2. [ ] Verify layout scales:
   - [ ] Content centered or well-distributed
   - [ ] Cards don't become excessively wide
   - [ ] Detail panel doesn't exceed max width (~500pt)
   - [ ] Text line length reasonable (< 80ch)

#### Fullscreen Mode
1. [ ] Press Cmd+Ctrl+F to enter fullscreen
2. [ ] Verify layout:
   - [ ] Sidebar and content well-proportioned
   - [ ] No extreme stretching
   - [ ] ThemeSwitcher visible
   - [ ] Exit fullscreen button accessible
3. [ ] Press Cmd+Ctrl+F to exit
4. [ ] Verify window returns to previous size

### 6.2.6 Screenshot Comparison with Reference

#### Compare to uisample.png
1. [ ] Open `docs/uisample.png` in Preview
2. [ ] Open Aleph Settings Providers tab
3. [ ] Position windows side-by-side
4. [ ] Compare visually:

**Layout**
- [ ] Sidebar width similar proportion
- [ ] Provider cards have similar height/width ratio
- [ ] Card spacing (padding/margins) similar
- [ ] Detail panel layout similar

**Typography**
- [ ] Font sizes match reference hierarchy
- [ ] Provider name: Large/bold
- [ ] Description: Smaller/regular
- [ ] Section headers: Medium/semibold

**Colors**
- [ ] Accent blue similar (#007AFF or close)
- [ ] Card backgrounds similar tone
- [ ] Status indicators match (green/gray/red)
- [ ] Sidebar background similar

**Visual Effects**
- [ ] Card corner radius matches (~10pt)
- [ ] Shadow depth similar (subtle)
- [ ] Card spacing (gaps) similar (~12-16pt)

**Differences Noted** (Document any intentional differences):
- [ ] ___________________________
- [ ] ___________________________

#### Create Reference Archive
1. [ ] Create `docs/screenshots/reference/` directory
2. [ ] Save current screenshots for future comparison
3. [ ] Include metadata:
   ```
   Date: 2025-12-26
   macOS Version: 15.2
   Aleph Version: 1.0.0
   Theme Modes: Light, Dark, Auto
   Window Size: 1200x800
   ```

## Test Results Documentation

### Results Template

```markdown
# Visual Testing Results - Phase 6.2

**Tester**: [Your Name]
**Date**: [YYYY-MM-DD]
**macOS Version**: [13.x / 14.x / 15.x]
**Aleph Version**: [x.x.x]
**Device**: [MacBook Pro 16" 2023 / etc.]

## 6.2.1 Light Mode
- [ ] Pass / [ ] Fail
- Issues: [None / List issues]
- Screenshots: [Saved to docs/screenshots/light-mode/]

## 6.2.2 Dark Mode
- [ ] Pass / [ ] Fail
- Issues: [None / List issues]
- Screenshots: [Saved to docs/screenshots/dark-mode/]

## 6.2.3 Auto Mode
- [ ] Pass / [ ] Fail
- Issues: [None / List issues]
- Transition Quality: [Smooth / Acceptable / Poor]

## 6.2.4 Theme Switcher
- [ ] Pass / [ ] Fail
- Animation Quality: [60fps / <60fps / Laggy]

## 6.2.5 Window Sizes
- [ ] Minimum (800x600): Pass / Fail
- [ ] Ideal (1200x800): Pass / Fail
- [ ] Fullscreen: Pass / Fail

## 6.2.6 Reference Comparison
- [ ] Pass / [ ] Fail
- Differences: [List any significant differences]

## Overall Assessment
- [ ] All visual tests PASS
- [ ] Visual tests PASS with minor issues
- [ ] Visual tests FAIL - requires fixes

## Action Items
1. [Issue to fix]
2. [Issue to fix]
```

### Save Results
- Save as: `docs/testing/phase6/visual-test-results-[DATE].md`

## Common Issues and Fixes

### Issue: Text Not Readable in Dark Mode
**Fix**: Increase contrast by adjusting text color or background color in `DesignTokens.swift`

### Issue: Shadows Not Visible
**Fix**: Increase shadow opacity or add borders as fallback in dark mode

### Issue: Theme Transition Flickers
**Fix**: Check animation timing in `ThemeManager.swift`, ensure all views update simultaneously

### Issue: Layout Breaks at Small Sizes
**Fix**: Add minimum width constraints in SwiftUI views

### Issue: ThemeSwitcher Buttons Too Close
**Fix**: Adjust spacing in `ThemeSwitcher.swift`

## Approval Criteria

All visual tests PASS when:
- ✅ All checkboxes marked in Light, Dark, and Auto modes
- ✅ Screenshots archived and match reference quality
- ✅ No P0 or P1 visual bugs
- ✅ Theme transitions smooth with no flicker
- ✅ Contrast ratios meet WCAG AA (4.5:1 for text)
- ✅ Layout works at all tested window sizes

**Approved By**: ___________
**Date**: ___________
