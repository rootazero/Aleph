# Performance Testing Guide - Modernize Settings UI

## Purpose

This document outlines performance testing procedures to ensure the Modernized Settings UI maintains 60fps animations, fast search response times, and efficient memory usage.

## Test Environment

### Hardware Requirements
- **Primary**: Modern Mac (M1/M2/M3 or recent Intel)
- **Low-end**: 2020 Intel MacBook Air or equivalent
- **RAM**: Monitor performance with both 8GB and 16GB+ configurations

### Software Requirements
- Xcode 15.0+ with Instruments
- macOS 13, 14, or 15
- Aleph built in Release configuration

### Pre-Test Setup
1. Build in Release mode:
   ```bash
   xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Release build
   ```
2. Close all other applications
3. Disable Spotlight indexing temporarily (for consistent results)
4. Connect power adapter (disable battery throttling)
5. Set Energy Saver to "High Performance"

## 6.3.1 Instruments Profiling

### Time Profiler Test

#### Objective
Identify CPU-intensive operations and ensure no single function blocks the UI thread for > 100ms.

#### Procedure
1. **Launch Instruments**
   ```bash
   open -a Instruments
   ```
2. **Select Template**: Time Profiler
3. **Choose Target**: Aleph.app (Release build)
4. **Configure**:
   - Recording mode: Immediate
   - Sample rate: 1ms (high resolution)
5. **Record Session** (30 seconds):
   - 0-5s: Launch app, open Settings
   - 5-10s: Navigate General → Providers → Routing → Shortcuts
   - 10-15s: In Providers tab, type "openai" in search bar
   - 15-20s: Scroll provider list up and down
   - 20-25s: Click Light → Dark → Auto theme switcher rapidly
   - 25-30s: Resize window from 800x600 to 1600x1000

6. **Stop Recording**

#### Analysis Checklist
- [ ] **Main Thread**: No single call > 100ms
- [ ] **Heaviest Stack Trace**: Identify top 5 functions
  - [ ] None should be UI rendering (should be in background)
  - [ ] SwiftUI body calculations should be < 16ms (60fps)
- [ ] **Search Performance**: Text input to filter update < 50ms
- [ ] **Theme Switching**: Complete within 300ms
- [ ] **Navigation**: Tab switching < 100ms

#### Expected Results
```
Top 5 Hotspots (example acceptable values):
1. SwiftUI::View::body - 5-10% (distributed across views)
2. Metal::renderPass - 3-5% (GPU rendering)
3. ProviderCard::init - 1-2%
4. ThemeManager::applyTheme - 1-2%
5. SearchBar::filter - < 1%

Main Thread Utilization: < 60% average
```

#### Failure Conditions
- ❌ Any function > 200ms on main thread
- ❌ Search filtering > 100ms
- ❌ Theme switching > 500ms
- ❌ Main thread blocked > 80% during animations

#### Save Report
- Export: `docs/testing/phase6/time-profiler-[DATE].trace`
- Screenshot: `docs/testing/phase6/time-profiler-summary.png`

---

### Core Animation Test

#### Objective
Verify all animations maintain 60fps with no dropped frames.

#### Procedure
1. **Launch Instruments**
2. **Select Template**: Core Animation
3. **Configure**:
   - Enable "Color Blended Layers" (optional, for debugging)
   - Enable "Color Offscreen-Rendered Yellow"
4. **Record Session** (20 seconds):
   - 0-5s: Hover over provider cards (if hover detectable)
   - 5-10s: Click sidebar items repeatedly (General → Providers → General)
   - 10-15s: Toggle theme switcher (Light → Dark → Light → Auto)
   - 15-20s: Open and close detail panel by clicking provider cards

5. **Stop Recording**

#### Analysis Checklist
- [ ] **Frame Rate**: Maintained 60fps (16.67ms per frame)
  - [ ] During sidebar navigation: 60fps
  - [ ] During theme switching: 60fps
  - [ ] During card hover: 60fps
  - [ ] During detail panel animation: 60fps
- [ ] **Dropped Frames**: < 5% of total frames
- [ ] **GPU Utilization**: < 30% average
- [ ] **Offscreen Rendering**: Minimal yellow highlights (cards may be yellow due to shadows)

#### Frame Rate Targets
```
Acceptable Frame Rates:
- Sidebar navigation: 60fps (no drops)
- Theme switching: 55-60fps (brief drop acceptable during first frame)
- Provider card hover: 60fps
- Detail panel slide: 60fps
- Search filtering: 60fps during animation
```

