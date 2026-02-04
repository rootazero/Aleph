# Halo Performance Specification

## ADDED Requirements

### Requirement: Maintain 60fps Frame Rate During Animations
Halo animations SHALL maintain 60fps frame rate on target hardware (2018+ Macs with M1 or Intel Iris Plus).

#### Scenario: Frame rate profiling during animations

**Given** Halo is animating (listening → processing → success)
**And** performance monitor is active
**When** measuring frame timestamps over 60 frames
**Then** average frame time is < 16.67ms (60fps)
**And** 95th percentile frame time < 20ms
**And** no dropped frames during transitions

---

### Requirement: Automatic Quality Degradation on Low Frame Rate
Performance monitoring SHALL detect frame rate drops and trigger quality degradation automatically.

#### Scenario: Automatic quality degradation on low FPS

**Given** app is running on 2015 MacBook Air (Intel HD 6000)
**When** PerformanceMonitor detects average FPS < 55 for 5 consecutive seconds
**Then** PerformanceManager sets effectsQuality to .medium
**And** posts .performanceDegradation notification
**And** themes disable Metal shaders
**And** animations switch to linear (no spring curves)

---

### Requirement: GPU Capability Detection on App Launch
GPU capabilities SHALL be detected on app launch to set initial quality level.

#### Scenario: High quality on M1 Mac

**Given** app launches on M1 MacBook Pro
**When** PerformanceManager queries Metal device
**Then** GPU name contains "Apple M1"
**And** effectsQuality sets to .high
**And** all theme effects enabled (shaders, gradients, complex animations)

---

#### Scenario: Low quality on Intel HD 3000

**Given** app launches on 2011 MacBook Air (Intel HD 3000)
**When** PerformanceManager queries Metal device
**Then** GPU name contains "Intel HD 3000"
**And** effectsQuality sets to .low
**And** themes use solid colors (no gradients)
**And** animations simplified (linear, no particles)

---

### Requirement: User-Overrideable Quality Degradation
Quality degradation SHALL be user-overrideable via hidden Settings preference.

#### Scenario: Manual quality override

**Given** PerformanceManager auto-detected .medium quality
**When** user runs terminal command: `defaults write com.aleph.app forceQualityHigh -bool YES`
**And** relaunches app
**Then** effectsQuality sets to .high
**And** auto-degradation disabled
**And** user takes responsibility for performance

---

### Requirement: Minimal Resource Usage During Animation
Halo rendering SHALL use minimal CPU (< 5%) and memory (< 10MB delta) during active animation.

#### Scenario: Resource usage profiling

**Given** Halo is animating (processing state with spinner)
**When** monitoring CPU usage with Activity Monitor
**Then** Aleph process CPU < 5%
**And** memory footprint delta < 10MB (vs idle state)
**And** no memory leaks after 1000 animation cycles

---

## Cross-References

- **Related Specs**: `halo-theming` (quality affects theme rendering)
- **Depends On**: Metal framework (GPU detection)
- **Blocks**: None
