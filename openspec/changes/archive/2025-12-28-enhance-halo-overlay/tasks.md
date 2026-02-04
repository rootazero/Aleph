# Tasks: Enhance Halo Overlay

This document breaks down the `enhance-halo-overlay` change into concrete, verifiable work items.

## Task Organization

Tasks are organized by capability, with clear dependencies and validation criteria. Each task should deliver user-visible progress and include tests where applicable.

---

## Section 1: Theme System Foundation (halo-theming)

### Task 1.1: Define Theme Protocol and Enum
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Create `Theme.swift` with `Theme` enum (cyberpunk, zen, jarvis)
- Define `HaloTheme` protocol with required properties and methods
- Add `ThemeEngine` ObservableObject for theme management

**Validation**:
- [x] Theme enum has 3 cases
- [x] Protocol compiles without errors
- [x] ThemeEngine initializes with default theme (zen)

**Files**:
- `Aleph/Sources/Themes/Theme.swift` (new)
- `Aleph/Sources/Themes/ThemeEngine.swift` (new)

---

### Task 1.2: Implement Zen Theme
**Depends on**: Task 1.1
**Parallelizable**: After 1.1

**Work**:
- Create `ZenTheme.swift` implementing `HaloTheme`
- Design soft circular animations with breathing effect
- Use pastel color palette (white, light gray, sage green)
- Implement all required view methods

**Validation**:
- [x] ZenTheme conforms to HaloTheme protocol
- [x] Preview shows breathing circle animation
- [x] Colors match design spec (soft pastels)
- [x] Animation runs smoothly at 60fps

**Files**:
- `Aleph/Sources/Themes/ZenTheme.swift` (new)

---

### Task 1.3: Implement Cyberpunk Theme
**Depends on**: Task 1.1
**Parallelizable**: Yes (parallel with 1.2, 1.4)

**Work**:
- Create `CyberpunkTheme.swift` implementing `HaloTheme`
- Design hexagonal Halo shape with neon colors
- Add scanline overlay effect
- Implement glitch transition effects

**Validation**:
- [x] CyberpunkTheme conforms to HaloTheme protocol
- [x] Preview shows hexagonal shape with neon colors
- [x] Glitch effect renders correctly
- [x] Scanline overlay visible but subtle

**Files**:
- `Aleph/Sources/Themes/CyberpunkTheme.swift` (new)
- `Aleph/Sources/Themes/Effects/GlitchOverlay.swift` (new)

---

### Task 1.4: Implement Jarvis Theme
**Depends on**: Task 1.1
**Parallelizable**: Yes (parallel with 1.2, 1.3)

