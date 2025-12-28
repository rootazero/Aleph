# Specification: Halo Overlay

Transparent, animated overlay window that provides visual feedback at cursor location during AI processing.

## ADDED Requirements

### Requirement: Transparent Floating Window

The Halo overlay **SHALL** be a borderless, transparent NSWindow that floats above all applications.

**Why:** Core visual feedback mechanism - must be visible in any context.

**Acceptance criteria:**
- NSWindow with styleMask: .borderless
- backgroundColor: .clear, isOpaque: false
- window.level: .floating (above all apps)
- collectionBehavior: .canJoinAllSpaces, .ignoresCycle
- No shadow, no title bar

#### Scenario: Halo appears over active application

**Given** user is in Safari with text selected
**When** user presses Cmd+~
**Then** Halo window appears at cursor location
**And** Halo is visible above Safari window
**And** Halo works across all spaces/desktops

---

### Requirement: Click-Through Behavior

The Halo window **SHALL** never intercept mouse events or steal keyboard focus.

**Why:** Critical for "Ghost" aesthetic - must never interfere with user's work.

**Acceptance criteria:**
- ignoresMouseEvents = true
- Never calls makeKeyAndOrderFront()
- Uses orderFrontRegardless() to show
- Mouse clicks pass through to app below
- Keyboard focus remains in active app

#### Scenario: User interacts with app while Halo is visible

**Given** Halo is showing processing animation
**When** user clicks on Safari window beneath Halo
**Then** click is registered by Safari, not Halo
**And** Halo remains visible
**And** Safari retains keyboard focus

---

### Requirement: Cursor Position Tracking

The Halo **SHALL** appear at the exact mouse cursor location when summoned.

**Why:** Visual feedback must be contextual to where user is working.

**Acceptance criteria:**
- Use NSEvent.mouseLocation for coordinates
- Position window center at cursor point
- Clamp to screen bounds (handle edges)
- Support multi-monitor setups
- Update position only on show, not continuously

#### Scenario: Halo appears at cursor on secondary monitor

**Given** user has dual-monitor setup
**And** cursor is on secondary monitor
**When** user presses Cmd+~
**Then** Halo appears at cursor location on secondary monitor
**And** window is clamped to secondary monitor bounds

---

### Requirement: State Machine Animation

The Halo **SHALL** animate through distinct states reflecting processing status.

**Why:** Users need clear visual feedback on what Aether is doing.

**Acceptance criteria:**
- States: idle, listening, processing, success, error
- Smooth transitions between states
- State-specific animations:
  - Listening: Pulsing ring (500ms cycle)
  - Processing: Spinning animation
  - Success: Checkmark with fade out
  - Error: X icon with shake
- Automatic fade to idle after 2 seconds

#### Scenario: Normal processing flow

**Given** Halo is in idle state (invisible)
**When** hotkey is detected
**Then** Halo transitions to listening state
**And** pulsing ring animation starts
**When** AI processing begins
**Then** Halo transitions to processing state
**And** spinner animation starts
**When** AI response received
**Then** Halo transitions to success state
**And** checkmark appears
**After** 1.5 seconds
**Then** Halo fades out over 0.5 seconds
**And** returns to idle state

---

### Requirement: Provider-Specific Colors

The Halo **SHALL** support different colors based on the AI provider being used.

**Why:** Visual indicator of which AI model is processing the request.

**Acceptance criteria:**
- Processing state accepts Color parameter
- Predefined colors:
  - OpenAI: #10a37f (green)
  - Claude: #d97757 (orange)
  - Gemini: #4285F4 (blue)
  - Ollama: #000000 (black)
- Color applied to spinner/ring during processing

#### Scenario: Processing with Claude

**Given** routing rule directs request to Claude
**When** Halo enters processing state
**Then** spinner color is #d97757 (orange)
**And** animation uses Claude's brand color

---

### Requirement: SwiftUI View Architecture

The Halo view **SHALL** be implemented in SwiftUI with declarative state management.

**Why:** Clean, testable animation code with built-in transitions.

**Acceptance criteria:**
- HaloView is a SwiftUI View
- @State var state: HaloState
- Switch statement on state for view body
- Separate view components per state
- Built-in SwiftUI animations for transitions

#### Scenario: State change updates view

**Given** HaloView is rendering
**When** state changes from .listening to .processing(color: .green)
**Then** view body re-evaluates
**And** SpinnerView replaces PulsingRingView
**And** transition animation plays smoothly

---

### Requirement: Performance Constraints

The Halo **SHALL** render animations at 60 FPS without impacting system performance.

**Why:** Smooth animations are critical for professional feel.

**Acceptance criteria:**
- Animations run at 60 FPS
- CPU usage < 5% during animation
- No frame drops during state transitions
- Renders correctly on Retina displays
- No memory leaks after repeated show/hide

#### Scenario: Performance under load

**Given** user is running multiple apps
**When** Halo animates through all states
**Then** animations maintain 60 FPS
**And** CPU usage stays below 5%
**And** no stuttering or frame drops observed
