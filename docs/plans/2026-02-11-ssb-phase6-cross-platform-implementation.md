# SSB Phase 6: Cross-Platform Implementation Plan

**Date**: 2026-02-11
**Status**: Planning
**Dependencies**: Phase 1-5 (Core Infrastructure) completed
**Target**: Cross-platform perception and actuation

## Executive Summary

Phase 6 implements the cross-platform architecture for System State Bus, enabling Aleph Server to run on macOS, Linux, and Windows with graceful degradation. The implementation follows a tiered perception strategy: structured APIs (Level 1) → local vision (Level 2) → cloud vision (Level 3).

**Key Deliverables**:
1. Platform Abstraction Layer (SystemSensor trait)
2. macOS implementation (reference platform)
3. Windows/Linux skeleton implementations
4. Cross-platform screenshot and OCR
5. Permission health check system
6. Graceful degradation for headless environments

## Architecture Recap

### Tiered Perception Strategy

```
Level 1: Structured API (AX/UIA/AT-SPI) → Precision: High, Cost: Low
Level 2: Local Vision (Screenshot + OCR) → Precision: Medium, Cost: Medium
Level 3: Cloud Vision (Multimodal API)   → Precision: Highest, Cost: High
```

### Platform Support Matrix

| Platform | Structured API | Screenshot | OCR | Input Sim | Status |
|----------|---------------|------------|-----|-----------|--------|
| macOS    | AX API        | ScreenCaptureKit | ✅ | CGEvent | Phase 6 |
| Windows  | UI Automation | GDI+       | ✅ | SendInput | Phase 7 |
| Linux    | AT-SPI        | X11/Wayland | ✅ | XTest | Phase 7 |
| Headless | ❌            | ❌         | ✅ | ❌ | Phase 6 |

## Task Breakdown

### Task 1: Platform Abstraction Layer (PAL) ⭐ Priority 1

**Goal**: Define cross-platform traits and types for perception and actuation.

**Files to Create**:
- `core/src/perception/sensor.rs` - SystemSensor trait
- `core/src/perception/actuator.rs` - InputActuator trait
- `core/src/perception/health.rs` - PerceptionHealth
- `core/src/perception/types.rs` - Shared types (UINodeTree, SensorCapabilities)

**Step 1: Define SystemSensor trait**

```rust
// core/src/perception/sensor.rs

use async_trait::async_trait;
use image::DynamicImage;
use crate::AlephError;

#[async_trait]
pub trait SystemSensor: Send + Sync {
    /// Get currently focused application ID (bundle ID / process name)
    async fn get_focused_app(&self) -> Result<String, AlephError>;

    /// Capture UI tree (AX Tree / UIA Tree / AT-SPI)
    async fn capture_ui_tree(&self, app_id: &str) -> Result<UINodeTree, AlephError>;

    /// Visual fallback: capture screenshot
    async fn capture_screenshot(&self) -> Result<DynamicImage, AlephError>;

    /// Check if sensor is available in current environment
    fn is_available(&self) -> bool;

    /// Get sensor capabilities
    fn capabilities(&self) -> SensorCapabilities;

    /// Get sensor name for logging
    fn name(&self) -> &'static str;
}

#[derive(Debug, Clone)]
pub struct SensorCapabilities {
    pub has_structured_api: bool,
    pub has_screenshot: bool,
    pub has_event_notifications: bool,
    pub platform: Platform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOS,
    Windows,
    Linux,
    Unknown,
}
```

**Step 2: Define UINodeTree type**

```rust
// core/src/perception/types.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UINodeTree {
    pub root: UINode,
    pub timestamp: i64,
    pub app_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UINode {
    pub id: String,
    pub role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub rect: Rect,
    pub state: UINodeState,
    pub children: Vec<UINode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UINodeState {
    pub focused: bool,
    pub enabled: bool,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
```

**Step 3: Define InputActuator trait**