**Work**:
- Create `JarvisTheme.swift` implementing `HaloTheme`
- Design hexagonal segments that assemble/disassemble
- Add arc reactor blue color (#00d4ff)
- Implement pulsing energy core center

**Validation**:
- [x] JarvisTheme conforms to HaloTheme protocol
- [x] Preview shows hexagonal segments
- [x] Arc reactor blue color accurate
- [x] Core pulsing animation smooth

**Files**:
- `Aleph/Sources/Themes/JarvisTheme.swift` (new)
- `Aleph/Sources/Themes/Shapes/HexSegment.swift` (new)

---

### Task 1.5: Integrate ThemeEngine into HaloWindow
**Depends on**: Tasks 1.2, 1.3, 1.4
**Parallelizable**: No

**Work**:
- Inject `ThemeEngine` into `HaloWindow` initializer
- Update `HaloView` to use current theme's view methods
- Implement theme switching with crossfade transition (0.5s)
- Persist theme selection to UserDefaults

**Validation**:
- [x] HaloWindow renders using selected theme
- [x] Theme switching triggers crossfade animation
- [x] Theme persists across app restarts
- [x] No crashes when switching themes during animation

**Files**:
- `Aleph/Sources/HaloWindow.swift` (modify)
- `Aleph/Sources/HaloView.swift` (modify)

---

### Task 1.6: Add Theme Selector to Settings (Stub)
**Depends on**: Task 1.5
**Parallelizable**: Yes

**Work**:
- Add "Theme" section to General settings tab
- Create theme preview thumbnails (static images for now)
- Implement theme selection picker
- Wire up to ThemeEngine

**Validation**:
- [x] Settings shows 3 theme options
- [x] Selecting a theme updates Halo immediately
- [x] Preview thumbnails visible (placeholder OK)
- [x] Selection persists after app restart

**Files**:
- `Aleph/Sources/SettingsView.swift` (modify)
- `Aleph/Assets.xcassets/ThemePreviews/` (new)

---

### Task 1.7: Write Tests for Theme System
**Depends on**: Task 1.5
**Parallelizable**: Yes

**Work**:
- Unit test: Theme enum rawValue persistence
- Unit test: ThemeEngine loads saved theme
- Unit test: Each theme conforms to protocol
- Snapshot test: Visual regression for each theme

**Validation**:
- [x] All tests pass (`cmd+U` in Xcode)
- [x] Code coverage > 80% for theme logic
- [x] Snapshot tests capture all 3 themes in all states

**Files**:
- `AlephTests/Themes/ThemeEngineTests.swift` (new)
- `AlephTests/Themes/ThemeSnapshotTests.swift` (new)

---

## Section 2: Streaming Response Display (halo-streaming-text)

### Task 2.1: Extend UniFFI Interface for Streaming
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Add `on_response_chunk(text: String)` callback to `aleph.udl`
- Regenerate Swift bindings
- Update `AlephEventHandler` trait in Rust

**Validation**:
- [x] UniFFI bindings generate without errors
- [x] Swift sees new callback method
- [x] Rust trait updated with new method

**Files**:
- `Aleph/core/src/aleph.udl` (modify)
- `Aleph/core/src/event_handler.rs` (modify)
- `Aleph/Sources/Generated/aleph.swift` (regenerated)

---

### Task 2.2: Extend HaloState for Streaming Text
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Add `streamingText: String?` associated value to `.processing` case
- Add `finalText: String?` to `.success` case
- Update Equatable conformance

**Validation**:
- [x] HaloState compiles with new associated values
- [x] Equatable comparison works correctly
- [x] No breaking changes to existing state transitions

**Files**:
- `Aleph/Sources/HaloState.swift` (modify)

---

### Task 2.3: Implement StreamingTextView Component
**Depends on**: Task 2.2
**Parallelizable**: Yes

**Work**:
- Create `StreamingTextView.swift` SwiftUI component
- Implement typewriter animation (reveal characters over time)
- Add line wrapping (max 3 lines)
- Handle text overflow with horizontal scrolling marquee

**Validation**:
- [x] Text animates character-by-character
- [x] Max 3 lines enforced
- [x] Overflow triggers marquee scroll
- [x] Monospace font for code, sans-serif for prose

**Files**:
- `Aleph/Sources/Components/StreamingTextView.swift` (new)

---

### Task 2.4: Integrate StreamingTextView into HaloView
**Depends on**: Tasks 2.2, 2.3
**Parallelizable**: No

**Work**:
- Update `HaloView` to render `StreamingTextView` when `.processing` has text
- Implement dynamic frame sizing (expand height for text)
- Add smooth spring animation for size changes
- Auto-collapse after 2s of no new text

**Validation**:
- [x] Halo expands vertically when text appears
- [x] Spring animation smooth (no jank)
- [x] Auto-collapse after timeout works
- [x] No layout issues on multi-monitor setups

**Files**:
- `Aleph/Sources/HaloView.swift` (modify)

---

### Task 2.5: Implement on_response_chunk Callback in EventHandler
**Depends on**: Tasks 2.1, 2.4
**Parallelizable**: No

**Work**:
- Implement `onResponseChunk(text: String)` in `EventHandler.swift`
- Accumulate chunks and update `HaloState.processing` with full text
- Dispatch to main queue for UI updates
- Handle rapid chunk arrival (debounce if needed)

**Validation**:
- [x] Callback executes on background thread, dispatches to main
- [x] Text accumulates correctly across multiple chunks
- [x] UI updates smoothly (no dropped frames)
- [x] Edge case: empty chunks handled gracefully

**Files**:
- `Aleph/Sources/EventHandler.swift` (modify)

---

### Task 2.6: Mock Streaming Response in Rust (for Testing)
**Depends on**: Task 2.1
**Parallelizable**: Yes

**Work**:
- Add test method in Rust: `test_streaming_response()`
- Simulate streaming by sending chunks with delays
- Call `on_response_chunk()` multiple times
- Verify Swift receives chunks in order

**Validation**:
- [x] Test harness sends chunks with 100ms delays
- [x] Swift receives all chunks in correct order
- [x] Final text matches expected output
- [x] No memory leaks or crashes

**Files**:
- `Aleph/core/src/core.rs` (modify - add test method)
- `Aleph/core/src/aleph.udl` (modify - add test method)

---

### Task 2.7: Write Tests for Streaming Display
**Depends on**: Task 2.5
**Parallelizable**: Yes

**Work**:
- Unit test: Text accumulation logic
- Unit test: Typewriter animation timing
- Integration test: Mock streaming from Rust → Swift → UI
- Performance test: Measure latency from callback to screen

**Validation**:
- [x] All tests pass
- [x] Latency < 50ms per chunk (measured)
- [x] No flicker or visual glitches in tests

**Files**:
- `AlephTests/Streaming/StreamingTextTests.swift` (new)

---

## Section 3: Enhanced Error Handling (halo-error-feedback)

### Task 3.1: Define ErrorType Enum in UniFFI
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Add `ErrorType` enum to `aleph.udl` (Network, Permission, Quota, Timeout, Unknown)
- Add `on_error_typed(ErrorType, String)` callback
- Regenerate Swift bindings

**Validation**:
- [ ] UniFFI generates ErrorType Swift enum
- [ ] Callback visible in Swift EventHandler
- [ ] Rust can instantiate all error types

**Files**:
- `Aleph/core/src/aleph.udl` (modify)
- `Aleph/Sources/Generated/aleph.swift` (regenerated)

---

### Task 3.2: Extend HaloState for Typed Errors
**Depends on**: Task 3.1
**Parallelizable**: Yes

**Work**:
- Update `.error` case to store `ErrorType` and `String`
- Update all error state transitions to use new signature

**Validation**:
- [ ] HaloState.error compiles with new parameters
- [ ] Equatable conformance updated
- [ ] No breaking changes to existing error handling

**Files**:
- `Aleph/Sources/HaloState.swift` (modify)

---

### Task 3.3: Implement ErrorActionView Component
**Depends on**: Task 3.2
**Parallelizable**: Yes

**Work**:
- Create `ErrorActionView.swift` SwiftUI component
- Display error icon based on `ErrorType`
- Show error message (truncate if > 100 chars)
- Add action buttons (Retry, Open Settings, Dismiss)

**Validation**:
- [ ] Correct icon displayed for each error type
- [ ] Action buttons render correctly
- [ ] Button tap handlers wired up
- [ ] Error message wraps properly

**Files**:
- `Aleph/Sources/Components/ErrorActionView.swift` (new)
- `Aleph/Sources/Styles/HaloButtonStyle.swift` (new)

---

### Task 3.4: Integrate ErrorActionView into HaloView
**Depends on**: Task 3.3
**Parallelizable**: No

**Work**:
- Update `HaloView` to render `ErrorActionView` when state is `.error`
- Implement retry action (call Rust retry method)
- Implement "Open Settings" action (launch System Settings)
- Add shake animation on error state entry

**Validation**:
- [ ] Error UI displays correctly for all error types
- [ ] Retry button triggers Rust retry logic
- [ ] Open Settings opens correct preference pane
- [ ] Shake animation plays on error state

**Files**:
- `Aleph/Sources/HaloView.swift` (modify)
- `Aleph/Sources/EventHandler.swift` (modify - add retry method)

---

### Task 3.5: Implement Retry Logic in Rust Core
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Add `retry_last_request()` method to `AlephCore`
- Store last request context (clipboard content, provider)
- Implement retry with exponential backoff (2s, 4s, 8s)
- Max 2 auto-retries, then manual retry only

**Validation**:
- [ ] Retry method callable from Swift
- [ ] Exponential backoff implemented correctly
- [ ] Max retry limit enforced
- [ ] State transitions correctly on retry success/failure

**Files**:
- `Aleph/core/src/core.rs` (modify)
- `Aleph/core/src/aleph.udl` (modify - add method)

---

### Task 3.6: Test Error Handling Flow
**Depends on**: Tasks 3.4, 3.5
**Parallelizable**: Yes

**Work**:
- Manual test: Simulate network error (disconnect WiFi)
- Manual test: Simulate timeout (mock slow API)
- Manual test: Simulate permission error (revoke Accessibility)
- Manual test: Verify retry logic works end-to-end

**Validation**:
- [ ] Network error shows correct UI + Retry button
- [ ] Timeout error shows Retry button
- [ ] Permission error shows Open Settings button
- [ ] Retry button successfully retries request

**Files**:
- Update `TESTING_GUIDE.md` with error scenarios

---

## Section 4: Audio Feedback System (halo-audio-feedback)

### Task 4.1: Create Audio Assets
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Design/source 4 sound effects (listening, processing, success, error)
- Format as 16-bit PCM AIFF files
- Keep duration < 200ms (except processing loop)
- Add to `Aleph/Resources/Sounds/`

**Validation**:
- [ ] All 4 sound files present
- [ ] File format: AIFF, 16-bit PCM, 44.1kHz
- [ ] Peak amplitude: -6 dB (headroom)
- [ ] Files load in macOS QuickTime Player

**Files**:
- `Aleph/Resources/Sounds/listening.aiff` (new)
- `Aleph/Resources/Sounds/processing.aiff` (new)
- `Aleph/Resources/Sounds/success.aiff` (new)
- `Aleph/Resources/Sounds/error.aiff` (new)

---

### Task 4.2: Implement AudioManager
**Depends on**: Task 4.1
**Parallelizable**: Yes

**Work**:
- Create `AudioManager.swift` singleton
- Pre-load all sounds on app launch using AVAudioPlayer
- Implement `play(for state: HaloState)` method
- Set volume to 30% of system volume
- Respect `soundEnabled` UserDefaults setting

**Validation**:
- [ ] AudioManager initializes without errors
- [ ] All sounds pre-loaded successfully
- [ ] play() method plays correct sound
- [ ] Volume set to 30% (not system volume)

**Files**:
- `Aleph/Sources/Audio/AudioManager.swift` (new)

---

### Task 4.3: Integrate Audio Playback into State Transitions
**Depends on**: Task 4.2
**Parallelizable**: No

**Work**:
- Update `EventHandler` to call `AudioManager.shared.play()` on state changes
- Play sound AFTER state transition (not before)
- Stop previous sound if new state entered rapidly

**Validation**:
- [ ] Sounds play on all state transitions (if enabled)
- [ ] No overlapping sounds (previous stopped)
- [ ] Sound playback doesn't block UI thread
- [ ] Disabling sound in settings works immediately

**Files**:
- `Aleph/Sources/EventHandler.swift` (modify)

---

### Task 4.4: Add Sound Toggle to Menu Bar
**Depends on**: Task 4.2
**Parallelizable**: Yes

**Work**:
- Add "Mute Sounds" / "Unmute Sounds" menu item to menu bar
- Toggle `soundEnabled` UserDefaults on click
- Update menu item title dynamically
- Add checkmark icon when sounds enabled

**Validation**:
- [ ] Menu item appears in menu bar dropdown
- [ ] Toggling works (sounds stop/resume)
- [ ] Menu item title updates correctly
- [ ] Setting persists across app restarts

**Files**:
- `Aleph/Sources/AppDelegate.swift` (modify)

---

### Task 4.5: Test Audio Feedback
**Depends on**: Tasks 4.3, 4.4
**Parallelizable**: Yes

**Work**:
- Manual test: Trigger all state transitions, verify sounds play
- Manual test: Toggle sounds on/off via menu bar
- Manual test: Test at different system volumes (0%, 50%, 100%)
- Manual test: Verify no audio artifacts or pops

**Validation**:
- [ ] All sounds play correctly at 30% volume
- [ ] Toggle works reliably
- [ ] No audio glitches or distortion
- [ ] Sounds are subtle and non-intrusive

**Files**:
- Update `TESTING_GUIDE.md` with audio test section

---

## Section 5: Performance Optimization (halo-performance)

### Task 5.1: Implement PerformanceMonitor
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Create `PerformanceMonitor.swift` using CADisplayLink
- Track frame timestamps over last 60 frames
- Calculate average FPS
- Post notification if FPS drops below 55

**Validation**:
- [ ] Monitor tracks FPS accurately
- [ ] Notification posted on performance drop
- [ ] Minimal CPU overhead (< 0.5%)
- [ ] Works across all macOS versions (13+)

**Files**:
- `Aleph/Sources/Performance/PerformanceMonitor.swift` (new)

---

### Task 5.2: Detect GPU Capabilities
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Create `PerformanceManager.swift` to detect GPU
- Query Metal device name on app launch
- Set `effectsQuality` level (high/medium/low)
- Store in UserDefaults (allow manual override)

**Validation**:
- [ ] Correctly detects M1/M2 (high quality)
- [ ] Correctly detects Intel HD 3000/4000 (low quality)
- [ ] Defaults to medium if detection fails
- [ ] Manual override works (for testing)

**Files**:
- `Aleph/Sources/Performance/PerformanceManager.swift` (new)

---

### Task 5.3: Optimize Theme Rendering for Low-End GPUs
**Depends on**: Tasks 5.1, 5.2
**Parallelizable**: No

**Work**:
- Add quality checks to theme view methods
- Disable Metal shaders on low quality setting
- Use solid colors instead of gradients on low quality
- Simplify animations (linear instead of spring)

**Validation**:
- [ ] High quality: full effects render
- [ ] Medium quality: no shaders, basic gradients
- [ ] Low quality: solid colors, linear animations
- [ ] Quality degradation triggers automatically

**Files**:
- `Aleph/Sources/Themes/CyberpunkTheme.swift` (modify)
- `Aleph/Sources/Themes/ZenTheme.swift` (modify)
- `Aleph/Sources/Themes/JarvisTheme.swift` (modify)

---

### Task 5.4: Profile and Optimize HaloView Rendering
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Profile HaloView with Instruments (Time Profiler)
- Identify hotspots (> 5ms per frame)
- Optimize: reduce view hierarchy depth, cache shapes
- Ensure < 16ms per frame (60fps target)

**Validation**:
- [ ] Instruments shows < 16ms render time
- [ ] No dropped frames during transitions
- [ ] Memory usage stable (< 10MB for Halo)
- [ ] CPU usage < 5% during animation

**Files**:
- `Aleph/Sources/HaloView.swift` (modify - optimizations)

---

### Task 5.5: Test Performance on Target Hardware
**Depends on**: Tasks 5.3, 5.4
**Parallelizable**: Yes

**Work**:
- Manual test: 2018 MacBook Pro (Intel Iris Plus)
- Manual test: 2020 M1 Mac
- Manual test: 2015 MacBook Air (Intel HD 6000)
- Verify 60fps on M1, 30fps minimum on older hardware

**Validation**:
- [ ] M1: Consistent 60fps with all effects
- [ ] 2018 Intel: 60fps with medium quality
- [ ] 2015 Intel: 30fps minimum with low quality
- [ ] No thermal throttling after 30min runtime

**Files**:
- Update `TESTING_GUIDE.md` with performance benchmarks

---

## Section 6: Accessibility Support (halo-accessibility)

### Task 6.1: Add VoiceOver Announcements
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Extend `HaloWindow` with `announceToVoiceOver()` method
- Call on every state change with descriptive message
- Use low-priority announcements (non-intrusive)
- Map states to user-friendly text

**Validation**:
- [ ] VoiceOver announces "Aleph listening" on hotkey
- [ ] VoiceOver announces "Processing with OpenAI"
- [ ] VoiceOver announces "Complete" on success
- [ ] VoiceOver announces "Error: [message]" on error

**Files**:
- `Aleph/Sources/HaloWindow.swift` (modify)

---

### Task 6.2: Set Accessibility Labels on HaloWindow
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Set `accessibilityLabel` on HaloWindow for each state
- Set `accessibilityRole` to `.window`
- Make Halo window accessible (not ignored by VoiceOver)

**Validation**:
- [ ] Accessibility Inspector shows correct labels
- [ ] VoiceOver can focus on Halo window
- [ ] Label updates dynamically with state changes

**Files**:
- `Aleph/Sources/HaloWindow.swift` (modify)

---

### Task 6.3: Test with macOS Accessibility Tools
**Depends on**: Tasks 6.1, 6.2
**Parallelizable**: Yes

**Work**:
- Manual test: Enable VoiceOver (Cmd+F5)
- Manual test: Trigger all state transitions
- Manual test: Verify announcements are clear and helpful
- Manual test: Use Accessibility Inspector to verify labels

**Validation**:
- [ ] All announcements audible via VoiceOver
- [ ] Announcements don't interrupt user workflow
- [ ] Accessibility Inspector shows no warnings
- [ ] Labels accurate and descriptive

**Files**:
- Update `TESTING_GUIDE.md` with accessibility tests

---

## Section 7: User Customization (halo-customization)

### Task 7.1: Define HaloPreferences Struct
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Create `HaloPreferences.swift` with Codable struct
- Define properties: size (Small/Medium/Large), opacity (0-1), animationSpeed (0.5-2.0)
- Add default values (Medium, 1.0, 1.0)

**Validation**:
- [ ] Struct compiles and conforms to Codable
- [ ] Default values match spec
- [ ] Encoding/decoding works correctly

**Files**:
- `Aleph/Sources/Models/HaloPreferences.swift` (new)

---

### Task 7.2: Implement PreferencesManager
**Depends on**: Task 7.1
**Parallelizable**: Yes

**Work**:
- Create `PreferencesManager.swift` ObservableObject
- Load preferences from UserDefaults on init
- Save preferences on change (didSet)
- Provide @Published property for SwiftUI binding

**Validation**:
- [ ] PreferencesManager loads saved prefs
- [ ] Changes persist across app restarts
- [ ] SwiftUI views react to preference changes
- [ ] No crashes on corrupted UserDefaults data

**Files**:
- `Aleph/Sources/Managers/PreferencesManager.swift` (new)

---

### Task 7.3: Apply Preferences to HaloView
**Depends on**: Task 7.2
**Parallelizable**: No

**Work**:
- Inject `PreferencesManager` into HaloView as @EnvironmentObject
- Apply size preference to `.frame()` modifier
- Apply opacity preference to `.opacity()` modifier
- Apply animation speed to `.animation()` duration

**Validation**:
- [ ] Changing size updates Halo immediately
- [ ] Changing opacity updates Halo immediately
- [ ] Changing animation speed updates transitions
- [ ] Preferences applied correctly on app launch

**Files**:
- `Aleph/Sources/HaloView.swift` (modify)
- `Aleph/Sources/HaloWindow.swift` (modify - inject manager)

---

### Task 7.4: Add Customization UI to Settings
**Depends on**: Task 7.2
**Parallelizable**: Yes

**Work**:
- Add "Halo Appearance" section to General settings
- Add size slider (Small/Medium/Large)
- Add opacity slider (50%-100%)
- Add animation speed slider (Slow/Normal/Fast)

**Validation**:
- [ ] Sliders render correctly in Settings
- [ ] Changes update HaloView in real-time (if visible)
- [ ] Settings persist after closing Settings window
- [ ] Reset button restores defaults

**Files**:
- `Aleph/Sources/SettingsView.swift` (modify)

---

### Task 7.5: Test Customization Settings
**Depends on**: Tasks 7.3, 7.4
**Parallelizable**: Yes

**Work**:
- Manual test: Change size, verify Halo resizes
- Manual test: Change opacity, verify Halo becomes transparent
- Manual test: Change animation speed, verify transitions faster/slower
- Manual test: Reset to defaults, verify values reset

**Validation**:
- [ ] All settings apply immediately
- [ ] Settings persist after app restart
- [ ] No visual glitches when changing settings
- [ ] Reset button works correctly

**Files**:
- Update `TESTING_GUIDE.md` with customization tests

---

## Section 8: Multi-Halo Queue Management (halo-concurrency)

### Task 8.1: Add Queue State to EventHandler
**Depends on**: None
**Parallelizable**: Yes

**Work**:
- Add `isProcessing` flag to EventHandler
- Add `pendingOperations` queue (max depth: 3)
- Add `updateQueueBadge()` helper method

**Validation**:
- [ ] EventHandler compiles with new properties
- [ ] Queue initialized as empty array
- [ ] isProcessing defaults to false

**Files**:
- `Aleph/Sources/EventHandler.swift` (modify)

---

### Task 8.2: Implement Queue Logic in Hotkey Handler
**Depends on**: Task 8.1
**Parallelizable**: No

**Work**:
- Guard `onHotkeyDetected()` with `if isProcessing { ... }`
- If processing, add to queue (if not full)
- If queue full, show "Queue full" error Halo
- Process next item on state change to success/error

**Validation**:
- [ ] Rapid hotkey presses enqueue operations
- [ ] Queue depth limited to 3
- [ ] Queue full error shows when depth exceeded
- [ ] Operations process sequentially (FIFO)

**Files**:
- `Aleph/Sources/EventHandler.swift` (modify)

---

### Task 8.3: Add Queue Badge to HaloView
**Depends on**: Task 8.1
**Parallelizable**: Yes

**Work**:
- Create `QueueBadge.swift` SwiftUI component
- Display badge with queue count (e.g., "2 pending")
- Position in top-right corner of Halo
- Fade in/out when queue changes

**Validation**:
- [ ] Badge renders correctly on Halo
- [ ] Count updates dynamically
- [ ] Badge hidden when queue empty
- [ ] Fade animation smooth

**Files**:
- `Aleph/Sources/Components/QueueBadge.swift` (new)
- `Aleph/Sources/HaloView.swift` (modify - add badge)

---

### Task 8.4: Test Multi-Halo Queue
**Depends on**: Tasks 8.2, 8.3
**Parallelizable**: Yes

**Work**:
- Manual test: Press Cmd+~ 5 times rapidly
- Verify first 3 queue, 4th+ show error
- Verify operations process sequentially
- Verify badge updates correctly

**Validation**:
- [ ] Queue holds max 3 operations
- [ ] 4th operation shows "Queue full" error
- [ ] Operations process in FIFO order
- [ ] Badge count accurate throughout

**Files**:
- Update `TESTING_GUIDE.md` with queue tests

---

## Section 9: Specification Deltas

### Task 9.1: Write Spec Delta for halo-theming
**Depends on**: Section 1 complete
**Parallelizable**: Yes

**Work**:
- Create `openspec/changes/enhance-halo-overlay/specs/halo-theming/spec.md`
- Document requirements for theme system
- Write scenarios for each requirement (Given/When/Then)
- Cross-reference with design.md

**Validation**:
- [ ] Spec file follows OpenSpec format
- [ ] At least 1 scenario per requirement
- [ ] All requirements tagged as ADDED
- [ ] `openspec validate enhance-halo-overlay --strict` passes

**Files**:
- `openspec/changes/enhance-halo-overlay/specs/halo-theming/spec.md` (new)

---

### Task 9.2: Write Spec Delta for halo-streaming-text
**Depends on**: Section 2 complete
**Parallelizable**: Yes

**Work**:
- Create spec.md for streaming text capability
- Document streaming display requirements
- Write scenarios for typewriter animation, size changes, overflow

**Validation**:
- [ ] Spec file complete with scenarios
- [ ] `openspec validate` passes
- [ ] Cross-references streaming design doc

**Files**:
- `openspec/changes/enhance-halo-overlay/specs/halo-streaming-text/spec.md` (new)

---

### Task 9.3: Write Spec Delta for halo-error-feedback
**Depends on**: Section 3 complete
**Parallelizable**: Yes

**Work**:
- Create spec.md for error feedback capability
- Document error types and action buttons
- Write scenarios for retry flow

**Validation**:
- [ ] Spec file complete
- [ ] `openspec validate` passes

**Files**:
- `openspec/changes/enhance-halo-overlay/specs/halo-error-feedback/spec.md` (new)

---

### Task 9.4: Write Spec Delta for halo-audio-feedback
**Depends on**: Section 4 complete
**Parallelizable**: Yes

**Work**:
- Create spec.md for audio feedback
- Document sound playback requirements
- Write scenarios for toggle behavior

**Validation**:
- [ ] Spec file complete
- [ ] `openspec validate` passes

**Files**:
- `openspec/changes/enhance-halo-overlay/specs/halo-audio-feedback/spec.md` (new)

---

### Task 9.5: Write Spec Delta for halo-performance
**Depends on**: Section 5 complete
**Parallelizable**: Yes

**Work**:
- Create spec.md for performance optimizations
- Document FPS requirements
- Write scenarios for quality degradation

**Validation**:
- [ ] Spec file complete
- [ ] `openspec validate` passes

**Files**:
- `openspec/changes/enhance-halo-overlay/specs/halo-performance/spec.md` (new)

---

### Task 9.6: Write Spec Delta for halo-accessibility
**Depends on**: Section 6 complete
**Parallelizable**: Yes

**Work**:
- Create spec.md for accessibility support
- Document VoiceOver requirements
- Write scenarios for announcements

**Validation**:
- [ ] Spec file complete
- [ ] `openspec validate` passes

**Files**:
- `openspec/changes/enhance-halo-overlay/specs/halo-accessibility/spec.md` (new)

---

### Task 9.7: Write Spec Delta for halo-customization
**Depends on**: Section 7 complete
**Parallelizable**: Yes

**Work**:
- Create spec.md for user customization
- Document preference options
- Write scenarios for settings persistence

**Validation**:
- [ ] Spec file complete
- [ ] `openspec validate` passes

**Files**:
- `openspec/changes/enhance-halo-overlay/specs/halo-customization/spec.md` (new)

---

### Task 9.8: Write Spec Delta for halo-concurrency
**Depends on**: Section 8 complete
**Parallelizable**: Yes

**Work**:
- Create spec.md for multi-Halo queue
- Document queue management requirements
- Write scenarios for rapid hotkey presses

**Validation**:
- [ ] Spec file complete
- [ ] `openspec validate` passes

**Files**:
- `openspec/changes/enhance-halo-overlay/specs/halo-concurrency/spec.md` (new)

---

## Section 10: Documentation and Polish

### Task 10.1: Update CLAUDE.md with Phase 3 Status
**Depends on**: All sections complete
**Parallelizable**: Yes

**Work**:
- Mark Phase 3 tasks as complete in "Development Phases" section
- Add theme system to architecture documentation
- Update success criteria with Phase 3 deliverables

**Validation**:
- [ ] CLAUDE.md reflects Phase 3 completion
- [ ] All task checkboxes marked
- [ ] No broken references

**Files**:
- `CLAUDE.md` (modify)

---

### Task 10.2: Update README with Theme Documentation
**Depends on**: All sections complete
**Parallelizable**: Yes

**Work**:
- Add "Themes" section to README
- Document all 3 themes with screenshots (optional)
- Add customization options to user guide

**Validation**:
- [ ] README documents theme system
- [ ] Customization options explained
- [ ] Screenshots/GIFs added (optional)

**Files**:
- `Aleph/README.md` (modify)

---

### Task 10.3: Update TESTING_GUIDE.md
**Depends on**: All sections complete
**Parallelizable**: Yes

**Work**:
- Add test sections for all Phase 3 features
- Document manual test procedures
- Add performance benchmark table

**Validation**:
- [ ] All Phase 3 features have test sections
- [ ] Manual test steps clear and actionable
- [ ] Benchmark table complete

**Files**:
- `TESTING_GUIDE.md` (modify)

---

### Task 10.4: Run Comprehensive Integration Tests
**Depends on**: All sections complete
**Parallelizable**: No

**Work**:
- Execute all manual tests from TESTING_GUIDE.md
- Test on macOS 13, 14, 15
- Test on M1 and Intel hardware
- Document any issues found

**Validation**:
- [ ] All tests pass on target hardware
- [ ] No regressions from Phase 2
- [ ] Performance meets targets (60fps)
- [ ] Issues documented in GitHub

**Files**:
- Create issue tickets for any bugs found

---

### Task 10.5: Validate OpenSpec Change
**Depends on**: Tasks 9.1-9.8
**Parallelizable**: No

**Work**:
- Run `openspec validate enhance-halo-overlay --strict`
- Fix any validation errors
- Ensure all spec deltas have scenarios
- Check cross-references

**Validation**:
- [ ] `openspec validate` passes with zero errors
- [ ] All requirements have at least 1 scenario
- [ ] All ADDED/MODIFIED tags correct

**Files**:
- Various spec.md files (fix if needed)

---

## Summary

**Total Tasks**: 59
**Parallelizable**: 42
**Sequential**: 17
**Estimated Spec Files**: 8 (one per capability)

### Dependency Graph (High-Level)

```
Section 1 (Themes) → Section 9.1 (Spec)
Section 2 (Streaming) → Section 9.2 (Spec)
Section 3 (Errors) → Section 9.3 (Spec)
Section 4 (Audio) → Section 9.4 (Spec)
Section 5 (Performance) → Section 9.5 (Spec)
Section 6 (Accessibility) → Section 9.6 (Spec)
Section 7 (Customization) → Section 9.7 (Spec)
Section 8 (Queue) → Section 9.8 (Spec)
All Sections → Section 10 (Documentation)
```

### Critical Path

The critical path for Phase 3 delivery is:

1. Task 1.1 (Theme Protocol) → 1.2-1.4 (Theme Implementations) → 1.5 (Integration) → 9.1 (Spec)
2. Task 2.1 (UniFFI) → 2.2 (HaloState) → 2.3 (StreamingTextView) → 2.4 (Integration) → 2.5 (Callback) → 9.2 (Spec)
3. All sections → 10.4 (Integration Tests) → 10.5 (Validation)

### Recommended Parallelization Strategy

**Week 1**: Sections 1, 2, 3 (Foundation)
- Parallel: 1.2, 1.3, 1.4 (Theme implementations)
- Parallel: 2.1, 2.2, 2.3 (Streaming components)
- Parallel: 3.1, 3.2, 3.3 (Error components)

**Week 2**: Sections 4, 5, 6 (Enhancement)
- Parallel: 4.1, 4.2 (Audio)
- Parallel: 5.1, 5.2 (Performance)
- Parallel: 6.1, 6.2 (Accessibility)

**Week 3**: Sections 7, 8, 9 (Polish & Specs)
- Parallel: 7.1-7.4 (Customization)
- Parallel: 8.1-8.3 (Queue)
- Parallel: 9.1-9.8 (Specs)

**Week 4**: Section 10 (Testing & Documentation)
- Sequential: 10.1-10.5

---

## Notes

- All file paths are relative to project root (`/Users/zouguojun/Workspace/Aleph/`)
- "(new)" indicates file creation; "(modify)" indicates editing existing file
- Manual tests should be documented in TESTING_GUIDE.md with checkbox format
- Each task should result in a working, testable increment
- Prefer small commits per task over large batch commits
