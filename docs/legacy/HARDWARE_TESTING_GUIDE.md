# Hardware Performance Testing Guide

## Overview
This guide provides test procedures for validating Aleph performance across different Mac hardware configurations.

---

## Test Hardware Matrix

| Device | GPU | Year | Expected Quality | Target FPS |
|--------|-----|------|------------------|------------|
| MacBook Pro 14" M3 Pro | Apple M3 Pro | 2023 | High | 60 |
| MacBook Air M2 | Apple M2 | 2022 | High | 60 |
| MacBook Pro 13" M1 | Apple M1 | 2020 | High | 60 |
| MacBook Pro 16" (Intel) | AMD Radeon Pro 5500M | 2019 | High | 60 |
| MacBook Pro 13" (Intel) | Intel Iris Plus 655 | 2018 | Medium | 60 |
| MacBook Air (Intel) | Intel HD 6000 | 2015 | Low | 30+ |
| Mac mini (Intel) | Intel UHD 630 | 2018 | Medium | 60 |

---

## Pre-Test Setup

### 1. Build Configuration
```bash
cd /Users/zouguojun/Workspace/Aether
xcodegen generate
open Aleph.xcodeproj

# In Xcode:
# - Set scheme to "Release" (for accurate performance)
# - Build (Cmd+B)
# - Run (Cmd+R)
```

### 2. System Configuration
- **macOS Version**: 13.0+ (Ventura or later)
- **Display**: Native resolution, 60Hz refresh rate
- **Power**: Plugged into power (not on battery)
- **Background Apps**: Quit all non-essential apps
- **Do Not Disturb**: Disabled
- **System Volume**: 50%

### 3. Enable Performance Overlay
Add to HaloView for testing:
```swift
ZStack {
    // Existing Halo content

    #if DEBUG
    // Performance overlay (top-left)
    VStack {
        PerformanceOverlay()
        Spacer()
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    #endif
}
```

---

## Test Procedures

### Test 5.1: GPU Detection Verification

**Objective**: Verify correct GPU detection and quality assignment

**Steps**:
1. Launch Aleph
2. Check console logs for GPU info:
   ```
   [PerformanceManager] Detected GPU: [GPU Name]
   [PerformanceManager] GPU Family: [Family]
   [PerformanceManager] Auto-detected quality: [Quality]
   ```
3. Open debug menu (if available) or check PerformanceManager

**Expected Results**:
| Hardware | Detected Family | Quality |
|----------|----------------|---------|
| M1/M2/M3 | Apple Silicon | High |
| Intel Iris Xe/Plus 655+ | Intel | High |
| Intel UHD 630 | Intel | Medium |
| Intel HD 6000 | Intel | Low |
| AMD Radeon Pro 5000+ | AMD | High |

**Pass Criteria**: GPU detected correctly, quality matches table

---

### Test 5.2: Idle State Performance

**Objective**: Verify minimal CPU/GPU usage when Halo is hidden

**Steps**:
1. Launch Aleph (Halo hidden)
2. Wait 30 seconds
3. Open Activity Monitor
4. Check Aleph CPU/GPU usage

**Expected Results**:
- CPU: < 1% (idle)
- GPU: < 1% (idle)
- Memory: < 50 MB
- No thermal impact

**Pass Criteria**: Aleph idle footprint negligible

---

### Test 5.3: Listening State FPS (M1/M2/M3)

**Objective**: Verify 60 FPS during listening animation

**Steps**:
1. Press Cmd+~ to trigger hotkey
2. Observe listening animation (pulsing ring)
3. Check performance overlay: FPS counter
4. Let animate for 10 seconds
5. Record min/max/avg FPS

**Expected Results**:
- Min FPS: >= 58
- Avg FPS: 60
- Max FPS: 60
- No stuttering or jank

**Pass Criteria**: Consistent 60 FPS for full 10 seconds

---

### Test 5.4: Processing State FPS (M1/M2/M3)

**Objective**: Verify 60 FPS during processing animation

**Steps**:
1. Trigger test streaming response:
   ```swift
   core?.testStreamingResponse()
   ```
2. Observe processing animation (spinner + text)
3. Check FPS counter during entire animation
4. Record FPS throughout 5-second stream

**Expected Results**:
- Min FPS: >= 55
- Avg FPS: 60
- Frame drops: 0
- Streaming text smooth

**Pass Criteria**: FPS never drops below 55

---

### Test 5.5: Success State FPS (M1/M2/M3)

**Objective**: Verify smooth success animation

**Steps**:
1. Complete full request cycle
2. Observe success animation (checkmark fade-in)
3. Check FPS during 2-second success display

**Expected Results**:
- Avg FPS: 60
- No dropped frames
- Smooth fade-out

**Pass Criteria**: Consistent 60 FPS

---

### Test 5.6: Error State FPS (M1/M2/M3)

**Objective**: Verify error UI renders smoothly

**Steps**:
1. Trigger error:
   ```swift
   core?.testTypedError(errorType: .network, message: "Test error")
   ```
2. Observe shake animation
3. Check FPS during error display

**Expected Results**:
- Avg FPS: 60
- Shake animation smooth
- Buttons render cleanly

**Pass Criteria**: No performance degradation

---

### Test 5.7: Rapid State Changes (M1/M2/M3)

**Objective**: Stress test with rapid state transitions

**Steps**:
1. Rapidly trigger state changes (10 times in 5 seconds):
   - Hotkey press → immediate cancel
   - Repeat 10x
2. Monitor FPS throughout
3. Check for memory leaks