```rust
// core/src/perception/actuator.rs

use async_trait::async_trait;
use crate::AlephError;

#[async_trait]
pub trait InputActuator: Send + Sync {
    /// Click at absolute screen coordinates
    async fn click(&self, x: i32, y: i32) -> Result<(), AlephError>;

    /// Type text (respects current input focus)
    async fn type_text(&self, text: &str) -> Result<(), AlephError>;

    /// Press a key (with optional modifiers)
    async fn press_key(&self, key: Key, modifiers: &[Modifier]) -> Result<(), AlephError>;

    /// Check if actuator is available
    fn is_available(&self) -> bool;
}

#[derive(Debug, Clone, Copy)]
pub enum Key {
    Return,
    Tab,
    Escape,
    Space,
    Backspace,
    Delete,
    // ... more keys
}

#[derive(Debug, Clone, Copy)]
pub enum Modifier {
    Command,
    Control,
    Alt,
    Shift,
}
```

**Step 4: Define PerceptionHealth**

```rust
// core/src/perception/health.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptionHealth {
    pub accessibility_enabled: bool,
    pub screen_recording_enabled: bool,
    pub input_monitoring_enabled: bool,
    pub platform_support: PlatformSupport,
    pub available_sensors: Vec<String>,
    pub recommendations: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PlatformSupport {
    Full,      // All features available
    Partial,   // Some features missing
    Degraded,  // Only basic features
    None,      // No GUI support (headless)
}

impl PerceptionHealth {
    pub async fn check() -> Self {
        // Platform-specific implementation
        #[cfg(target_os = "macos")]
        return Self::check_macos().await;

        #[cfg(target_os = "windows")]
        return Self::check_windows().await;

        #[cfg(target_os = "linux")]
        return Self::check_linux().await;

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        return Self::unsupported();
    }

    #[cfg(target_os = "macos")]
    async fn check_macos() -> Self {
        // Check AX permission, Screen Recording, etc.
        todo!("Implement macOS health check")
    }

    fn unsupported() -> Self {
        Self {
            accessibility_enabled: false,
            screen_recording_enabled: false,
            input_monitoring_enabled: false,
            platform_support: PlatformSupport::None,
            available_sensors: vec![],
            recommendations: vec![
                "Platform not supported for UI perception".to_string(),
            ],
        }
    }
}
```

**Step 5: Update SystemStateBus to use SystemSensor**

```rust
// core/src/perception/state_bus/mod.rs

use crate::perception::sensor::SystemSensor;

pub struct SystemStateBus {
    sensor: Arc<dyn SystemSensor>,
    // ... existing fields
}

impl SystemStateBus {
    pub fn new(sensor: Arc<dyn SystemSensor>) -> Self {
        Self {
            sensor,
            // ... initialize other fields
        }
    }

    pub async fn sense_ui(&self, app_id: &str) -> Result<AppState> {
        // Level 1: Try structured API
        if self.sensor.capabilities().has_structured_api {
            if let Ok(tree) = self.sensor.capture_ui_tree(app_id).await {
                return Ok(self.tree_to_state(tree));
            }
        }

        // Level 2: Fallback to screenshot + OCR
        if self.sensor.capabilities().has_screenshot {
            let screenshot = self.sensor.capture_screenshot().await?;
            // TODO: OCR processing
        }

        Err(AlephError::perception("No available perception method"))
    }
}
```

**Step 6: Write tests**

```rust
// core/tests/perception_pal.rs

#[tokio::test]
async fn test_sensor_trait_object_safety() {
    // Verify trait is object-safe
    let _: Box<dyn SystemSensor>;
}

#[tokio::test]
async fn test_health_check() {
    let health = PerceptionHealth::check().await;
    assert!(health.platform_support != PlatformSupport::None || cfg!(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux"
    ))));
}
```

**Step 7: Commit**

```bash
git add core/src/perception/{sensor,actuator,health,types}.rs
git add core/tests/perception_pal.rs
git commit -m "feat(perception): add Platform Abstraction Layer (PAL)

- Define SystemSensor trait for cross-platform UI sensing
- Define InputActuator trait for cross-platform input simulation
- Add PerceptionHealth for permission and capability checking
- Add shared types (UINodeTree, Rect, SensorCapabilities)
- Update SystemStateBus to use SystemSensor trait"
```

