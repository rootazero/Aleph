# Audio Feedback Testing Guide

## Overview
This guide covers testing for the audio feedback system (Section 4: halo-audio-feedback).

---

## Pre-Test Setup

### 1. Verify Audio Assets
Check that all sound files are present:
```bash
ls -lh Aleph/Resources/Sounds/
# Expected output:
# listening.aiff
# processing.aiff
# success.aiff
# error.aiff
# README.md
```

### 2. System Audio Configuration
- System volume: Set to 50%
- Output device: Built-in speakers or headphones
- Do not disturb: OFF (so sounds can play)

### 3. Build and Run
```bash
cd /Users/zouguojun/Workspace/Aether
xcodegen generate
open Aleph.xcodeproj
# Click Run (Cmd+R) in Xcode
```

---

## Test Cases

### Test 4.1: AudioManager Initialization
**Objective**: Verify AudioManager loads all sounds on app launch

**Steps**:
1. Launch Aleph app
2. Check console logs for sound loading messages

**Expected Results**:
```
[AudioManager] Loaded sound: listening.aiff
[AudioManager] Loaded sound: processing.aiff
[AudioManager] Loaded sound: success.aiff
[AudioManager] Loaded sound: error.aiff
```

**Pass Criteria**: All 4 sounds load without errors

---

### Test 4.2: Listening State Sound
**Objective**: Verify sound plays when hotkey is detected

**Steps**:
1. Grant Accessibility permission (if not already)
2. Press Cmd+~ (or configured hotkey)
3. Listen for "Pop" sound (listening.aiff)
4. Observe Halo appears at cursor

**Expected Results**:
- Sound plays immediately on hotkey press
- Volume is subtle (30% of system volume)
- No delay between hotkey and sound
- Halo appears simultaneously

**Pass Criteria**: Sound plays reliably every time

---

### Test 4.3: Processing State Sound
**Objective**: Verify sound plays during AI processing

**Steps**:
1. Trigger test streaming response:
   ```swift
   // In debug menu or console:
   core?.testStreamingResponse()
   ```
2. Listen for "Tink" sound (processing.aiff)
3. Observe Halo in processing state

**Expected Results**:
- Sound plays when state changes to .processing
- Sound is non-intrusive
- Previous listening sound stops (no overlap)

**Pass Criteria**: Clean transition between sounds

---

### Test 4.4: Success State Sound
**Objective**: Verify sound plays on successful completion

**Steps**:
1. Complete a full request cycle (listening → processing → success)
2. Listen for "Glass" sound (success.aiff)
3. Observe Halo shows success state

**Expected Results**:
- Sound plays when streaming completes
- Sound is satisfying/positive
- Processing sound stops cleanly

**Pass Criteria**: Success feedback is clear and pleasant

---

### Test 4.5: Error State Sound
**Objective**: Verify sound plays on error

**Steps**:
1. Trigger typed error:
   ```swift
   core?.testTypedError(
       errorType: .network,
       message: "Network error occurred"
   )
   ```
2. Listen for "Basso" sound (error.aiff)
3. Observe Halo shows error UI

**Expected Results**:
- Sound plays immediately on error
- Sound is negative but not alarming
- User can distinguish error sound from others

**Pass Criteria**: Error sound is clearly different from success

---

### Test 4.6: Sound Toggle - Mute
**Objective**: Verify mute toggle works

**Steps**:
1. Click menu bar icon (sparkles)
2. Select "Mute Sounds"
3. Trigger any state transition
4. Verify NO sound plays

**Expected Results**:
- Menu item changes to "Unmute Sounds"
- All sounds stop playing
- Visual feedback still works
- Setting persists (check UserDefaults)

**Pass Criteria**: No audio when muted

---

### Test 4.7: Sound Toggle - Unmute
**Objective**: Verify unmute restores sound

**Steps**:
1. With sounds muted, click menu bar icon
2. Select "Unmute Sounds"
3. Trigger state transition
4. Verify sound plays

**Expected Results**:
- Menu item changes to "Mute Sounds"
- Sounds resume immediately
- No restart required

**Pass Criteria**: Sounds work after unmuting

---

### Test 4.8: Volume Levels
**Objective**: Verify audio plays at correct volume

**Steps**:
1. Set system volume to 100%
2. Trigger listening sound
3. Estimate volume (should be ~30%)
4. Repeat at system volume 50%
5. Repeat at system volume 10%

**Expected Results**:
- Aleph sounds are always proportional to system volume
- At 100% system: Aleph plays at 30% (comfortable)
- At 50% system: Aleph plays at 15% (subtle)
- At 10% system: Aleph plays at 3% (barely audible)

**Pass Criteria**: Volume is never jarring or too loud

---

### Test 4.9: Rapid State Changes
**Objective**: Verify no audio artifacts on rapid transitions