**Expected Results**:
- FPS remains >= 55
- No memory accumulation
- No visual artifacts
- UI remains responsive

**Pass Criteria**: Performance stable under stress

---

### Test 5.8: 30-Minute Soak Test (M1/M2/M3)

**Objective**: Verify no thermal throttling or memory leaks

**Steps**:
1. Launch Aleph
2. Script to trigger state changes every 10 seconds
3. Run for 30 minutes
4. Monitor:
   - FPS (should remain 60)
   - CPU temperature
   - Memory usage
   - Fan speed

**Expected Results**:
- FPS: 60 throughout
- CPU temp: < 80°C
- Memory: Stable (< 100 MB)
- No fan ramp-up

**Pass Criteria**: No performance degradation over time

---

### Test 5.9: Intel Mid-Range (2018 MBP)

**Objective**: Verify medium quality works at 60 FPS

**Steps**:
1. Force medium quality:
   ```swift
   PerformanceManager.shared.setQuality(.medium)
   ```
2. Run Tests 5.3-5.6 (all states)
3. Record FPS for each state

**Expected Results**:
- All states: >= 55 FPS
- Simplified gradients render
- Linear animations smooth
- No blur/shadow overhead

**Pass Criteria**: 60 FPS with medium quality

---

### Test 5.10: Intel Low-End (2015 MBA)

**Objective**: Verify low quality achieves 30+ FPS

**Steps**:
1. Force low quality:
   ```swift
   PerformanceManager.shared.setQuality(.low)
   ```
2. Run Tests 5.3-5.6
3. Accept 30 FPS minimum

**Expected Results**:
- Listening: >= 30 FPS
- Processing: >= 25 FPS
- Success: >= 30 FPS
- Error: >= 30 FPS
- Solid colors used
- No complex animations

**Pass Criteria**: Minimum 25 FPS, UI functional

---

## Performance Profiling (Instruments)

### Time Profiler Session

**Steps**:
1. Xcode: Product → Profile (Cmd+I)
2. Select "Time Profiler"
3. Click Record
4. Trigger all state transitions
5. Stop after 30 seconds
6. Analyze call tree

**Look For**:
- Functions taking > 5ms per frame
- `HaloView.body` render time
- SwiftUI layout bottlenecks
- Animation overhead

**Target**:
- Total frame time: < 16ms (60 FPS)
- HaloView.body: < 8ms
- Theme rendering: < 5ms

### Metal System Trace

**Steps**:
1. Xcode: Product → Profile (Cmd+I)
2. Select "Metal System Trace"
3. Record during animations
4. Analyze GPU load

**Target**:
- GPU utilization: < 50%
- No shader compilation stutters
- Texture memory: < 5MB

---

## Test Results Template

```markdown
## Test Results: [Device Name]

**Hardware**: [MacBook Pro 14" M3 Pro]
**GPU**: [Apple M3 Pro]
**Quality**: [High / Auto-detected]
**Date**: [2025-12-24]

### FPS Results

| State | Min FPS | Avg FPS | Max FPS | Pass |
|-------|---------|---------|---------|------|
| Listening | 60 | 60 | 60 | ✅ |
| Processing | 58 | 60 | 60 | ✅ |
| Success | 60 | 60 | 60 | ✅ |
| Error | 60 | 60 | 60 | ✅ |
| Rapid Changes | 55 | 58 | 60 | ✅ |

### Resource Usage

| Metric | Idle | Active | Soak (30min) | Pass |
|--------|------|--------|--------------|------|
| CPU % | 0.5% | 3.2% | 3.5% | ✅ |
| Memory (MB) | 45 | 68 | 72 | ✅ |
| GPU % | 0% | 15% | 16% | ✅ |
| Temp (°C) | 42 | 55 | 58 | ✅ |

### Notes
- [Any observations, issues, or special conditions]

### Overall Result: ✅ PASS / ❌ FAIL
```

---

## Known Issues / Workarounds

### Issue 1: CVDisplayLink Thread Safety
**Symptom**: Rare crash in PerformanceMonitor
**Workaround**: Ensure `start()` and `stop()` called on main thread
**Status**: Under investigation

### Issue 2: Quality Not Persisting
**Symptom**: Manual quality override resets on launch
**Workaround**: Set quality in AppDelegate
**Status**: Fixed in PerformanceManager v1.1

---

## Hardware Not Available for Testing

If you don't have access to specific hardware:

1. **Simulate Quality Levels**:
   ```swift
   // Test low quality on M1 Mac
   PerformanceManager.shared.setQuality(.low)
   ```

2. **Synthetic GPU Load**:
   ```swift
   // Add artificial delay to simulate slower GPU
   Thread.sleep(forTimeInterval: 0.005) // 5ms per frame
   ```

3. **Request Community Testing**:
   - Post on GitHub asking for test results
   - Provide this testing guide
   - Collect FPS data from users

---

## Automated Performance Tests (Future)

```swift
// Example XCTest performance test
func testHaloRenderingPerformance() throws {
    measure(metrics: [XCTClockMetric(), XCTMemoryMetric()]) {
        // Render Halo 100 times
        for _ in 0..<100 {
            haloWindow.updateState(.listening)
            RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.016)) // 1 frame
        }
    }
}
```

---

## Status

- ⬜ M1/M2/M3 Mac tested
- ⬜ 2018+ Intel Mac tested
- ⬜ 2015 Intel Mac tested
- ⬜ AMD dGPU Mac tested
- ⬜ Instruments profiling completed
- ⬜ 30-minute soak test passed
- ⬜ All quality levels validated

**Blocker**: Requires physical access to diverse hardware