---

### Task 2: macOS Sensor Implementation ⭐ Priority 2

**Goal**: Implement SystemSensor for macOS using Accessibility API.

**Files to Create**:
- `core/src/perception/sensors/macos.rs` - MacosSensor implementation
- `core/src/perception/sensors/macos/ax_bridge.rs` - AX API FFI bindings
- `core/src/perception/sensors/macos/screen_capture.rs` - ScreenCaptureKit wrapper

**Step 1: Create MacosSensor skeleton**

```rust
// core/src/perception/sensors/macos.rs

use crate::perception::sensor::{SystemSensor, SensorCapabilities, Platform};
use crate::perception::types::{UINodeTree, UINode};
use async_trait::async_trait;

pub struct MacosSensor {
    ax_bridge: AxBridge,
    screen_capture: ScreenCapture,
}

impl MacosSensor {
    pub fn new() -> Result<Self, AlephError> {
        Ok(Self {
            ax_bridge: AxBridge::new()?,
            screen_capture: ScreenCapture::new()?,
        })
    }
}

#[async_trait]
impl SystemSensor for MacosSensor {
    async fn get_focused_app(&self) -> Result<String, AlephError> {
        self.ax_bridge.get_focused_app()
    }

    async fn capture_ui_tree(&self, app_id: &str) -> Result<UINodeTree, AlephError> {
        self.ax_bridge.capture_tree(app_id)
    }

    async fn capture_screenshot(&self) -> Result<DynamicImage, AlephError> {
        self.screen_capture.capture()
    }

    fn is_available(&self) -> bool {
        self.ax_bridge.check_permission()
    }

    fn capabilities(&self) -> SensorCapabilities {
        SensorCapabilities {
            has_structured_api: true,
            has_screenshot: true,
            has_event_notifications: true,
            platform: Platform::MacOS,
        }
    }

    fn name(&self) -> &'static str {
        "MacosSensor"
    }
}
```

**Step 2: Implement AX API bridge (simplified)**

For Phase 6, we'll use a simplified approach that calls existing `ax_observer` code:

```rust
// core/src/perception/sensors/macos/ax_bridge.rs

use crate::perception::state_bus::ax_observer::AxObserver;

pub struct AxBridge {
    observer: AxObserver,
}

impl AxBridge {
    pub fn new() -> Result<Self, AlephError> {
        Ok(Self {
            observer: AxObserver::new()?,
        })
    }

    pub fn get_focused_app(&self) -> Result<String, AlephError> {
        // Use existing AX observer code
        self.observer.get_focused_app()
    }

    pub fn capture_tree(&self, app_id: &str) -> Result<UINodeTree, AlephError> {
        // Convert existing AX tree to UINodeTree
        let ax_tree = self.observer.capture_tree(app_id)?;
        Ok(self.convert_tree(ax_tree))
    }

    pub fn check_permission(&self) -> bool {
        // Check AX permission
        self.observer.check_permission()
    }

    fn convert_tree(&self, ax_tree: /* existing type */) -> UINodeTree {
        // Convert to standard UINodeTree format
        todo!()
    }
}
```

**Step 3: Implement screenshot capture**

```rust
// core/src/perception/sensors/macos/screen_capture.rs

use image::DynamicImage;

pub struct ScreenCapture;

impl ScreenCapture {
    pub fn new() -> Result<Self, AlephError> {
        Ok(Self)
    }

    pub fn capture(&self) -> Result<DynamicImage, AlephError> {
        // Use existing snapshot_capture tool or ScreenCaptureKit
        // For Phase 6, reuse existing code
        todo!("Integrate with existing screenshot code")
    }
}
```

**Step 4: Write integration tests**

