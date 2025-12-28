# Halo Customization Specification

## ADDED Requirements

### Requirement: Customize Halo Size with Preset Options
User SHALL be able to customize Halo size with three preset options: Small (80px), Medium (120px), Large (160px).

#### Scenario: Size customization in Settings

**Given** user opens Settings → General → Halo Appearance
**When** user selects "Large" from size picker
**Then** HaloPreferences.size updates to .large
**And** HaloView.frame updates to 160x160 immediately
**And** if Halo is visible, resize animation plays (spring curve)
**And** setting persists to UserDefaults
**And** after app restart, Halo loads with Large size

---

### Requirement: Customize Halo Opacity with Slider
User SHALL be able to customize Halo opacity from 50% to 100%.

#### Scenario: Opacity slider adjustment

**Given** user opens Settings → Halo Appearance
**When** user drags opacity slider to 75%
**Then** HaloPreferences.opacity updates to 0.75
**And** HaloView.opacity updates immediately
**And** if Halo is visible, opacity changes smoothly (0.3s duration)
**And** setting persists across app restarts

---

### Requirement: Customize Animation Speed with Presets
User SHALL be able to customize animation speed with three presets: Slow (1.5x), Normal (1.0x), Fast (0.7x).

#### Scenario: Animation speed adjustment

**Given** user opens Settings → Halo Appearance
**When** user selects "Fast" from speed picker
**Then** HaloPreferences.animationSpeed updates to 0.7
**And** all SwiftUI animations multiply duration by 0.7
**And** state transitions complete 30% faster
**And** setting applies immediately to active animations

---

### Requirement: HaloPreferences Persistence in UserDefaults
HaloPreferences SHALL be stored as Codable struct in UserDefaults under "haloPreferences" key.

#### Scenario: Preferences persistence

**Given** user has customized size=Large, opacity=0.8, speed=Fast
**When** app quits normally
**Then** PreferencesManager encodes HaloPreferences struct to JSON
**And** saves to UserDefaults with key "haloPreferences"
**When** app launches again
**Then** PreferencesManager decodes JSON from UserDefaults
**And** restores all custom settings
**And** no flash of default settings on startup

---

### Requirement: Reset to Factory Defaults Button
Settings UI SHALL provide "Reset to Defaults" button to restore factory settings.

#### Scenario: Reset button restores defaults

**Given** user has customized multiple Halo preferences
**When** user clicks "Reset to Defaults" button in Settings
**Then** confirmation alert appears: "Restore default Halo settings?"
**When** user confirms
**Then** HaloPreferences resets to:
  - size: .medium (120px)
  - opacity: 1.0 (100%)
  - animationSpeed: 1.0 (Normal)
**And** HaloView updates immediately
**And** UserDefaults updated

---

### Requirement: Real-Time Preference Application
Preferences SHALL apply in real-time without requiring app restart.

#### Scenario: Live preview of customization

**Given** Halo is visible in processing state
**And** user opens Settings (Settings window appears)
**When** user drags opacity slider
**Then** Halo opacity updates immediately (every slider value change)
**And** user sees live preview of changes
**And** no lag or dropped frames during adjustment

---

## Cross-References

- **Related Specs**: `macos-client` (Settings UI), `halo-theming` (HaloView rendering)
- **Depends On**: UserDefaults (built-in persistence)
- **Blocks**: None
