# Halo Theming Specification

## ADDED Requirements

### Requirement: Support Multiple Visual Themes
The system SHALL support three distinct visual themes (Cyberpunk, Zen, and Jarvis) that each provide a unique visual language while maintaining core functionality.

#### Scenario: User selects Cyberpunk theme in settings

**Given** the app is running with default Zen theme
**When** user opens Settings → General → Theme
**And** selects "Cyberpunk" from theme picker
**Then** Halo overlay updates to cyberpunk aesthetic
**And** theme selection persists across app restarts
**And** all state animations use neon colors (cyan, magenta, yellow)
**And** hexagonal Halo shape renders correctly

---

#### Scenario: Cyberpunk theme displays during processing state

**Given** Cyberpunk theme is active
**When** user triggers hotkey and AI begins processing
**Then** Halo displays hexagonal ring with magenta color
**And** scanline overlay effect is visible
**And** glitch effect triggers during state transitions
**And** animation runs at 60fps without dropped frames

---

### Requirement: Zen Theme Visual Experience
Zen theme SHALL provide a minimalist, calming visual experience with soft colors and organic animations.

#### Scenario: Zen theme renders breathing circle animation

**Given** Zen theme is active
**When** Halo enters listening state
**Then** circular ring appears with white color (opacity 0.8)
**And** breathing animation (scale 1.0 ↔ 1.2) repeats smoothly
**And** radial gradient fades from sage green to transparent
**And** no harsh edges or sharp transitions

---

### Requirement: Jarvis Theme Arc Reactor Aesthetic
Jarvis theme SHALL emulate Iron Man's arc reactor aesthetic with hexagonal HUD elements.

#### Scenario: Jarvis theme displays arc reactor blue during processing

**Given** Jarvis theme is active
**When** Halo enters processing state
**Then** six hexagonal segments appear around center
**And** segments animate in sequence (assembling effect)
**And** center displays pulsing blue core (#00d4ff)
**And** shadow glow effect enhances arc reactor look

---

### Requirement: Smooth Theme Transitions
Theme switching SHALL transition smoothly with crossfade animation lasting 0.5 seconds.

#### Scenario: Theme switch during active Halo display

**Given** Halo is visible in Cyberpunk theme processing state
**When** user switches to Zen theme in Settings
**Then** crossfade animation begins (duration 0.5s)
**And** old theme fades out with easeOut curve
**And** new theme fades in with easeIn curve
**And** no visual glitches or artifacts during transition
**And** Halo state (processing) is preserved

---

### Requirement: Persistent Theme Preferences
Theme preferences SHALL persist across app launches using UserDefaults.

#### Scenario: Saved theme loads on app startup

**Given** user previously selected Jarvis theme
**And** app was quit normally
**When** user launches app again
**Then** ThemeEngine initializes with Jarvis theme
**And** Halo overlay uses Jarvis theme immediately
**And** no flash of default theme on startup

---

### Requirement: HaloTheme Protocol Implementation
Each theme SHALL implement the HaloTheme protocol defining colors, shapes, and animations.

#### Scenario: New theme conformance to protocol

**Given** developer creates CustomTheme struct
**When** implementing HaloTheme protocol
**Then** theme must define listeningColor, processingColor, successColor, errorColor
**And** theme must implement listeningView(), processingView(), successView(), errorView()
**And** theme must define transitionDuration and pulseAnimation
**And** Swift compiler enforces all requirements

---

## Cross-References

- **Related Specs**: `macos-client` (NSWindow overlay foundation)
- **Depends On**: Phase 2 Halo overlay implementation
- **Blocks**: None