```rust
// core/tests/macos_sensor.rs

#[cfg(target_os = "macos")]
#[tokio::test]
async fn test_macos_sensor_creation() {
    let sensor = MacosSensor::new();
    assert!(sensor.is_ok() || !sensor.unwrap().is_available());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn test_macos_sensor_capabilities() {
    if let Ok(sensor) = MacosSensor::new() {
        let caps = sensor.capabilities();
        assert_eq!(caps.platform, Platform::MacOS);
        assert!(caps.has_structured_api);
    }
}
```

**Step 5: Commit**

```bash
git add core/src/perception/sensors/macos/
git add core/tests/macos_sensor.rs
git commit -m "feat(perception): implement MacosSensor

- Implement SystemSensor trait for macOS
- Bridge to existing AxObserver code
- Add screenshot capture integration
- Add platform-specific tests"
```

---

### Task 3: Windows/Linux Skeleton Implementations ⭐ Priority 3

**Goal**: Create skeleton implementations that return `NotSupported` errors.

**Step 1: Windows skeleton**

```rust
// core/src/perception/sensors/windows.rs

#[cfg(target_os = "windows")]
pub struct WindowsSensor;

#[cfg(target_os = "windows")]
impl WindowsSensor {
    pub fn new() -> Result<Self, AlephError> {
        Ok(Self)
    }
}

#[cfg(target_os = "windows")]
#[async_trait]
impl SystemSensor for WindowsSensor {
    async fn get_focused_app(&self) -> Result<String, AlephError> {
        Err(AlephError::not_supported("Windows sensor not yet implemented"))
    }

    async fn capture_ui_tree(&self, _app_id: &str) -> Result<UINodeTree, AlephError> {
        Err(AlephError::not_supported("Windows sensor not yet implemented"))
    }

    async fn capture_screenshot(&self) -> Result<DynamicImage, AlephError> {
        Err(AlephError::not_supported("Windows sensor not yet implemented"))
    }

    fn is_available(&self) -> bool {
        false
    }

    fn capabilities(&self) -> SensorCapabilities {
        SensorCapabilities {
            has_structured_api: false,
            has_screenshot: false,
            has_event_notifications: false,
            platform: Platform::Windows,
        }
    }

    fn name(&self) -> &'static str {
        "WindowsSensor (not implemented)"
    }
}
```

**Step 2: Linux skeleton**

```rust
// core/src/perception/sensors/linux.rs

#[cfg(target_os = "linux")]
pub struct LinuxSensor;

#[cfg(target_os = "linux")]
impl LinuxSensor {
    pub fn new() -> Result<Self, AlephError> {
        Ok(Self)
    }

    fn has_display(&self) -> bool {
        std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok()
    }
}

#[cfg(target_os = "linux")]
#[async_trait]
impl SystemSensor for LinuxSensor {
    async fn get_focused_app(&self) -> Result<String, AlephError> {
        if !self.has_display() {
            return Err(AlephError::not_supported(
                "UI sensing requires display server (X11/Wayland)"
            ));
        }
        Err(AlephError::not_supported("Linux sensor not yet implemented"))
    }

    async fn capture_ui_tree(&self, _app_id: &str) -> Result<UINodeTree, AlephError> {
        if !self.has_display() {
            return Err(AlephError::not_supported(
                "UI sensing requires display server (X11/Wayland)"
            ));
        }
        Err(AlephError::not_supported("Linux sensor not yet implemented"))
    }

    async fn capture_screenshot(&self) -> Result<DynamicImage, AlephError> {
        if !self.has_display() {
            return Err(AlephError::not_supported(
                "Screenshot requires display server (X11/Wayland)"
            ));
        }
        Err(AlephError::not_supported("Linux sensor not yet implemented"))
    }

    fn is_available(&self) -> bool {
        self.has_display()
    }

    fn capabilities(&self) -> SensorCapabilities {
        SensorCapabilities {
            has_structured_api: false,
            has_screenshot: false,
            has_event_notifications: false,
            platform: Platform::Linux,
        }
    }

    fn name(&self) -> &'static str {
        "LinuxSensor (not implemented)"
    }
}
```

**Step 3: Sensor factory**

