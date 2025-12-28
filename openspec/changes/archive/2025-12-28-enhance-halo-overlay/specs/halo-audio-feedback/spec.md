# Halo Audio Feedback Specification

## ADDED Requirements

### Requirement: Audio Feedback for State Transitions
Audio feedback SHALL provide optional sound effects for all Halo state transitions.

#### Scenario: Sound plays on state transition

**Given** sound feedback is enabled in settings
**When** Halo transitions to .listening state
**Then** AudioManager plays "listening.aiff"
**And** sound plays at 30% of system volume
**And** playback completes in < 100ms

---

### Requirement: Toggleable Audio Playback Control
Audio playback SHALL be toggleable via menu bar and Settings, defaulting to OFF.

#### Scenario: User enables sounds via menu bar

**Given** sound feedback is disabled
**When** user clicks menu bar icon
**And** selects "Enable Sounds" menu item
**Then** UserDefaults.soundEnabled sets to true
**And** menu item changes to "Disable Sounds" with checkmark
**And** subsequent state transitions play sounds

---

#### Scenario: Settings toggle synchronizes with menu bar

**Given** user enabled sounds via menu bar
**When** user opens Settings → General
**Then** "Sound Effects" toggle shows ON state
**When** user toggles OFF in Settings
**Then** menu bar item updates to "Enable Sounds" (no checkmark)
**And** sounds stop playing immediately

---

### Requirement: Pre-Load Audio Assets on Launch
Audio assets SHALL be pre-loaded on app launch using AVAudioPlayer for zero-latency playback.

#### Scenario: AudioManager pre-loads all sound files

**Given** app launches
**When** AudioManager.shared initializes
**Then** loads listening.aiff into AVAudioPlayer
**And** loads processing.aiff into AVAudioPlayer
**And** loads success.aiff into AVAudioPlayer
**And** loads error.aiff into AVAudioPlayer
**And** calls prepareToPlay() on all players
**And** initialization completes in < 100ms

---

### Requirement: Sound Effects Audio Format Specification
Sound effects SHALL use 16-bit PCM AIFF format with peak amplitude at -6 dB.

#### Scenario: Audio asset validation

**Given** sound file "listening.aiff"
**Then** file format is AIFF
**And** sample rate is 44.1 kHz
**And** bit depth is 16-bit PCM
**And** peak amplitude is -6 dB (headroom for mixing)
**And** duration is 100ms ± 20ms
**And** no sharp transients (avoid pops)

---

### Requirement: Stop Previous Sound Before Playing New State Sound
Sound playback SHALL stop previous sounds before playing new state sound.

#### Scenario: Rapid state transitions stop overlapping sounds

**Given** Halo is in .processing state
**And** processing sound is playing (looping)
**When** Halo transitions to .success state
**Then** AudioManager stops processing sound immediately
**And** plays success sound
**And** no overlapping audio artifacts

---

## Cross-References

- **Related Specs**: `event-handler` (state transition triggers)
- **Depends On**: AVFoundation framework (built-in macOS)
- **Blocks**: None