**Steps**:
1. Rapidly trigger state changes:
   ```swift
   core?.testStreamingResponse()
   // Immediately trigger error:
   core?.testTypedError(errorType: .timeout, message: "Timeout")
   ```
2. Listen for audio glitches, pops, or overlaps

**Expected Results**:
- Previous sound stops cleanly
- New sound starts without artifact
- No crackling or popping
- No overlapping sounds

**Pass Criteria**: Clean audio transitions

---

### Test 4.10: Persistence Across App Restarts
**Objective**: Verify sound setting persists

**Steps**:
1. Mute sounds via menu bar
2. Quit Aleph (Cmd+Q)
3. Relaunch Aleph
4. Check menu bar: should show "Unmute Sounds"
5. Trigger state change: should be silent
6. Unmute and quit
7. Relaunch: should show "Mute Sounds"

**Expected Results**:
- Mute/unmute setting saved to UserDefaults
- Setting restored on app launch
- No reset to default

**Pass Criteria**: Setting persists correctly

---

## Performance Tests

### Test 4.11: Audio Latency
**Objective**: Measure time from state change to sound playback

**Steps**:
1. Use Xcode Instruments (Time Profiler)
2. Trigger listening sound
3. Measure time from `AudioManager.shared.play()` call to actual audio output

**Expected Results**:
- Latency < 10ms (pre-loaded sounds)
- No frame drops in UI
- Audio playback doesn't block main thread

**Pass Criteria**: Sub-10ms latency

---

### Test 4.12: Memory Usage
**Objective**: Verify sounds don't leak memory

**Steps**:
1. Use Xcode Memory Graph
2. Trigger 100 state transitions
3. Check for memory growth

**Expected Results**:
- AudioManager is singleton (1 instance)
- AVAudioPlayer instances reused (not recreated)
- No memory leaks

**Pass Criteria**: Stable memory usage

---

## Edge Cases

### Test 4.13: Missing Sound File
**Objective**: Verify graceful handling of missing assets

**Steps**:
1. Temporarily rename `success.aiff`
2. Relaunch app
3. Trigger success state

**Expected Results**:
- Console warning: "Sound file not found: success.aiff"
- App doesn't crash
- Other sounds still work

**Pass Criteria**: Degrades gracefully

---

### Test 4.14: Corrupted Sound File
**Objective**: Verify handling of corrupted audio

**Steps**:
1. Replace `error.aiff` with invalid data
2. Relaunch app
3. Check console logs

**Expected Results**:
- Console error: "Error loading sound error: ..."
- App doesn't crash
- Other sounds work

**Pass Criteria**: Robust error handling

---

## Automated Test Implementation

### Unit Tests (Optional)
Create `AudioManagerTests.swift`:
```swift
import XCTest
@testable import Aleph

class AudioManagerTests: XCTestCase {
    func testSingletonInstance() {
        let instance1 = AudioManager.shared
        let instance2 = AudioManager.shared
        XCTAssertTrue(instance1 === instance2)
    }

    func testSoundToggle() {
        let initialState = AudioManager.shared.soundEnabled
        AudioManager.shared.toggleSound()
        XCTAssertNotEqual(initialState, AudioManager.shared.soundEnabled)
        AudioManager.shared.toggleSound()
        XCTAssertEqual(initialState, AudioManager.shared.soundEnabled)
    }

    func testVolumeLevel() {
        // Verify volume is set to 0.3
        // (requires access to private player instances)
    }
}
```

---

## Known Issues / Limitations

1. **Placeholder Sounds**: Current sounds are macOS system sounds. Replace with custom audio for production.
2. **No Loop Support**: processing.aiff doesn't loop (plays once). Future: implement looping for long operations.
3. **No Spatial Audio**: Sounds play in stereo/mono, not positioned at cursor location.

---

## Pass/Fail Summary

| Test ID | Test Name | Status | Notes |
|---------|-----------|--------|-------|
| 4.1 | AudioManager Init | ⬜ | |
| 4.2 | Listening Sound | ⬜ | |
| 4.3 | Processing Sound | ⬜ | |
| 4.4 | Success Sound | ⬜ | |
| 4.5 | Error Sound | ⬜ | |
| 4.6 | Mute Toggle | ⬜ | |
| 4.7 | Unmute Toggle | ⬜ | |
| 4.8 | Volume Levels | ⬜ | |
| 4.9 | Rapid Changes | ⬜ | |
| 4.10 | Persistence | ⬜ | |
| 4.11 | Latency | ⬜ | |
| 4.12 | Memory Usage | ⬜ | |
| 4.13 | Missing File | ⬜ | |
| 4.14 | Corrupted File | ⬜ | |

Legend: ✅ Pass | ❌ Fail | ⬜ Not Tested