```rust
// core/src/perception/sensors/mod.rs

pub mod macos;
pub mod windows;
pub mod linux;

use crate::perception::sensor::SystemSensor;
use std::sync::Arc;

pub fn create_platform_sensor() -> Result<Arc<dyn SystemSensor>, AlephError> {
    #[cfg(target_os = "macos")]
    return Ok(Arc::new(macos::MacosSensor::new()?));

    #[cfg(target_os = "windows")]
    return Ok(Arc::new(windows::WindowsSensor::new()?));

    #[cfg(target_os = "linux")]
    return Ok(Arc::new(linux::LinuxSensor::new()?));

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    Err(AlephError::not_supported("Platform not supported"))
}
```

**Step 4: Commit**

```bash
git add core/src/perception/sensors/{windows,linux,mod}.rs
git commit -m "feat(perception): add Windows/Linux sensor skeletons

- Add WindowsSensor skeleton (returns NotSupported)
- Add LinuxSensor skeleton with headless detection
- Add sensor factory for platform selection
- Enables graceful degradation on unsupported platforms"
```

---

### Task 4: Permission Health Check System ⭐ Priority 4

**Goal**: Implement permission checking and user guidance.

**Step 1: Implement macOS health check**

```rust
// core/src/perception/health.rs

#[cfg(target_os = "macos")]
impl PerceptionHealth {
    async fn check_macos() -> Self {
        let ax_enabled = check_ax_permission();
        let screen_recording = check_screen_recording_permission();
        let input_monitoring = check_input_monitoring_permission();

        let mut recommendations = Vec::new();
        if !ax_enabled {
            recommendations.push(
                "Enable Accessibility: System Settings → Privacy & Security → Accessibility → Add Aleph".to_string()
            );
        }
        if !screen_recording {
            recommendations.push(
                "Enable Screen Recording: System Settings → Privacy & Security → Screen Recording → Add Aleph".to_string()
            );
        }

        Self {
            accessibility_enabled: ax_enabled,
            screen_recording_enabled: screen_recording,
            input_monitoring_enabled: input_monitoring,
            platform_support: if ax_enabled && screen_recording {
                PlatformSupport::Full
            } else if ax_enabled || screen_recording {
                PlatformSupport::Partial
            } else {
                PlatformSupport::Degraded
            },
            available_sensors: vec!["MacosSensor".to_string()],
            recommendations,
        }
    }
}

#[cfg(target_os = "macos")]
fn check_ax_permission() -> bool {
    // Use existing AX permission check
    use core_foundation::base::TCFType;
    use core_graphics::access::AXIsProcessTrusted;
    AXIsProcessTrusted()
}

#[cfg(target_os = "macos")]
fn check_screen_recording_permission() -> bool {
    // Check if we can capture screen
    // This is tricky - might need to attempt capture
    true // Placeholder
}

#[cfg(target_os = "macos")]
fn check_input_monitoring_permission() -> bool {
    // Check if we can simulate input
    true // Placeholder
}
```

**Step 2: Add RPC method for health check**

```rust
// core/src/gateway/handlers/perception.rs

pub async fn handle_perception_health(
    _params: serde_json::Value,
    _ctx: Arc<HandlerContext>,
) -> Result<serde_json::Value, AlephError> {
    let health = PerceptionHealth::check().await;
    Ok(serde_json::to_value(health)?)
}
```

**Step 3: Register RPC method**

```rust
// core/src/gateway/router.rs

router.register("perception.health", handle_perception_health);
```

**Step 4: Write tests**

```rust
// core/tests/perception_health.rs

#[tokio::test]
async fn test_health_check_returns_valid_data() {
    let health = PerceptionHealth::check().await;
    assert!(matches!(
        health.platform_support,
        PlatformSupport::Full
            | PlatformSupport::Partial
            | PlatformSupport::Degraded
            | PlatformSupport::None
    ));
}
```

**Step 5: Commit**

