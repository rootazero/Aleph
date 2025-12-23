# Performance Optimization Guide

## Overview
This document describes the performance optimization system implemented in Section 5 and provides guidelines for profiling and optimizing Halo rendering.

---

## Architecture

### Components

1. **PerformanceMonitor** - Real-time FPS tracking
2. **PerformanceManager** - GPU detection and quality management
3. **ThemeOptimizations** - Helper utilities for quality-adaptive rendering

### Quality Levels

| Level | Description | Target Hardware |
|-------|-------------|-----------------|
| **High** | Full effects, complex gradients, smooth animations | M1/M2/M3, high-end Intel/AMD |
| **Medium** | Simplified gradients, linear animations | 2018+ Intel Macs |
| **Low** | Solid colors, minimal animations | Pre-2015 Intel Macs |

---

## PerformanceMonitor Usage

### Starting/Stopping Monitoring

```swift
// Start FPS tracking
PerformanceMonitor.shared.start()

// Stop tracking
PerformanceMonitor.shared.stop()

// Get current FPS
let fps = PerformanceMonitor.shared.getFPS()

// Get average frame time (ms)
let frameTime = PerformanceMonitor.shared.getAverageFrameTime()

// Check if performance is acceptable (>= 55 FPS)
if PerformanceMonitor.shared.isPerformanceAcceptable() {
    print("Performance OK")
}
```

### Performance Drop Notifications

```swift
// Listen for performance drops
NotificationCenter.default.addObserver(
    forName: .performanceDropDetected,
    object: nil,
    queue: .main
) { notification in
    if let fps = notification.userInfo?["fps"] as? Double {
        print("⚠️ FPS dropped to \(fps)")
        // Take action: reduce quality, simplify effects, etc.
    }
}
```

---

## PerformanceManager Usage

### Detecting GPU and Quality

```swift
// Get current quality level
let quality = PerformanceManager.shared.effectsQuality
print("Quality: \(quality.rawValue)")

// Check quality programmatically
if PerformanceManager.shared.shouldUseHighQuality() {
    // Render with full effects
} else if PerformanceManager.shared.shouldUseLowQuality() {
    // Use simplified rendering
}

// Get GPU information
print("GPU: \(PerformanceManager.shared.gpuName)")
print("Family: \(PerformanceManager.shared.gpuFamily)")

// Print debug info
PerformanceManager.shared.printDebugInfo()
```

### Manual Quality Override

```swift
// Set quality manually (for testing)
PerformanceManager.shared.setQuality(.low)

// Reset to auto-detected quality
PerformanceManager.shared.resetToAutoDetected()

// Check if quality is manually overridden
if PerformanceManager.shared.isManualOverride {
    print("Quality manually set")
}
```

---

## Theme Optimization Utilities

### OptimizedGradient

```swift
// Radial gradient that adapts to quality
Circle()
    .fill(OptimizedGradient.radial(
        colors: [.blue, .purple, .clear],
        center: .center,
        startRadius: 20,
        endRadius: 60
    ))

// Linear gradient
Rectangle()
    .fill(OptimizedGradient.linear(
        colors: [.red, .orange],
        startPoint: .top,
        endPoint: .bottom
    ))
```

**Behavior**:
- **High**: Full gradient with all color stops
- **Medium**: Simplified to first and last colors only
- **Low**: Solid color (first color)

### OptimizedAnimation

```swift
// Adaptive animation
withAnimation(OptimizedAnimation.get(duration: 1.5, autoreverses: true)) {
    scale = 1.2
}

// Adaptive spring animation
withAnimation(OptimizedAnimation.spring()) {
    offset = 100
}
```

**Behavior**:
- **High**: Smooth easeInOut or spring animations
- **Medium**: Linear animations
- **Low**: Slower linear animations (reduced GPU load)

### Adaptive View Modifiers

```swift
// Adaptive blur
Circle()
    .adaptiveBlur(radius: 10)
// High: 10pt blur, Medium: 5pt blur, Low: no blur

// Adaptive shadow
RoundedRectangle(cornerRadius: 8)
    .adaptiveShadow(color: .black.opacity(0.3), radius: 5)
// High: full shadow, Medium: half shadow, Low: no shadow
```

### OptimizedRotatingView

```swift
// Rotating view that adapts to quality
OptimizedRotatingView(duration: 3.0) {
    Circle()
        .trim(from: 0, to: 0.7)
        .stroke(Color.blue, lineWidth: 3)
}
// Low quality: no rotation to save GPU
```

---

## Profiling with Instruments

### Steps to Profile HaloView

1. **Open Xcode Project**:
   ```bash
   open Aether.xcodeproj
   ```

2. **Select Profiling Template**:
   - Menu: Product → Profile (Cmd+I)
   - Choose "Time Profiler"

3. **Record During Halo Animation**:
   - Click Record
   - Trigger hotkey (Cmd+~) to show Halo
   - Let Halo animate for 5-10 seconds
   - Stop recording

4. **Analyze Results**:
   - Look for functions taking > 5ms per frame
   - Check "HaloView.body" render time
   - Identify SwiftUI layout bottlenecks

### Key Metrics to Track

| Metric | Target | Warning Threshold |
|--------|--------|-------------------|
| Average FPS | 60 | < 55 |
| Frame render time | < 16ms | > 18ms |
| CPU usage (animation) | < 5% | > 10% |
| Memory (Halo visible) | < 10MB | > 20MB |

### Common Bottlenecks

1. **Complex Gradients**:
   - **Problem**: RadialGradient with many color stops
   - **Solution**: Use OptimizedGradient or reduce color stops