#### Failure Conditions
- ❌ Sustained frame rate < 55fps
- ❌ Dropped frames > 10%
- ❌ GPU usage > 50% (indicates inefficient rendering)
- ❌ Excessive offscreen rendering (entire screen yellow)

#### Save Report
- Export: `docs/testing/phase6/core-animation-[DATE].trace`
- Screenshot: `docs/testing/phase6/core-animation-fps-graph.png`

---

### Allocations & Leaks Test

#### Objective
Verify no memory leaks and reasonable memory usage for settings window.

#### Procedure
1. **Launch Instruments**
2. **Select Template**: Allocations
3. **Add Instrument**: Leaks (click + button, add Leaks)
4. **Record Session** (60 seconds):
   - 0-10s: Open Settings window
   - 10-20s: Navigate through all tabs
   - 20-30s: Perform actions (search, edit provider, etc.)
   - 30-40s: Close Settings window (Cmd+W)
   - 40-50s: Reopen Settings window (Cmd+,)
   - 50-60s: Close Settings window again

5. **Stop Recording**

#### Analysis Checklist

**Allocations**
- [ ] **Persistent Growth**: Memory should return to baseline after closing window
- [ ] **Peak Memory**: Settings window < 200MB total (including app)
- [ ] **Live Bytes**: Check for objects that should be deallocated:
  - [ ] NSWindow deallocated when window closed
  - [ ] NSView instances deallocated
  - [ ] SwiftUI state objects deallocated
- [ ] **Generations**: Mark generation after closing window, verify no growth

**Leaks**
- [ ] **Total Leaks**: Zero leaks detected
- [ ] **Leaked Blocks**: None
- [ ] **Common Leak Sources** (check these specifically):
  - [ ] No leaked NSWindow
  - [ ] No leaked NSView
  - [ ] No leaked NSImage
  - [ ] No leaked closure captures

#### Memory Usage Targets
```
Acceptable Memory Usage:
- App baseline: 30-50MB
- Settings window open: 80-150MB
- After closing window: Returns to baseline (± 10MB)
- Peak during animations: < 200MB
```

#### Failure Conditions
- ❌ Any leaks detected (0 tolerance)
- ❌ Memory > 300MB at any point
- ❌ Memory doesn't return to baseline after closing window (indicates leak)
- ❌ Persistent growth after 3x open/close cycles

#### Save Report
- Export: `docs/testing/phase6/allocations-[DATE].trace`
- Screenshot: `docs/testing/phase6/allocations-graph.png`
- If leaks found: `docs/testing/phase6/leaks-details.png`

---

## 6.3.2 Large Dataset Performance

### Objective
Verify UI remains responsive with 50+ providers.

### Setup: Generate Test Data
1. Create test configuration:
   ```bash
   cd ~/.aether
   cp config.toml config.toml.backup
   ```

2. Generate 50 providers:
   ```bash
   cat > test_providers.sh << 'EOF'
   #!/bin/bash
   CONFIG=~/.aleph/config.toml

   # Backup existing
   cp $CONFIG ${CONFIG}.backup

   # Generate 50 providers
   for i in {1..50}; do
     cat >> $CONFIG << PROVIDER

   [providers.test_provider_$i]
   api_key = "sk-test-key-$i-xxxxxxxxxxxxxxxx"
   model = "gpt-4"
   base_url = "https://api.openai.com/v1"
   color = "#10a37f"
   max_tokens = 4096
   temperature = 0.7
   PROVIDER
   done

   echo "Generated 50 test providers"
   EOF

   chmod +x test_providers.sh
   ./test_providers.sh
   ```

### Test Procedure

#### Scroll Performance
1. [ ] Open Aleph Settings
2. [ ] Navigate to Providers tab
3. [ ] Verify 50+ provider cards visible in list
4. [ ] Scroll to bottom (smooth scroll, not jump)
   - **Measure**: Use 240fps screen recording
   - **Metric**: Should maintain 60fps
5. [ ] Scroll to top
6. [ ] Rapid scroll up and down 10 times
   - [ ] No jank or stuttering
   - [ ] All cards render instantly when visible
   - [ ] No loading spinners or blank cards

#### Search Performance
1. [ ] Type "test" in search bar
   - **Measure**: Time from keypress to filtered results visible
   - **Metric**: < 50ms response time
2. [ ] Verify ~50 results shown (all test providers)
3. [ ] Type "provider_25"
   - **Metric**: < 50ms response time
4. [ ] Verify only 1 result shown
5. [ ] Clear search (click X button)
   - **Metric**: < 50ms to restore all results
6. [ ] Type random strings and clear 10 times
   - [ ] All searches < 50ms
   - [ ] No freezing or lag