```bash
git add core/src/perception/health.rs
git add core/src/gateway/handlers/perception.rs
git commit -m "feat(perception): add permission health check system

- Implement macOS permission checking (AX, Screen Recording)
- Add user-friendly recommendations for missing permissions
- Add RPC method perception.health
- Enable clients to guide users through permission setup"
```

---

### Task 5: Integration with SystemStateBus ⭐ Priority 5

**Goal**: Update SystemStateBus to use the new PAL.

**Step 1: Update SystemStateBus constructor**

```rust
// core/src/perception/state_bus/mod.rs

impl SystemStateBus {
    pub fn new_with_platform_sensor() -> Result<Self, AlephError> {
        let sensor = crate::perception::sensors::create_platform_sensor()?;
        Self::new(sensor)
    }

    pub fn new(sensor: Arc<dyn SystemSensor>) -> Self {
        Self {
            sensor,
            event_bus: Arc::new(RwLock::new(EventBus::new())),
            state_cache: Arc::new(RwLock::new(HashMap::new())),
            state_history: Arc::new(RwLock::new(StateHistory::new())),
            privacy_filter: Arc::new(PrivacyFilter::new()),
            connector_registry: Arc::new(ConnectorRegistry::new()),
        }
    }
}
```

**Step 2: Update initialization in main**

```rust
// core/src/bin/aleph-server.rs

let ssb = SystemStateBus::new_with_platform_sensor()?;
```

**Step 3: Add health check on startup**

```rust
// core/src/bin/aleph-server.rs

async fn check_perception_health() {
    let health = PerceptionHealth::check().await;
    match health.platform_support {
        PlatformSupport::Full => {
            info!("Perception system: Full support");
        }
        PlatformSupport::Partial => {
            warn!("Perception system: Partial support");
            for rec in &health.recommendations {
                warn!("  - {}", rec);
            }
        }
        PlatformSupport::Degraded | PlatformSupport::None => {
            error!("Perception system: Limited or no support");
            for rec in &health.recommendations {
                error!("  - {}", rec);
            }
        }
    }
}
```

**Step 4: Commit**

```bash
git add core/src/perception/state_bus/mod.rs
git add core/src/bin/aleph-server.rs
git commit -m "feat(perception): integrate PAL with SystemStateBus

- Update SystemStateBus to use platform sensor
- Add health check on server startup
- Log permission issues and recommendations"
```

---

## Testing Strategy

### Unit Tests
- Trait object safety
- Platform detection
- Permission checking (mocked)

### Integration Tests
- macOS sensor creation and basic operations
- Health check on all platforms
- Graceful degradation on unsupported platforms

### Manual Testing
- Run on macOS with/without permissions
- Run on Linux (headless and desktop)
- Verify error messages are helpful

## Success Criteria

- [ ] SystemSensor trait compiles and is object-safe
- [ ] MacosSensor implements full functionality
- [ ] Windows/Linux sensors return NotSupported gracefully
- [ ] Health check provides actionable recommendations
- [ ] SystemStateBus works with platform sensor
- [ ] All tests pass on macOS
- [ ] Server starts successfully on all platforms

## Future Work (Phase 7)

- Full Windows UI Automation implementation
- Full Linux AT-SPI implementation
- Cross-platform input simulation (enigo integration)
- Local OCR integration (tesseract-rs)
- Cloud vision integration (Set-of-Mark)

## Timeline Estimate

- Task 1 (PAL): 4-6 hours
- Task 2 (macOS): 6-8 hours
- Task 3 (Skeletons): 2-3 hours
- Task 4 (Health Check): 3-4 hours
- Task 5 (Integration): 2-3 hours

**Total**: 17-24 hours (2-3 days)

## Dependencies

- Existing `ax_observer` code (Phase 1-5)
- Existing screenshot capture code
- `async-trait` crate
- `image` crate

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| AX API complexity | High | Reuse existing code, simplify for Phase 6 |
| Permission checking unreliable | Medium | Provide clear error messages, manual verification |
| Cross-platform trait design issues | High | Start with simple trait, iterate based on feedback |
| Integration breaks existing code | High | Comprehensive testing, gradual rollout |
