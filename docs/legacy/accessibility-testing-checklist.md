# Accessibility Testing Checklist - Modernize Settings UI

## Purpose

This document provides comprehensive accessibility testing procedures to ensure the Modernized Settings UI is fully accessible to users with disabilities, complying with WCAG 2.1 AA standards.

## Test Environment

### Required Tools
- **macOS Accessibility Features**: VoiceOver, Keyboard Access, Display Settings
- **Xcode Accessibility Inspector**: Xcode > Open Developer Tool > Accessibility Inspector
- **Color Contrast Analyzer**: [Download from TPGi](https://www.tpgi.com/color-contrast-checker/)
- **Screen Recording**: For documenting issues

### Pre-Test Setup
1. Build Aleph in Release mode
2. Enable Accessibility features:
   - VoiceOver: Cmd+F5
   - Full Keyboard Access: System Preferences > Keyboard > Shortcuts > "Use keyboard navigation to move focus between controls"
3. Open Accessibility Inspector in Xcode

---

## 6.5.1 VoiceOver Testing

### Setup VoiceOver
1. Press Cmd+F5 to enable VoiceOver
2. Open VoiceOver Utility (Cmd+F8) if needed to adjust settings
3. Launch Aleph and open Settings window

### General VoiceOver Navigation

#### Window and App Identification
- [ ] **App Name**: VoiceOver announces "Aether" when app gains focus
- [ ] **Window Title**: Announces "Settings" or similar
- [ ] **Window Role**: Identified as "window"

#### Sidebar Navigation
- [ ] Navigate to sidebar using VoiceOver cursor (Ctrl+Option+Arrow keys)
- [ ] **General Tab**:
  - [ ] Announced as: "General, tab" or "General, button"
  - [ ] State announced: "selected" when active
- [ ] **Providers Tab**:
  - [ ] Announced as: "Providers, tab"
  - [ ] State announced correctly
- [ ] **Routing Tab**: Announced correctly
- [ ] **Shortcuts Tab**: Announced correctly
- [ ] **Behavior Tab**: Announced correctly
- [ ] **Memory Tab**: Announced correctly

**Navigation Order**:
- [ ] Tabs announced in logical order (top to bottom)
- [ ] VO+Right Arrow moves through tabs sequentially
- [ ] VO+Left Arrow moves backward

#### Bottom Action Buttons
- [ ] **Import Settings**: Announced as "Import Settings, button"
- [ ] **Export Settings**: Announced as "Export Settings, button"
- [ ] **Reset Settings**: Announced as "Reset Settings, button" with hint "Warning: This will reset all settings to defaults"

### Theme Switcher

- [ ] Navigate to ThemeSwitcher in toolbar
- [ ] **Light Mode Button**:
  - [ ] Announced as: "Light mode, button" or "Switch to light mode, button"
  - [ ] State: "selected" when active, "not selected" when inactive
- [ ] **Dark Mode Button**:
  - [ ] Announced as: "Dark mode, button" or "Switch to dark mode, button"
  - [ ] State announced correctly
- [ ] **Auto Mode Button**:
  - [ ] Announced as: "Auto mode, button" or "Follow system appearance, button"
  - [ ] State announced correctly

**Interaction**:
- [ ] VO+Space activates button
- [ ] Selection state change announced immediately: "Selected" or theme name

### Providers Tab

#### Search Bar
- [ ] Navigate to search field
- [ ] Announced as: "Search providers, search field" or similar
- [ ] Role identified as "search field" or "text field"
- [ ] Current value announced if text entered
- [ ] Clear button (X) announced: "Clear, button"

#### Provider Cards
- [ ] Navigate to provider card list
- [ ] **First Provider Card**:
  - [ ] Announced with format: "OpenAI, provider, active" or similar
  - [ ] Includes provider name
  - [ ] Includes status (active/inactive)
  - [ ] Role identified: "button" or "group"
- [ ] **Multiple Cards**:
  - [ ] Each card announced distinctly
  - [ ] Count announced: "Provider 1 of 5" (if supported)

**Provider Card Details**:
- [ ] Provider icon: Decorative, should be hidden from VoiceOver or have empty label
- [ ] Provider name: Announced
- [ ] Provider type: Announced (e.g., "OpenAI")
- [ ] Status indicator: Announced (e.g., "Active" or "Inactive")
- [ ] Description: Announced or accessible via Rotor

#### Detail Panel
- [ ] Select a provider card (VO+Space)
- [ ] Detail panel appearance announced: "Detail panel appeared" or similar
- [ ] Navigate into detail panel
- [ ] **Section Headers**:
  - [ ] "Configuration" announced as heading
  - [ ] "Code Example" announced as heading
- [ ] **Labels and Values**:
  - [ ] "API Endpoint: https://api.openai.com/v1" announced
  - [ ] "Model: gpt-4" announced
- [ ] **Copy Buttons**:
  - [ ] Announced as: "Copy API endpoint, button"
  - [ ] Action announced after click: "Copied to clipboard"
- [ ] **Edit Button**: Announced as "Edit provider, button"
- [ ] **Delete Button**: Announced as "Delete provider, button"

### Routing Tab

- [ ] Navigate to Routing tab
- [ ] Rule cards announced:
  - [ ] "Rule 1: Match /draw, use OpenAI"
  - [ ] Pattern included
  - [ ] Provider included
- [ ] **Add Rule Button**: Announced correctly
- [ ] **Edit Button**: For each rule, announced with context
- [ ] **Delete Button**: For each rule, announced with context

### Shortcuts Tab

- [ ] **Hotkey Recorder**:
  - [ ] Announced as: "Current shortcut: Command Grave" or similar
  - [ ] Instructions announced: "Click to record new shortcut"
- [ ] **Record Button**: Announced and activatable
- [ ] **Permission Card**:
  - [ ] Status announced: "Accessibility permission: Granted" or "Denied"
  - [ ] "Open System Settings" button announced

### Behavior Tab

- [ ] **Input Mode**:
  - [ ] Radio buttons or picker announced
  - [ ] Options: "Cut mode, radio button, selected"
  - [ ] Options: "Copy mode, radio button, not selected"
- [ ] **Output Mode**:
  - [ ] Same as above for Typewriter/Instant
- [ ] **Typing Speed Slider**:
  - [ ] Announced as: "Typing speed, 50 characters per second, slider"
  - [ ] Current value announced when adjusted
  - [ ] Min/max announced: "Minimum 50, maximum 400"
- [ ] **PII Scrubbing Toggle**:
  - [ ] Announced as: "Enable PII scrubbing, checkbox" or "toggle"
  - [ ] State announced: "checked" or "unchecked"

### Memory Tab

- [ ] **Enable Memory Toggle**: Announced with state
- [ ] **Retention Days Slider**: Value announced
- [ ] **Memory Entries List**:
  - [ ] Each entry announced with app name and timestamp
  - [ ] Delete button for each entry announced
- [ ] **Clear All Button**: Announced with warning hint

### Modal Dialogs

- [ ] Open "Add Provider" modal (if accessible)
- [ ] **Modal Announcement**: "Add Provider dialog" or similar
- [ ] **Form Fields**:
  - [ ] "Provider name, text field" announced
  - [ ] "API Key, secure text field" announced
  - [ ] Labels associated with fields
- [ ] **Buttons**:
  - [ ] "Cancel, button"
  - [ ] "Save, button" (primary button indicated if possible)
- [ ] **Close Modal**: Esc key or Cancel button, closure announced

### Error States

- [ ] Trigger an error (e.g., invalid regex in routing rule)
- [ ] Error message announced by VoiceOver
- [ ] Error associated with relevant field
- [ ] Error can be navigated to and read

### Loading States

- [ ] If SkeletonView or ProgressView shown:
  - [ ] Loading state announced: "Loading providers" or similar
  - [ ] Completion announced: "Providers loaded"

---

## 6.5.2 Keyboard Navigation Testing

### Full Keyboard Access Setup
1. System Preferences > Keyboard > Shortcuts
2. Enable "Use keyboard navigation to move focus between controls"
3. Verify Tab key cycles through all controls

### Tab Key Navigation

#### Forward Tab Order
Starting from Settings window open:
1. [ ] Press Tab
   - Focus moves to first control (likely sidebar or ThemeSwitcher)
   - Focus ring visible (blue outline)
2. [ ] Continue pressing Tab
   - [ ] Cycles through all interactive elements
   - [ ] Order is logical: top-left → top-right → down
   - [ ] Sidebar tabs
   - [ ] Content area controls
   - [ ] Detail panel (if visible)
   - [ ] Bottom action buttons

#### Reverse Tab Order
1. [ ] Press Shift+Tab
   - Focus moves backward
   - [ ] Reverse order matches forward order

#### Tab Traps
- [ ] No "tab trap" where focus gets stuck
- [ ] Can tab into and out of all sections
- [ ] Modals: Tab cycles within modal, doesn't escape to background

### Arrow Key Navigation

#### Sidebar Navigation
- [ ] Click or Tab to sidebar
- [ ] Press Down Arrow
  - [ ] Moves to next tab
- [ ] Press Up Arrow
  - [ ] Moves to previous tab
- [ ] Top tab + Up Arrow:
  - [ ] Wraps to bottom OR stops at top (both acceptable)
- [ ] Bottom tab + Down Arrow:
  - [ ] Wraps to top OR stops at bottom

#### Provider Cards List
- [ ] Tab to provider cards area
- [ ] Press Down Arrow
  - [ ] Selects next provider card
  - [ ] Detail panel updates
- [ ] Press Up Arrow
  - [ ] Selects previous card

#### Sliders (Typing Speed, Retention Days)
- [ ] Tab to slider
- [ ] Press Right Arrow
  - [ ] Increases value
  - [ ] Visual update immediate
- [ ] Press Left Arrow
  - [ ] Decreases value
- [ ] Press Shift+Arrow
  - [ ] Larger increment (if implemented)

### Spacebar Activation

- [ ] Tab to any button (e.g., "Add Provider")
- [ ] Press Spacebar
  - [ ] Button activates (same as click)
- [ ] Tab to checkbox/toggle
- [ ] Press Spacebar
  - [ ] Toggles state

### Return/Enter Activation

- [ ] Tab to primary button (e.g., "Save" in modal)
- [ ] Press Return/Enter
  - [ ] Button activates
- [ ] Tab to text field
- [ ] Type text, press Return
  - [ ] Expected action occurs (e.g., search filters)

### Escape Key

- [ ] Open modal dialog
- [ ] Press Escape
  - [ ] Modal closes
  - [ ] Focus returns to trigger button
- [ ] Open search field
- [ ] Type text, press Escape
  - [ ] Clears search OR closes field (depending on design)

### Command Key Shortcuts

- [ ] Press Cmd+W (close window)
  - [ ] Settings window closes (or alerts if unsaved changes)
- [ ] Press Cmd+, (open settings)
  - [ ] Settings window opens (if app supports)
- [ ] Press Cmd+Q (quit app)
  - [ ] App quits (or settings window closes, depending on design)

### Focus Visibility

- [ ] Tab through all controls
- [ ] **Check Focus Ring**:
  - [ ] All focused elements have visible focus indicator
  - [ ] Focus ring color contrasts with background (blue on light, blue on dark)
  - [ ] Focus ring not obscured by other elements
  - [ ] No elements with invisible focus

### Focus Trapping in Modals

- [ ] Open "Add Provider" modal
- [ ] Tab through all modal controls
  - [ ] Focus stays within modal
  - [ ] Cannot tab to background window
- [ ] Shift+Tab backward
  - [ ] Cycles within modal
- [ ] Close modal (Esc or Cancel)
  - [ ] Focus returns to element that opened modal (e.g., "Add Provider" button)

---

## 6.5.3 Color Contrast Testing

### Tools Setup
1. Install Color Contrast Analyzer (CCA)
2. Or use online tool: https://webaim.org/resources/contrastchecker/
3. Use Xcode Accessibility Inspector > Color Contrast

### WCAG 2.1 AA Standards
- **Normal Text** (< 18pt or < 14pt bold): Minimum 4.5:1 contrast ratio
- **Large Text** (≥ 18pt or ≥ 14pt bold): Minimum 3:1 contrast ratio
- **UI Components** (buttons, borders): Minimum 3:1 contrast ratio

### Light Mode Contrast Tests

#### Background and Text
- [ ] **Primary Text on Card Background**:
  - Foreground: __________ (e.g., #1C1C1E)
  - Background: __________ (e.g., #FFFFFF)
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

- [ ] **Secondary Text on Card Background**:
  - Foreground: __________ (e.g., #8E8E93)
  - Background: __________
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

- [ ] **Sidebar Text on Sidebar Background**:
  - Foreground: __________
  - Background: __________
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

#### Buttons
- [ ] **Primary Button Text** (e.g., Save button):
  - Foreground: __________ (white)
  - Background: __________ (blue #007AFF)
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

- [ ] **Secondary Button Text**:
  - Foreground: __________
  - Background: __________
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

- [ ] **Danger Button Text** (Delete):
  - Foreground: __________
  - Background: __________ (red)
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

#### Status Indicators
- [ ] **Green Status Dot** (Active):
  - Foreground: __________ (green)
  - Background: __________ (card background)
  - Ratio: ______:1
  - [ ] Pass (≥ 3:1) / Fail

- [ ] **Gray Status Dot** (Inactive):
  - Foreground: __________ (gray)
  - Background: __________
  - Ratio: ______:1
  - [ ] Pass (≥ 3:1) / Fail

#### Borders and UI Components
- [ ] **Card Borders**:
  - Foreground: __________ (border color)
  - Background: __________ (window background)
  - Ratio: ______:1
  - [ ] Pass (≥ 3:1) / Fail

- [ ] **Selected Card Border** (blue):
  - Ratio: ______:1
  - [ ] Pass (≥ 3:1) / Fail

### Dark Mode Contrast Tests

Repeat all above tests in Dark mode:

#### Background and Text (Dark Mode)
- [ ] **Primary Text on Card Background**:
  - Foreground: __________ (e.g., #FFFFFF)
  - Background: __________ (e.g., #2C2C2E)
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

- [ ] **Secondary Text on Card Background**:
  - Foreground: __________ (e.g., #AEAEB2)
  - Background: __________
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

- [ ] **Sidebar Text on Sidebar Background**:
  - Foreground: __________
  - Background: __________
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

#### Buttons (Dark Mode)
- [ ] **Primary Button Text**:
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

- [ ] **Secondary Button Text**:
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

- [ ] **Danger Button Text**:
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

#### Status Indicators (Dark Mode)
- [ ] **Green Status Dot**:
  - Ratio: ______:1
  - [ ] Pass (≥ 3:1) / Fail

- [ ] **Gray Status Dot**:
  - Ratio: ______:1
  - [ ] Pass (≥ 3:1) / Fail

#### Borders (Dark Mode)
- [ ] **Card Borders**:
  - Ratio: ______:1
  - [ ] Pass (≥ 3:1) / Fail
  - Note: May need visible borders in dark mode since shadows aren't effective

### Focus Indicators
- [ ] **Focus Ring in Light Mode**:
  - Foreground: __________ (blue)
  - Background: __________ (varies)
  - Ratio: ______:1
  - [ ] Pass (≥ 3:1) / Fail

- [ ] **Focus Ring in Dark Mode**:
  - Ratio: ______:1
  - [ ] Pass (≥ 3:1) / Fail

### Special Cases

#### Search Bar Placeholder Text
- [ ] **Placeholder in Light Mode**:
  - Foreground: __________
  - Background: __________
  - Ratio: ______:1
  - Note: Placeholder text can be < 4.5:1 (WCAG allows 3:1 for non-essential text)
  - [ ] Pass (≥ 3:1) / Fail

- [ ] **Placeholder in Dark Mode**:
  - Ratio: ______:1
  - [ ] Pass (≥ 3:1) / Fail

#### Link Text (if any)
- [ ] **Link Color**:
  - Foreground: __________
  - Background: __________
  - Ratio: ______:1
  - [ ] Pass (≥ 4.5:1) / Fail

### Using Xcode Accessibility Inspector

1. Open Xcode > Open Developer Tool > Accessibility Inspector
2. Click "Audit" tab
3. Select Aleph.app window
4. Click "Run Audit"
5. Review "Color Contrast" issues
6. [ ] All issues resolved or documented

---

## 6.5.4 Accessibility Labels and Hints

### Using Accessibility Inspector

1. Open Accessibility Inspector
2. Enable "Inspection Mode" (crosshair icon)
3. Hover over each UI element

### Check Each Element

#### Buttons
- [ ] **Add Provider Button**:
  - Label: "Add Provider" (descriptive)
  - Role: Button
  - Hint: "Opens dialog to add new AI provider" (optional)
  - [ ] Pass / Fail

- [ ] **Theme Switcher Buttons**:
  - Light: Label "Light mode" or "Switch to light mode"
  - Dark: Label "Dark mode" or "Switch to dark mode"
  - Auto: Label "Auto mode" or "Follow system appearance"
  - [ ] Pass / Fail

#### Images/Icons
- [ ] **Provider Icons**:
  - If decorative: Label empty "" (VoiceOver ignores)
  - If functional: Descriptive label
  - [ ] Pass / Fail

- [ ] **Status Indicators**:
  - Label: "Active" or "Inactive" (or empty if redundant)
  - [ ] Pass / Fail

#### Text Fields
- [ ] **Search Field**:
  - Label: "Search providers" (not placeholder)
  - Placeholder: "Search..." (supplemental)
  - [ ] Pass / Fail

- [ ] **Provider Name Field** (in modal):
  - Label: "Provider name"
  - Associated with label element
  - [ ] Pass / Fail

#### Sliders
- [ ] **Typing Speed Slider**:
  - Label: "Typing speed"
  - Value: "50 characters per second" (descriptive)
  - [ ] Pass / Fail

#### Custom Controls (Provider Cards)
- [ ] **Provider Card**:
  - Label: "OpenAI provider, active" (includes state)
  - Role: Button (or custom)
  - Children accessible (name, status, etc.)
  - [ ] Pass / Fail

---

## Accessibility Test Results Template

```markdown
# Accessibility Testing Results - Phase 6.5

**Tester**: [Name]
**Date**: [YYYY-MM-DD]
**macOS**: [15.2 / etc.]
**VoiceOver Version**: [Check in VoiceOver Utility]

## 6.5.1 VoiceOver Testing

### Sidebar Navigation
- [ ] Pass / [ ] Fail
- Issues: _______________________

### Theme Switcher
- [ ] Pass / [ ] Fail
- Issues: _______________________

### Providers Tab
- [ ] Pass / [ ] Fail
- Search bar: Pass / Fail
- Provider cards: Pass / Fail
- Detail panel: Pass / Fail

### Routing Tab
- [ ] Pass / [ ] Fail
- Issues: _______________________

### Shortcuts Tab
- [ ] Pass / [ ] Fail
- Issues: _______________________

### Behavior Tab
- [ ] Pass / [ ] Fail
- Issues: _______________________

### Memory Tab
- [ ] Pass / [ ] Fail
- Issues: _______________________

### Modal Dialogs
- [ ] Pass / [ ] Fail
- Issues: _______________________

**Overall VoiceOver**: Pass / Fail

---

## 6.5.2 Keyboard Navigation

### Tab Key Navigation
- [ ] Pass / [ ] Fail
- Tab order logical: Yes / No
- All controls reachable: Yes / No

### Arrow Key Navigation
- [ ] Pass / [ ] Fail
- Sidebar: Pass / Fail
- Provider list: Pass / Fail
- Sliders: Pass / Fail

### Spacebar & Return
- [ ] Pass / [ ] Fail
- Buttons activate: Yes / No
- Toggles work: Yes / No

### Escape Key
- [ ] Pass / [ ] Fail
- Modals close: Yes / No
- Focus returns: Yes / No

### Focus Visibility
- [ ] Pass / [ ] Fail
- All focused elements visible: Yes / No
- Focus ring contrast sufficient: Yes / No

**Overall Keyboard Nav**: Pass / Fail

---

## 6.5.3 Color Contrast

### Light Mode
- [ ] Pass / [ ] Fail
- Failed elements: _______________________

### Dark Mode
- [ ] Pass / [ ] Fail
- Failed elements: _______________________

### Xcode Audit
- [ ] Pass / [ ] Fail
- Issues found: _____ (target: 0)

**Overall Contrast**: Pass / Fail

---

## 6.5.4 Accessibility Labels

- [ ] Pass / [ ] Fail
- Buttons labeled: Yes / No
- Images labeled appropriately: Yes / No
- Form fields labeled: Yes / No
- Custom controls labeled: Yes / No

**Overall Labels**: Pass / Fail

---

## Summary

- [ ] All accessibility tests PASS
- [ ] Minor issues (document below)
- [ ] Major issues requiring fixes

### Issues Found
1. [Description and severity]
2. [Description and severity]

### Action Items
1. [Fix needed]
2. [Fix needed]
```

### Save Results
`docs/testing/phase6/accessibility-test-results-[DATE].md`

---

## Common Accessibility Issues and Fixes

### Issue: Button has no label
**Fix**: Add `.accessibilityLabel("Descriptive label")` in SwiftUI

### Issue: Image icon read by VoiceOver
**Fix**: Add `.accessibilityHidden(true)` if decorative

### Issue: Focus ring not visible
**Fix**: Ensure `focusRingType` is not `.none`, verify color contrast

### Issue: Tab order illogical
**Fix**: Adjust view hierarchy or use `.accessibilitySort Order()`

### Issue: Contrast ratio fails
**Fix**: Adjust colors in `DesignTokens.swift` to meet 4.5:1 minimum

---

## Approval Criteria

Accessibility testing PASSES when:
- ✅ VoiceOver: All elements announced correctly, navigation logical
- ✅ Keyboard: All functions accessible via keyboard, focus visible
- ✅ Contrast: All text ≥ 4.5:1, UI components ≥ 3:1 (WCAG AA)
- ✅ Labels: All interactive elements have descriptive labels
- ✅ Xcode Audit: Zero critical accessibility issues

**Approved By**: ___________
**Date**: ___________