#### Memory Usage
1. [ ] Open Activity Monitor
2. [ ] Find Aleph process
3. [ ] Record memory usage:
   - With 50 providers: _______ MB
4. [ ] **Target**: < 200MB total
5. [ ] Scroll through all providers
6. [ ] Verify memory doesn't grow continuously

#### Rendering Performance
1. [ ] Use Instruments Core Animation
2. [ ] Record while scrolling through 50 providers
3. [ ] Verify:
   - [ ] Frame rate: 60fps
   - [ ] No long frames (> 16.67ms)
4. [ ] Click on providers 1, 10, 25, 50
5. [ ] Verify detail panel loads instantly (< 100ms)

### Cleanup
```bash
cd ~/.aether
mv config.toml.backup config.toml
```

### Expected Results
```
Scroll Performance: 60fps maintained
Search Response:
  - Average: 20-30ms
  - Max: < 50ms
Memory Usage: 120-180MB
Detail Panel Load: < 100ms
```

### Failure Conditions
- ❌ Scroll drops below 55fps
- ❌ Search takes > 100ms
- ❌ Memory exceeds 250MB
- ❌ App freezes or becomes unresponsive

---

## 6.3.3 Animation Smoothness Manual Tests

### Provider Card Hover Animation

**Test**:
1. [ ] Navigate to Providers tab
2. [ ] Hover over a provider card
3. [ ] Observe scale animation (should grow to 1.02)
4. [ ] Observe shadow deepening
5. [ ] Move mouse out
6. [ ] Observe card returning to normal

**Metrics**:
- [ ] Animation duration: ~200ms (feels natural)
- [ ] No stuttering during scale
- [ ] Shadow transition smooth
- [ ] No "pop" in or out

**Recording**: Use 120fps or 240fps screen recording to verify smoothness

---

### Sidebar Selection Animation

**Test**:
1. [ ] Click General tab
2. [ ] Observe blue indicator sliding in (left edge)
3. [ ] Click Providers tab
4. [ ] Observe indicator sliding to Providers
5. [ ] Rapidly click: General → Providers → Routing → General

**Metrics**:
- [ ] Indicator slide duration: ~300ms
- [ ] Smooth motion (no jumping)
- [ ] No overlapping indicators during rapid clicks
- [ ] Background color transitions smoothly

---

### Detail Panel Appear/Disappear

**Test**:
1. [ ] In Providers tab, click a provider card
2. [ ] Observe detail panel sliding in from right
3. [ ] Click different provider
4. [ ] Observe panel updating (fade out old, fade in new)
5. [ ] Click outside or same card to close
6. [ ] Observe panel sliding out

**Metrics**:
- [ ] Slide duration: ~300ms
- [ ] Fade duration: ~200ms
- [ ] No content jumping
- [ ] Opacity transition smooth (not abrupt)

---

### Search Results Filter Animation

**Test**:
1. [ ] Start with 10+ providers visible
2. [ ] Type in search bar
3. [ ] Observe non-matching cards fading out
4. [ ] Observe matching cards moving up to fill space
5. [ ] Clear search
6. [ ] Observe cards fading back in

**Metrics**:
- [ ] Fade out: ~200ms
- [ ] Move transition: ~300ms
- [ ] Cards don't overlap during animation
- [ ] Smooth stagger (not all at once)

---

### Theme Switching Animation

**Test**:
1. [ ] Click Light mode
2. [ ] Observe color transitions
3. [ ] Click Dark mode immediately
4. [ ] Click Auto mode
5. [ ] Rapidly click: Light → Dark → Light (10 times)

**Metrics**:
- [ ] Color transition: ~300ms
- [ ] All elements update simultaneously (no elements "left behind")
- [ ] No white or black flash
- [ ] Smooth color interpolation
- [ ] Rapid switching doesn't cause lag or crash

---

### Window Resize Animation

**Test**:
1. [ ] Grab window corner
2. [ ] Slowly drag to resize smaller
3. [ ] Drag to resize larger
4. [ ] Rapidly resize in all directions

**Metrics**:
- [ ] Layout updates in real-time (no delay)
- [ ] Elements don't jump or flicker
- [ ] Constraints resolve smoothly
- [ ] Text reflows without jumping
- [ ] Detail panel collapses/expands smoothly

---

## 6.3.4 Low-End Device Testing

### Test Device
**Ideal**: 2020 Intel MacBook Air (i3, 8GB RAM)

If not available, simulate with:
- CPU Throttling in Xcode
- Activity Monitor: Force CPU limit
- Rosetta mode on Apple Silicon

