# Specification: Settings UI

SwiftUI-based settings interface for configuring Aleph's AI providers, routing rules, and shortcuts.

## ADDED Requirements

### Requirement: Tab-Based Settings Window

The settings UI **SHALL** use a tab-based layout for organizing configuration options.

**Why:** Clear organization of different configuration categories.

**Acceptance criteria:**
- SwiftUI TabView with 4 tabs
- Tabs: General, Providers, Routing, Shortcuts
- Window size: 600x500 (fixed)
- Window title: "Aleph Settings"
- Accessible via menu bar "Settings" item

#### Scenario: Opening settings

**Given** Aleph is running
**When** user clicks "Settings" in menu bar
**Then** settings window opens at 600x500 size
**And** General tab is selected by default
**And** all 4 tabs are visible

---

### Requirement: General Settings Tab

The General tab **SHALL** provide basic application preferences.

**Why:** Core app configuration like theme and sounds.

**Acceptance criteria:**
- Theme selector (placeholder - disabled for Phase 2)
- Sound effects toggle (placeholder - disabled)
- "Check for Updates" button (disabled)
- Version number display
- Layout uses SwiftUI Form

#### Scenario: Viewing general settings

**Given** settings window is open
**When** user selects General tab
**Then** version number is displayed
**And** theme selector shows "Cyberpunk" (disabled)
**And** sound toggle shows OFF (disabled)
**And** "Check for Updates" button is grayed out

---

### Requirement: Providers Configuration Tab

The Providers tab **SHALL** display available AI providers with placeholder configuration.

**Why:** Users need to see which providers are available (even if not configurable yet).

**Acceptance criteria:**
- List of providers: OpenAI, Claude, Gemini, Ollama
- Each provider shows:
  - Name
  - API key status (placeholder: "Not Configured")
  - Configure button (disabled, shows "Coming Soon" alert)
- Layout uses List view

#### Scenario: Viewing providers

**Given** settings window is open
**When** user selects Providers tab
**Then** 4 providers are listed
**And** each shows "API Key: Not Configured"
**When** user clicks "Configure" for OpenAI
**Then** alert shows "Coming Soon in Phase 4"

---

### Requirement: Routing Rules Tab

The Routing tab **SHALL** display hardcoded routing rules with read-only access.

**Why:** Users should see routing logic even if not editable yet.

**Acceptance criteria:**
- List of routing rules (hardcoded examples)
- Each rule shows:
  - Pattern (e.g., "^/draw")
  - Provider (e.g., "OpenAI")
- "Add Rule" button (disabled with tooltip)
- Layout uses List view

#### Scenario: Viewing routing rules

**Given** settings window is open
**When** user selects Routing tab
**Then** 3 example rules are displayed:
  - Pattern: "^/draw" → Provider: "OpenAI"
  - Pattern: "^/code" → Provider: "Claude"
  - Pattern: ".*" → Provider: "OpenAI" (catch-all)
**And** "Add Rule" button is disabled
**When** user hovers over "Add Rule"
**Then** tooltip shows "Available in Phase 4"

---

### Requirement: Shortcuts Configuration Tab

The Shortcuts tab **SHALL** display the current global hotkey with placeholder customization.

**Why:** Users should know the hotkey even if they can't change it yet.

**Acceptance criteria:**
- Display current hotkey: "⌘ + ~"
- "Change Hotkey" button (disabled)
- Explanation text about Accessibility permissions
- Link to System Settings for permissions

#### Scenario: Viewing shortcuts

**Given** settings window is open
**When** user selects Shortcuts tab
**Then** current hotkey "⌘ + ~" is displayed
**And** "Change Hotkey" button is grayed out
**And** text explains Accessibility permission requirement
**When** user clicks "Open System Settings"
**Then** System Settings opens to Accessibility pane

---

### Requirement: Responsive Layout

The settings UI **SHALL** adapt to different window sizes while maintaining usability.

**Why:** Users may resize the window for better view.

**Acceptance criteria:**
- Minimum window size: 500x400
- Maximum window size: 800x700
- Content scales appropriately
- No overlapping UI elements
- Scroll views where needed

#### Scenario: Resizing settings window

**Given** settings window is open at 600x500
**When** user resizes window to 700x600
**Then** all tabs remain visible and usable
**And** content scales proportionally
**And** no UI elements overlap