2. **Nested ZStacks**:
   - **Problem**: Deep view hierarchy (> 5 levels)
   - **Solution**: Flatten hierarchy, use GeometryReader sparingly

3. **Frequent State Changes**:
   - **Problem**: @State changes every frame
   - **Solution**: Throttle updates, use DrawingGroup

4. **Blur/Shadow Effects**:
   - **Problem**: Expensive GPU operations
   - **Solution**: Use adaptive modifiers, disable on low quality

---

## Optimization Checklist

### For Theme Developers

- [ ] Use `OptimizedGradient` instead of raw `RadialGradient`
- [ ] Use `OptimizedAnimation` for all animations
- [ ] Apply `.adaptiveBlur()` instead of `.blur()`
- [ ] Apply `.adaptiveShadow()` instead of `.shadow()`
- [ ] Check `shouldUseLowQuality()` before complex effects
- [ ] Limit ZStack nesting to 3-4 levels
- [ ] Cache computed properties (use `@State` or `let`)
- [ ] Use `.drawingGroup()` for complex shapes

### For HaloView Rendering

- [ ] Profile with Instruments (Time Profiler)
- [ ] Ensure < 16ms render time at 60 FPS
- [ ] Minimize view updates during animation
- [ ] Use `@StateObject` for persistent managers
- [ ] Avoid creating new views in body
- [ ] Test on oldest supported hardware (2015 MacBook)

---

## Performance Overlay (Debug)

Add performance overlay to see real-time FPS:

```swift
// In HaloView.swift or debug menu
ZStack {
    // Your content
    HaloView(themeEngine: themeEngine)

    // Performance overlay (top-left)
    VStack {
        PerformanceOverlay()
        Spacer()
    }
    .frame(maxWidth: .infinity, alignment: .leading)
}
```

**Displays**:
- Current FPS (green if >= 55, red otherwise)
- Quality level (High/Medium/Low)
- GPU family (Apple Silicon, Intel, AMD)

---

## GPU Detection Logic

### Apple Silicon
- **M1/M2/M3**: High quality (excellent integrated GPU)
- Detection: `device.supportsFamily(.apple6+)`

### Intel
- **Iris Xe, Iris Plus 655+**: High quality
- **Iris Plus, UHD 630+**: Medium quality
- **HD 3000/4000/5000/6000**: Low quality
- Detection: String matching on GPU name

### AMD
- **Radeon Pro 5000+, Vega**: High quality
- **Radeon Pro 400-500**: Medium quality
- Detection: String matching on GPU name

### NVIDIA
- All discrete NVIDIA GPUs: High quality
- Detection: String matching on GPU name

---

## Testing on Different Hardware

### M1/M2 Mac (High Quality)
**Expected**:
- 60 FPS constant during all animations
- Full effects: complex gradients, smooth springs
- No thermal throttling after 30 min

**Commands**:
```swift
// Force high quality for testing
PerformanceManager.shared.setQuality(.high)
```

### 2018 Intel Mac (Medium Quality)
**Expected**:
- 60 FPS with simplified effects
- Linear animations instead of springs
- Reduced gradient complexity

**Commands**:
```swift
// Force medium quality
PerformanceManager.shared.setQuality(.medium)
```

### 2015 MacBook Air (Low Quality)
**Expected**:
- 30+ FPS minimum
- Solid colors instead of gradients
- Minimal animations
- No blur or shadow effects

**Commands**:
```swift
// Force low quality
PerformanceManager.shared.setQuality(.low)
```

---

## Troubleshooting

### FPS Below 55 on M1 Mac
1. Check for background processes (Activity Monitor)
2. Profile with Instruments to find bottleneck
3. Verify vsync is enabled (60Hz display)
4. Check for memory leaks

### Quality Not Auto-Detecting Correctly
1. Verify Metal device is detected:
   ```swift
   print(PerformanceManager.shared.gpuName)
   ```
2. Check GPU name matching logic
3. Set quality manually as workaround

### Performance Overlay Not Updating
1. Ensure `PerformanceMonitor.shared.start()` called
2. Check display link is running
3. Verify main thread not blocked

---

## Integration with Existing Code

### AppDelegate Integration

```swift
func applicationDidFinishLaunching(_ notification: Notification) {
    // ... existing setup

    // Print GPU info on launch
    PerformanceManager.shared.printDebugInfo()

    // Start performance monitoring (optional, for debugging)
    #if DEBUG
    PerformanceMonitor.shared.start()
    #endif
}
```

### EventHandler Integration

```swift
// Listen for performance drops
NotificationCenter.default.addObserver(
    forName: .performanceDropDetected,
    object: nil,
    queue: .main
) { [weak self] notification in
    if let fps = notification.userInfo?["fps"] as? Double {
        print("[Aether] ⚠️ Performance drop: \(fps) FPS")
        // Could automatically reduce quality here
    }
}
```

---

## Future Enhancements

1. **Dynamic Quality Adjustment**:
   - Auto-downgrade quality if FPS drops
   - Auto-upgrade after sustained 60 FPS

2. **Per-Theme Quality Profiles**:
   - Some themes more expensive than others
   - Allow theme-specific quality overrides

3. **Thermal Monitoring**:
   - Detect thermal throttling
   - Reduce quality to prevent overheating

4. **Battery Awareness**:
   - Lower quality on battery power
   - Extend battery life

---

## Status

- ✅ PerformanceMonitor implemented
- ✅ PerformanceManager with GPU detection
- ✅ ThemeOptimizations utilities
- ✅ Quality level system
- ⬜ Full theme integration (manual work required)
- ⬜ Instruments profiling session
- ⬜ Hardware testing on old Macs

**Ready for**: Manual profiling and hardware testing