### Test Procedure

1. **Launch App on Low-End Device**
   - [ ] App launches within 5 seconds
   - [ ] No beach ball cursor

2. **Open Settings Window**
   - [ ] Opens within 2 seconds
   - [ ] No delay rendering UI

3. **Navigate All Tabs**
   - [ ] Each tab loads within 1 second
   - [ ] Animations still smooth (may drop to 50-55fps, acceptable)

4. **Theme Switching**
   - [ ] Light → Dark within 1 second
   - [ ] No hang or freeze

5. **Search Providers**
   - [ ] Results filter within 100ms (slightly slower than 50ms target)
   - [ ] Typing not laggy

6. **Monitor Thermal**
   - [ ] Run for 5 minutes
   - [ ] Check fan speed (shouldn't spin to max)
   - [ ] Check temperature (shouldn't thermal throttle)

### Acceptance Criteria
- [ ] All features functional
- [ ] Animations may be 50fps (acceptable)
- [ ] No freezes or crashes
- [ ] Response times < 2x normal device

### Failure Conditions
- ❌ App crashes or hangs
- ❌ Response times > 3x normal
- ❌ Thermal throttling to point of unusability
- ❌ Fan at max speed continuously

---

## Performance Test Results Template

```markdown
# Performance Testing Results - Phase 6.3

**Tester**: [Name]
**Date**: [YYYY-MM-DD]
**Device**: [MacBook Pro 16" M1 Max / etc.]
**macOS**: [15.2 / etc.]
**Build**: [Release / Debug]

## 6.3.1 Instruments Profiling

### Time Profiler
- [ ] Pass / [ ] Fail
- Main thread max: _____ ms
- Top hotspot: _____________________ (___%)
- Search performance: _____ ms
- Theme switch: _____ ms
- Screenshot: ✅ Saved

### Core Animation
- [ ] Pass / [ ] Fail
- Average FPS: _____
- Dropped frames: _____%
- GPU usage: _____%
- Screenshot: ✅ Saved

### Allocations & Leaks
- [ ] Pass / [ ] Fail
- Peak memory: _____ MB
- Leaks found: _____ (target: 0)
- Memory returns to baseline: Yes / No
- Screenshot: ✅ Saved

## 6.3.2 Large Dataset (50 Providers)

- [ ] Pass / [ ] Fail
- Scroll FPS: _____
- Search response: _____ ms (avg)
- Memory usage: _____ MB
- Detail load time: _____ ms

## 6.3.3 Animation Smoothness

- [ ] Provider card hover: Pass / Fail
- [ ] Sidebar selection: Pass / Fail
- [ ] Detail panel: Pass / Fail
- [ ] Search filter: Pass / Fail
- [ ] Theme switching: Pass / Fail
- [ ] Window resize: Pass / Fail

## 6.3.4 Low-End Device

- [ ] Pass / [ ] Fail / [ ] Not Tested
- Device: [Intel MacBook Air 2020 / Simulated]
- Launch time: _____ s
- Settings open: _____ s
- Animations: Smooth / Acceptable / Laggy
- Thermal: No issues / Minor / Significant

## Overall Performance Assessment

- [ ] All metrics meet targets
- [ ] Minor performance issues (document below)
- [ ] Major performance issues (requires fixes)

## Issues Found
1. [Description]
2. [Description]

## Action Items
1. [Fix needed]
2. [Fix needed]
```

### Save Results
`docs/testing/phase6/performance-test-results-[DATE].md`

---

## Performance Targets Summary

| Metric | Target | Acceptable | Failure |
|--------|--------|------------|---------|
| Animation FPS | 60fps | 55fps | < 55fps |
| Search Response | < 50ms | < 100ms | > 100ms |
| Main Thread Block | < 100ms | < 200ms | > 200ms |
| Theme Switch | < 300ms | < 500ms | > 500ms |
| Memory (Peak) | < 150MB | < 200MB | > 250MB |
| Memory Leaks | 0 | 0 | > 0 |
| GPU Usage | < 20% | < 30% | > 50% |
| Launch Time | < 2s | < 5s | > 5s |

---

## Approval Criteria

Performance testing PASSES when:
- ✅ Time Profiler: No function > 200ms on main thread
- ✅ Core Animation: Average FPS ≥ 55fps
- ✅ Allocations: Zero leaks, memory < 200MB peak
- ✅ Large dataset: Search < 100ms, scroll smooth
- ✅ Animations: All subjectively smooth to tester
- ✅ Low-end device: Functional and acceptable performance

**Approved By**: ___________
**Date**: ___________
