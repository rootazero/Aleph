# System State Bus (SSB) Architecture Design

**Date**: 2026-02-11
**Status**: Design
**Authors**: Architecture Team

## Executive Summary

System State Bus (SSB) 是 Aleph 从"被动工具调用"转向"主动环境感知"的关键架构演进。当前的 `snapshot_capture` 工具解决了"看"的问题，但缺乏"流"的连续性和"深"的上下文。SSB 将作为 Aleph 的"感知神经系统"，实现：

- **实时状态流**：从静态快照到连续事件流
- **跨应用上下文**：统一的应用状态抽象层
- **模拟交互闭环**：从感知到行动的完整回路
- **隐私优先**：内置数据脱敏和审计机制

## Motivation

### Current Limitations

1. **记忆断层**：Agent 在长流程任务中无法持续感知环境变化
2. **高延迟**：每次需要状态时都要重新截屏和 OCR（200-500ms）
3. **无法响应事件**：无法实现"当邮件到达时自动处理"等主动行为
4. **坐标漂移**：截屏时的坐标在执行点击时可能已失效

### Design Goals

1. **低延迟**：事件驱动，< 10ms 端到端延迟
2. **低开销**：CPU < 2%，内存 < 50MB（30 秒历史）
3. **易调试**：浏览器控制台可直接监控状态流
4. **隐私安全**：密码字段自动脱敏，审计日志完整
5. **渐进增强**：优雅降级到 OCR，支持所有应用

## Architecture Overview

### Core Components

```
┌─────────────────────────────────────────────────────────────────┐
│                    Gateway (18789)                              │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │         GatewayEventBus (Topic: system.state.*)          │   │
│  └────────────────────────┬─────────────────────────────────┘   │
│                           │                                      │
│  ┌────────────────────────┴─────────────────────────────────┐   │
│  │              SystemStateBus                              │   │
│  │  ┌────────────┐  ┌──────────────┐  ┌─────────────────┐  │   │
│  │  │ StateCache │  │ StateHistory │  │ PrivacyFilter   │  │   │
│  │  │ (HashMap)  │  │ (I/P-Frame)  │  │ (Middleware)    │  │   │
│  │  └────────────┘  └──────────────┘  └─────────────────┘  │   │
│  └──────────────────────┬───────────────────────────────────┘   │
│                         │                                        │
│  ┌──────────────────────┴───────────────────────────────────┐   │
│  │           ConnectorRegistry (智能选择)                    │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐   │   │
│  │  │AxConnector   │  │PluginConn    │  │VisionConn    │   │   │
│  │  │(事件驱动)     │  │(DOM/IDE API) │  │(OCR 轮询)    │   │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘   │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │           ActionDispatcher (模拟交互)                     │   │
│  │  - ID → 坐标映射  - 前置验证  - 后置验证                  │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
         ▲                    ▲                    ▲
         │                    │                    │
    AX API (事件)      Browser Plugin      OCR Engine (轮询)
```

### Key Design Decisions

#### 1. Transport Layer: WebSocket + JSON

**Decision**: Use WebSocket (port 18789) with JSON-RPC 2.0 format, enhanced with JSON Patch (RFC 6902) for incremental updates.

**Rationale**:
- **Bottleneck Analysis**: AX API (10-100ms) and OCR (200-500ms) dominate latency; transport layer (UDS ~10μs vs WebSocket ~1ms) is negligible
- **Ecosystem Integration**: Reuse existing Gateway infrastructure
- **Developer Experience**: Browser console can directly monitor state flow
- **Future-Proof**: Can upgrade to MessagePack without protocol changes

**Alternatives Considered**:
- Unix Domain Socket + Protobuf: Higher performance but poor debuggability
- Hybrid (UDS core + WebSocket adapter): Added complexity without clear benefit

#### 2. Event-Driven Architecture

**Decision**: Use macOS Accessibility API notifications as primary trigger, not polling/diffing.

**Rationale**:
- **Zero CPU Overhead**: Only process events when UI actually changes
- **O(1) Complexity**: Extract changes directly from AX notifications, no tree diffing needed
- **Real-time**: < 10ms latency from UI change to event publication

**Implementation**:
```rust
// AX notifications trigger incremental updates
match ax_notification {
    AXValueChanged(element_id) => {
        let patch = json!([{
            "op": "replace",
            "path": format!("/elements/{}/current_value", element_id),
            "value": new_value
        }]);
        bus.publish(TopicEvent::new(
            format!("system.state.{}.delta", app_id),
            patch
        ));
    }
}
```

## Cross-Platform Architecture

### Architectural Positioning: Worker-in-Situ

**Core Principle**: Aleph Server runs in the user's working environment, not as a remote cloud service.

- **Client Role**: Pure UI window for conversation (text input/output, file attachments)
- **Server Role**: All business logic, perception, and execution
- **Deployment Model**: Server runs where the user works (macOS Desktop / Linux Desktop / Windows Desktop)

**Implications**:
- Server must have cross-platform perception capabilities
- Server must handle platform-specific APIs (Accessibility, Screen Capture, Input Simulation)
- Client never processes business logic or tool execution

### Platform Abstraction Layer (PAL)

To elegantly handle cross-platform differences in Rust, we define a **SystemSensor** trait:

```rust
// core/src/perception/sensor.rs

#[async_trait]
pub trait SystemSensor: Send + Sync {
    /// Get currently focused application ID (bundle ID / process name)
    async fn get_focused_app(&self) -> Result<String>;

    /// Capture UI tree (AX Tree / UIA Tree / AT-SPI)
    async fn capture_ui_tree(&self, app_id: &str) -> Result<UINodeTree>;

    /// Visual fallback: capture screenshot
    async fn capture_screenshot(&self) -> Result<DynamicImage>;

    /// Check if sensor is available in current environment
    fn is_available(&self) -> bool;

    /// Get sensor capabilities
    fn capabilities(&self) -> SensorCapabilities;
}

/// Platform-specific implementations
pub struct MacosSensor {
    ax_observer: AxObserver,
    screen_capture: ScreenCaptureKit,
}

pub struct WindowsSensor {
    uia_client: UIAutomationClient,
    gdi_capture: GdiScreenCapture,
}

pub struct LinuxSensor {
    atspi_client: AtSpiClient,  // X11/Wayland
    x11_capture: X11ScreenCapture,
}
```

### Tiered Perception Strategy

**Core Design**: Graceful degradation from structured APIs to visual recognition.

```
┌─────────────────────────────────────────────────────────────┐
│  Level 1: Structured API (Accessibility)                    │
│  ├─ macOS: Accessibility API (AXUIElement)                  │
│  ├─ Windows: UI Automation API (IUIAutomation)              │
│  ├─ Linux: AT-SPI (Assistive Technology Service Provider)   │
│  └─ Precision: High | Cost: Low | Coverage: 80%             │
└─────────────────────────────────────────────────────────────┘
                          ↓ Fallback
┌─────────────────────────────────────────────────────────────┐
│  Level 2: Local Vision (Screenshot + OCR)                   │
│  ├─ Screenshot: Platform-specific capture                   │
│  ├─ OCR: tesseract-rs / ocrs (local)                        │
│  ├─ Element Detection: Local vision model (optional)        │
│  └─ Precision: Medium | Cost: Medium | Coverage: 95%        │
└─────────────────────────────────────────────────────────────┘
                          ↓ Fallback
┌─────────────────────────────────────────────────────────────┐
│  Level 3: Cloud Vision (Multimodal API)                     │
│  ├─ Providers: GPT-4o / Claude 3.5 / Gemini                 │
│  ├─ Set-of-Mark: Visual prompting with numbered anchors     │
│  ├─ Privacy: Local pre-redaction before upload              │
│  └─ Precision: Highest | Cost: High | Coverage: 100%        │
└─────────────────────────────────────────────────────────────┘
```

**Selection Logic**:
```rust
impl SystemStateBus {
    async fn sense_ui(&self, app_id: &str) -> Result<AppState> {
        // Level 1: Try structured API first
        if let Ok(state) = self.sensor.capture_ui_tree(app_id).await {
            return Ok(state);
        }

        // Level 2: Fallback to local vision
        if self.config.enable_local_vision {
            let screenshot = self.sensor.capture_screenshot().await?;
            if let Ok(state) = self.local_vision.analyze(screenshot).await {
                return Ok(state);
            }
        }

        // Level 3: Fallback to cloud vision (if enabled)
        if self.config.enable_cloud_vision {
            let screenshot = self.sensor.capture_screenshot().await?;
            let state = self.cloud_vision.analyze(screenshot).await?;
            return Ok(state);
        }

        Err(AlephError::perception("No available perception method"))
    }
}
```

### Platform-Specific Implementations

#### macOS: Accessibility API

**Status**: Primary implementation target (Phase 6)

**Key APIs**:
- `AXUIElementCreateApplication`: Get app's root element
- `AXObserverCreate`: Register for UI change notifications
- `AXUIElementCopyAttributeValue`: Extract element properties

**Challenges**:
- Requires Screen Recording permission
- CFRunLoop thread isolation required
- Some apps (Electron) have incomplete AX trees

#### Windows: UI Automation

**Status**: Planned (Phase 7)

**Key APIs**:
- `IUIAutomation`: Main automation interface
- `IUIAutomationElement`: Element inspection
- `IUIAutomationTreeWalker`: Tree traversal

**Challenges**:
- COM initialization required
- Some legacy apps don't support UIA

#### Linux: AT-SPI

**Status**: Planned (Phase 7)

**Key APIs**:
- `atspi_get_desktop`: Get desktop root
- `atspi_accessible_get_child_at_index`: Tree traversal
- `atspi_event_listener_register`: Event notifications

**Challenges**:
- X11 vs Wayland differences
- Inconsistent support across desktop environments

### Cross-Platform Input Simulation

**Requirement**: Server must simulate mouse/keyboard input on the platform it runs on.

**Implementation Strategy**:
```rust
// core/src/perception/actuator.rs

#[async_trait]
pub trait InputActuator: Send + Sync {
    async fn click(&self, x: i32, y: i32) -> Result<()>;
    async fn type_text(&self, text: &str) -> Result<()>;
    async fn press_key(&self, key: Key) -> Result<()>;
}

// Platform implementations
pub struct MacosActuator;  // CGEventCreateMouseEvent
pub struct WindowsActuator; // SendInput
pub struct LinuxActuator;   // XTest / libei
```

**Recommended Libraries**:
- `enigo`: Cross-platform input simulation (consider forking for SSB integration)
- `autopilot-rs`: Alternative with better coordinate handling

### Permission Management and Health Check

**Critical Requirement**: Server needs elevated permissions on all platforms.

**Health Check Module**:
```rust
// core/src/perception/health.rs

pub struct PerceptionHealth {
    pub accessibility_enabled: bool,
    pub screen_recording_enabled: bool,
    pub input_monitoring_enabled: bool,
    pub platform_support: PlatformSupport,
}

impl PerceptionHealth {
    pub async fn check() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                accessibility_enabled: check_ax_permission(),
                screen_recording_enabled: check_screen_recording(),
                input_monitoring_enabled: check_input_monitoring(),
                platform_support: PlatformSupport::Full,
            }
        }

        #[cfg(target_os = "linux")]
        {
            Self {
                accessibility_enabled: check_atspi_available(),
                screen_recording_enabled: check_x11_capture(),
                input_monitoring_enabled: check_xtest(),
                platform_support: if is_wayland() {
                    PlatformSupport::Partial
                } else {
                    PlatformSupport::Full
                },
            }
        }

        #[cfg(target_os = "windows")]
        {
            Self {
                accessibility_enabled: check_uia_available(),
                screen_recording_enabled: true, // Always available
                input_monitoring_enabled: check_admin_rights(),
                platform_support: PlatformSupport::Full,
            }
        }
    }
}
```

**User Guidance**: When permissions are missing, provide platform-specific instructions:
- macOS: "Open System Settings → Privacy & Security → Accessibility"
- Windows: "Run as Administrator or grant UIAccess"
- Linux: "Install at-spi2-core package"

### Graceful Degradation for Headless Environments

**Scenario**: Server deployed on headless Linux server (no GUI).

**Strategy**: SSB modules gracefully degrade, returning `NotSupported` errors.

```rust
impl SystemSensor for LinuxSensor {
    async fn capture_ui_tree(&self, app_id: &str) -> Result<UINodeTree> {
        if !self.has_display() {
            return Err(AlephError::not_supported(
                "UI sensing requires display server (X11/Wayland)"
            ));
        }
        // ... normal implementation
    }
}
```

**Agent Behavior**: When SSB returns `NotSupported`, Agent knows:
- "I cannot see the screen"
- "I can only operate files and APIs"
- "I should suggest user run Server on desktop environment"

### Cloud Vision Integration Strategy

**Use Case**: When structured APIs fail (Canvas apps, games, custom-drawn UIs).

**Architecture**: Structured Semantic Anchors (Skeleton & Skin)

```rust
// core/src/perception/cloud_vision.rs

pub struct CloudVisionProvider {
    client: reqwest::Client,
    api_key: String,
}

impl CloudVisionProvider {
    pub async fn analyze_with_context(
        &self,
        screenshot: DynamicImage,
        ax_context: SimplifiedAxTree,
    ) -> Result<UIUnderstanding> {
        // 1. Draw Set-of-Mark annotations on screenshot
        let annotated = self.draw_som_markers(&screenshot, &ax_context)?;

        // 2. Build multimodal prompt with AX context
        let prompt = format!(
            "The screenshot shows an application UI. \
             I've marked {} interactive elements with numbers. \
             Element details: {:?}. \
             Please identify what each numbered element does.",
            ax_context.elements.len(),
            ax_context.elements
        );

        // 3. Call cloud vision API
        let response = self.call_vision_api(annotated, prompt).await?;

        Ok(response)
    }

    fn draw_som_markers(
        &self,
        screenshot: &DynamicImage,
        ax_context: &SimplifiedAxTree,
    ) -> Result<DynamicImage> {
        let mut img = screenshot.clone();
        for (idx, element) in ax_context.elements.iter().enumerate() {
            // Draw small numbered circle at element position
            draw_text_mut(
                &mut img,
                Rgba([255, 0, 0, 255]),
                element.rect.x,
                element.rect.y,
                Scale::uniform(16.0),
                &self.font,
                &format!("{}", idx + 1),
            );
        }
        Ok(img)
    }
}
```

**Optimization Strategies**:

1. **Visual Debouncing**: Only call cloud API when AX tree hash changes significantly
2. **Region Cropping**: Send only the relevant window, not full 4K screenshot
3. **Local Pre-Redaction**: Blur password fields and sensitive areas before upload
4. **Caching**: Cache cloud responses for identical UI states

**Privacy Safeguards**:
```rust
impl CloudVisionProvider {
    fn redact_sensitive_regions(
        &self,
        screenshot: &mut DynamicImage,
        ax_context: &SimplifiedAxTree,
    ) {
        for element in &ax_context.elements {
            if element.role == "password" || element.is_secure {
                // Apply Gaussian blur to sensitive region
                blur_region(screenshot, element.rect, 20.0);
            }
        }
    }
}
```

### Performance Considerations

**On-Demand Sensing**: Don't run OCR continuously in background.

```rust
impl SystemStateBus {
    pub async fn sense_on_demand(&self, trigger: SenseTrigger) -> Result<()> {
        match trigger {
            SenseTrigger::AgentQuery => {
                // Agent asked "what's on screen?"
                self.perform_full_sense().await?;
            }
            SenseTrigger::AxTreeChange => {
                // Lightweight: only update changed elements
                self.perform_incremental_sense().await?;
            }
            SenseTrigger::Idle => {
                // Do nothing, save CPU
            }
        }
        Ok(())
    }
}
```

**CPU Budget**: Target < 2% CPU usage in idle state, < 10% during active sensing.

### Summary: Cross-Platform Execution Model

| Component | macOS | Windows | Linux | Headless |
|-----------|-------|---------|-------|----------|
| **Structured API** | AX API | UI Automation | AT-SPI | ❌ |
| **Screenshot** | ScreenCaptureKit | GDI+ | X11/Wayland | ❌ |
| **Local OCR** | ✅ | ✅ | ✅ | ✅ (if image provided) |
| **Cloud Vision** | ✅ | ✅ | ✅ | ✅ (if image provided) |
| **Input Simulation** | CGEvent | SendInput | XTest | ❌ |
| **Graceful Degradation** | N/A | N/A | ✅ | ✅ |

**Key Takeaway**: SSB is designed to work best on desktop environments with GUI, but gracefully degrades in headless scenarios by returning `NotSupported` errors and allowing Agent to adapt its behavior.

## Protocol Design

### Topic Naming Convention

SSB events use hierarchical topic structure for efficient filtering:

```
system.state.{app_id}.{event_type}

Examples:
- system.state.com.apple.mail.delta          # Mail app incremental update
- system.state.com.notion.Notion.snapshot    # Notion full snapshot
- system.state.global.focus_changed          # Global focus change
```

### Message Format

#### Event Envelope

```json
{
  "header": {
    "v": 1,
    "msg_id": "uuid-12345",
    "timestamp": 1739268000,
    "app_id": "com.notion.Notion",
    "window_id": "win_001",
    "event_type": "STATE_DELTA"
  },
  "payload": { ... }
}
```

**Event Types**:
- `FULL_SNAPSHOT`: Complete state (sent on subscription or major change)
- `STATE_DELTA`: Incremental update (JSON Patch format)
- `USER_ACTION`: User interaction event (click, type, scroll)
- `NOTIFICATION`: System notification or alert

#### Payload Structure

**Semantic Layer** (flattened AX tree):
```json
{
  "elements": [
    {
      "id": "btn_send_001",
      "role": "button",
      "label": "发送消息",
      "value": null,
      "rect": {"x": 100, "y": 200, "w": 50, "h": 20},
      "state": {"focused": false, "enabled": true},
      "source": "ax"
    },
    {
      "id": "input_chat_001",
      "role": "textarea",
      "placeholder": "输入消息...",
      "current_value": "你好，Aleph",
      "selection": [0, 5],
      "source": "ax"
    }
  ],
  "app_context": {
    "url": "https://notion.so/page-123",
    "page_id": "abc-def-ghi",
    "unread_count": 3
  }
}
```

**Delta Format** (JSON Patch RFC 6902):
```json
[
  {
    "op": "replace",
    "path": "/elements/input_chat_001/current_value",
    "value": "你好，Aleph！"
  },
  {
    "op": "add",
    "path": "/elements/btn_new_001",
    "value": {
      "id": "btn_new_001",
      "role": "button",
      "label": "新按钮"
    }
  }
]
```

### RPC Methods

#### Subscribe to State Stream

```json
{
  "jsonrpc": "2.0",
  "method": "system.state.subscribe",
  "params": {
    "patterns": [
      "system.state.com.apple.mail.*",
      "system.state.*.focus_changed"
    ],
    "include_snapshot": true,
    "debounce_ms": 100
  },
  "id": 1
}
```

**Response**:
```json
{
  "jsonrpc": "2.0",
  "result": {
    "subscription_id": "sub_123",
    "active_patterns": ["system.state.com.apple.mail.*"],
    "initial_snapshot": { ... }
  },
  "id": 1
}
```

#### Unsubscribe

```json
{
  "jsonrpc": "2.0",
  "method": "system.state.unsubscribe",
  "params": {
    "subscription_id": "sub_123"
  },
  "id": 2
}
```

#### Query Historical State

```json
{
  "jsonrpc": "2.0",
  "method": "system.state.query",
  "params": {
    "app_id": "com.apple.mail",
    "timestamp": 1739267950000,
    "max_age_secs": 30
  },
  "id": 3
}
```

## Detailed Design

### 1. ID Stability Strategy

**Problem**: Path-based IDs (e.g., `Window/VStack[0]/Button[2]`) break when UI structure changes.

**Solution**: Three-level fallback strategy with semantic hashing.

```rust
// core/src/perception/element_id.rs

pub struct StableElementId {
    primary: String,      // AXIdentifier or semantic hash
    fallback: String,     // Path hash
    version: u32,         // ID generation strategy version
}

impl StableElementId {
    pub fn generate(element: &AXElement) -> Self {
        // Level 1: AXIdentifier (most stable, developer-set)
        if let Some(id) = element.ax_identifier() {
            return Self {
                primary: format!("ax_id:{}", id),
                fallback: Self::path_hash(element),
                version: 1,
            };
        }

        // Level 2: Semantic Hash (role + label + relative position)
        if let Some(label) = element.label() {
            let semantic = format!(
                "{}:{}:{}",
                element.role(),
                label,
                element.relative_position()  // Relative to parent, not absolute index
            );
            return Self {
                primary: format!("sem:{}", hash(&semantic)),
                fallback: Self::path_hash(element),
                version: 2,
            };
        }

        // Level 3: Path Hash (least stable, always available)
        Self {
            primary: Self::path_hash(element),
            fallback: String::new(),
            version: 3,
        }
    }

    fn path_hash(element: &AXElement) -> String {
        let path = element.get_path_without_indices();  // e.g., "Window/VStack/Button"
        format!("path:{}", hash(&path))
    }

    // Resolve ID to element, trying fallback if primary fails
    pub fn resolve(&self, state_cache: &StateCache) -> Option<&Element> {
        state_cache.get(&self.primary)
            .or_else(|| state_cache.get(&self.fallback))
    }
}
```

**Benefits**:
- UI insertions/deletions don't break IDs
- Graceful degradation when semantic info unavailable
- Version field allows future ID strategy upgrades

### 2. Memory Optimization: I-Frame + P-Frame

**Problem**: Storing 30 seconds of full snapshots at 10Hz = 300 × 2MB = 600MB memory.

**Solution**: Video encoding strategy - Keyframes (I-Frame) + Deltas (P-Frame).

```rust
// core/src/perception/state_history.rs

pub struct StateHistory {
    i_frames: VecDeque<IFrame>,      // Full snapshots, every 5 seconds
    p_frames: VecDeque<PFrame>,      // Incremental patches
    max_duration_secs: u64,          // Default 30 seconds
}

struct IFrame {
    timestamp: u64,
    state: AppState,  // Complete state
}

struct PFrame {
    timestamp: u64,
    patches: Vec<JsonPatch>,  // RFC 6902 format
    base_iframe_ts: u64,      // Points to nearest I-Frame
}

impl StateHistory {
    // Query state at any point in time (key method)
    pub fn query(&self, target_ts: u64) -> Option<AppState> {
        // 1. Find nearest I-Frame (binary search)
        let iframe = self.i_frames
            .iter()
            .rev()
            .find(|f| f.timestamp <= target_ts)?;

        // 2. Clone base state
        let mut state = iframe.state.clone();

        // 3. Replay all subsequent P-Frames
        for pframe in self.p_frames.iter() {
            if pframe.timestamp > iframe.timestamp && pframe.timestamp <= target_ts {
                for patch in &pframe.patches {
                    state.apply_patch(patch);  // Apply JSON Patch
                }
            }
        }

        Some(state)
    }

    // Memory usage estimation
    pub fn memory_usage(&self) -> usize {
        let iframe_size = self.i_frames.len() * 2_000_000;  // Assume 2MB each
        let pframe_size = self.p_frames.len() * 500;        // Assume 500B each
        iframe_size + pframe_size
        // 30 seconds: 6 I-Frames (12MB) + 300 P-Frames (150KB) ≈ 12MB
    }
}
```

**Memory Comparison**:
- Naive approach: 30s × 10Hz × 2MB = **600MB**
- I/P-Frame: 6 × 2MB + 300 × 500B = **12MB** (98% reduction)

### 3. Action Dispatcher: Simulation Closed-Loop

**Problem**: Coordinates from snapshots become stale; no verification of action success.

**Solution**: Real-time coordinate mapping + pre/post validation.

```rust
// core/src/perception/action_dispatcher.rs

pub struct ActionDispatcher {
    state_bus: Arc<SystemStateBus>,
    executor: Arc<SimulationExecutor>,
}

#[derive(Deserialize)]
pub struct ActionRequest {
    target_id: String,
    method: ActionMethod,
    expect: ExpectCondition,
}

#[derive(Deserialize)]
pub enum ActionMethod {
    Click,
    Type { text: String },
    Scroll { delta: i32 },
}

#[derive(Deserialize)]
pub struct ExpectCondition {
    condition: ConditionType,
    timeout_ms: u64,
}

#[derive(Deserialize)]
pub enum ConditionType {
    ElementDisappear,
    ValueChanged { expected: String },
    StateChanged { key: String, value: Value },
}

impl ActionDispatcher {
    // RPC method: system.action.execute
    pub async fn execute(&self, action: ActionRequest) -> Result<ActionResult> {
        // 1. Get real-time coordinates from StateCache
        let element = self.state_bus.state_cache
            .read()
            .await
            .get(&action.target_id)
            .ok_or("Element not found")?;

        let rect = element.rect.ok_or("Element has no rect")?;

        // 2. Pre-action validation (critical: avoid blind actions)
        if !element.state.enabled {
            return Err("Element is disabled".into());
        }

        // 3. Execute simulation
        match action.method {
            ActionMethod::Click => {
                let center = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
                self.executor.click(center).await?;
            }
            ActionMethod::Type { text } => {
                self.executor.focus(rect).await?;
                self.executor.type_text(&text).await?;
            }
            ActionMethod::Scroll { delta } => {
                self.executor.scroll(rect, delta).await?;
            }
        }

        // 4. Post-action validation (closed-loop key)
        tokio::time::sleep(Duration::from_millis(action.expect.timeout_ms)).await;

        let new_state = self.state_bus.state_cache.read().await;
        match action.expect.condition {
            ConditionType::ElementDisappear => {
                if new_state.contains_key(&action.target_id) {
                    return Err("Element still exists after action".into());
                }
            }
            ConditionType::ValueChanged { expected } => {
                let actual = new_state.get(&action.target_id)
                    .and_then(|e| e.current_value.as_ref());
                if actual != Some(&expected) {
                    return Err(format!("Expected {:?}, got {:?}", expected, actual).into());
                }
            }
            ConditionType::StateChanged { key, value } => {
                let actual = new_state.get(&action.target_id)
                    .and_then(|e| e.state.get(&key));
                if actual != Some(&value) {
                    return Err(format!("State mismatch for key {}", key).into());
                }
            }
        }

        Ok(ActionResult { success: true })
    }
}
```

**RPC Example**:
```json
{
  "jsonrpc": "2.0",
  "method": "system.action.execute",
  "params": {
    "target_id": "btn_send_001",
    "method": "click",
    "expect": {
      "condition": "element_disappear",
      "timeout_ms": 500
    }
  },
  "id": 4
}
```

### 4. Privacy Filter: Middleware

**Problem**: SSB captures sensitive data (passwords, credit cards, private messages).

**Solution**: Mandatory filtering middleware before event publication.

```rust
// core/src/perception/privacy_filter.rs

pub struct PrivacyFilter {
    sensitive_apps: HashSet<String>,  // 1Password, Keychain Access
    sensitive_roles: HashSet<String>, // AXSecureTextField
    patterns: Vec<Regex>,             // Credit card, SSN patterns
}

impl PrivacyFilter {
    pub fn filter(&self, event: &mut StateEvent) {
        // Rule 1: Password fields
        for element in &mut event.elements {
            if element.role == "AXSecureTextField" || element.is_password {
                element.current_value = Some("***".into());
                element.placeholder = None;
            }
        }

        // Rule 2: Sensitive applications (complete blackout)
        if self.sensitive_apps.contains(&event.app_id) {
            event.elements.clear();
            event.errors.push("PRIVACY_FILTERED".into());
            return;
        }

        // Rule 3: Pattern matching (credit cards, SSNs)
        for element in &mut event.elements {
            if let Some(ref mut value) = element.current_value {
                if Self::looks_like_credit_card(value) {
                    *value = "****-****-****-****".into();
                }
                if Self::looks_like_ssn(value) {
                    *value = "***-**-****".into();
                }
            }
        }

        // Rule 4: Audit logging (for compliance)
        if event.elements.iter().any(|e| e.current_value.as_ref().map_or(false, |v| v.contains("***"))) {
            self.log_filtered_event(&event);
        }
    }

    fn looks_like_credit_card(s: &str) -> bool {
        // Luhn algorithm + format check
        let digits: String = s.chars().filter(|c| c.is_numeric()).collect();
        digits.len() >= 13 && digits.len() <= 19 && Self::luhn_check(&digits)
    }

    fn looks_like_ssn(s: &str) -> bool {
        // XXX-XX-XXXX format
        let re = Regex::new(r"^\d{3}-\d{2}-\d{4}$").unwrap();
        re.is_match(s)
    }
}
```

**Configuration** (`~/.aleph/config.toml`):
```toml
[system_state_bus.privacy]
sensitive_apps = ["com.agilebits.onepassword7", "com.apple.keychainaccess"]
filter_patterns = ["credit_card", "ssn", "phone"]
audit_log_path = "~/.aleph/privacy_audit.log"
```

### 5. RunLoop Isolation: Thread Architecture

**Problem**: macOS AXObserver requires CFRunLoop, conflicts with tokio's multi-threaded runtime.

**Solution**: Dedicated OS thread for AX events, communicate via channel.

```rust
// core/src/perception/ax_observer.rs

pub struct AxObserver {
    event_tx: mpsc::UnboundedSender<AxEvent>,
}

#[derive(Debug, Clone)]
pub enum AxEvent {
    ValueChanged { app_id: String, element_id: String, new_value: Value },
    FocusChanged { app_id: String, from: String, to: String },
    WindowCreated { app_id: String, window_id: String },
    WindowClosed { app_id: String, window_id: String },
}

impl AxObserver {
    pub fn start() -> (Self, mpsc::UnboundedReceiver<AxEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        // Critical: Dedicated OS Thread for CFRunLoop
        std::thread::spawn(move || {
            unsafe {
                // Create AX observer for all running applications
                let observer = AXObserverCreate(
                    pid,
                    ax_callback,
                    &tx_clone as *const _ as *mut _
                );

                // Register for notifications
                AXObserverAddNotification(
                    observer,
                    element,
                    kAXValueChangedNotification,
                    ptr::null()
                );
                AXObserverAddNotification(
                    observer,
                    element,
                    kAXFocusedUIElementChangedNotification,
                    ptr::null()
                );

                // Start RunLoop (blocking)
                CFRunLoopRun();
            }
        });

        (Self { event_tx: tx }, rx)
    }
}

// Callback invoked by macOS (runs on CFRunLoop thread)
extern "C" fn ax_callback(
    observer: AXObserverRef,
    element: AXUIElementRef,
    notification: CFStringRef,
    user_data: *mut c_void,
) {
    let tx = unsafe { &*(user_data as *const mpsc::UnboundedSender<AxEvent>) };

    let event = match notification_to_string(notification).as_str() {
        "AXValueChanged" => {
            let value = get_element_value(element);
            AxEvent::ValueChanged {
                app_id: get_app_id(element),
                element_id: get_element_id(element),
                new_value: value,
            }
        }
        "AXFocusedUIElementChanged" => {
            AxEvent::FocusChanged {
                app_id: get_app_id(element),
                from: get_previous_focus(),
                to: get_element_id(element),
            }
        }
        _ => return,
    };

    let _ = tx.send(event);  // Send to tokio runtime
}
```

**Integration with SystemStateBus**:
```rust
impl SystemStateBus {
    pub async fn start(&self) {
        let (observer, mut rx) = AxObserver::start();

        // Tokio task: consume AX events
        let bus = self.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                bus.handle_ax_event(event).await;
            }
        });
    }

    async fn handle_ax_event(&self, event: AxEvent) {
        // Apply privacy filter
        let mut state_event = self.convert_to_state_event(event);
        self.privacy_filter.filter(&mut state_event);

        // Update state cache
        self.state_cache.write().await.update(&state_event);

        // Create JSON Patch
        let patch = self.create_patch(&state_event);

        // Publish to EventBus
        self.event_bus.publish_json(&TopicEvent::new(
            format!("system.state.{}.delta", state_event.app_id),
            patch
        )).ok();
    }
}
```

### 6. Connector Architecture: Three-Level Fallback

**Problem**: Not all applications support Accessibility API (games, legacy Java apps, remote desktop).

**Solution**: Intelligent connector selection with graceful degradation.

```rust
// core/src/perception/connector_registry.rs

pub trait StateConnector: Send + Sync {
    async fn capture_state(&self, app_id: &str) -> Result<AppState>;
    fn supports_events(&self) -> bool;
    fn confidence(&self) -> f32;  // 0.0-1.0
}

pub struct ConnectorRegistry {
    connectors: HashMap<String, Box<dyn StateConnector>>,
}

impl ConnectorRegistry {
    // Automatically select best connector for application
    pub fn select_connector(&self, app_id: &str) -> &dyn StateConnector {
        // Level 1: Try AX Connector (highest confidence)
        if self.test_ax_support(app_id) {
            return self.connectors.get("ax").unwrap();
        }

        // Level 2: Try Plugin Connector (for browsers, IDEs)
        if app_id.contains("Browser") || app_id.contains("Chrome") {
            if let Some(plugin) = self.connectors.get("browser_plugin") {
                return plugin;
            }
        }

        // Level 3: Fallback to Vision Connector (OCR)
        self.connectors.get("vision").unwrap()
    }

    fn test_ax_support(&self, app_id: &str) -> bool {
        // Try to get AX tree root
        unsafe {
            let app = get_application_by_bundle_id(app_id);
            let mut root: AXUIElementRef = ptr::null_mut();
            let result = AXUIElementCopyAttributeValue(
                app,
                kAXChildrenAttribute,
                &mut root as *mut _ as *mut _
            );
            result == kAXErrorSuccess
        }
    }
}
```

#### Level 1: AX Connector (Event-Driven)

```rust
pub struct AxConnector {
    observer: Arc<AxObserver>,
}

impl StateConnector for AxConnector {
    async fn capture_state(&self, app_id: &str) -> Result<AppState> {
        // Get AX tree
        let root = get_ax_root(app_id)?;
        let elements = self.traverse_ax_tree(root, 0, 1500).await?;

        Ok(AppState {
            app_id: app_id.to_string(),
            elements,
            source: StateSource::Accessibility,
            confidence: 1.0,
        })
    }

    fn supports_events(&self) -> bool {
        true  // AX API provides notifications
    }

    fn confidence(&self) -> f32 {
        1.0  // Highest confidence
    }
}
```

#### Level 2: Plugin Connector (DOM/IDE API)

```rust
pub struct BrowserPluginConnector {
    extension_id: String,
}

impl StateConnector for BrowserPluginConnector {
    async fn capture_state(&self, app_id: &str) -> Result<AppState> {
        // Communicate with browser extension via Native Messaging
        let response = self.send_native_message(json!({
            "method": "getDOM",
            "url": self.get_current_url(app_id)?
        })).await?;

        // Convert DOM to pseudo-AX tree
        let elements = self.dom_to_elements(response)?;

        Ok(AppState {
            app_id: app_id.to_string(),
            elements,
            source: StateSource::Plugin,
            confidence: 0.95,
        })
    }

    fn supports_events(&self) -> bool {
        true  // DOM MutationObserver
    }

    fn confidence(&self) -> f32 {
        0.95
    }
}
```

#### Level 3: Vision Connector (OCR Polling)

```rust
pub struct VisionConnector {
    ocr_engine: Arc<OcrEngine>,
    polling_interval: Duration,
    active_apps: Arc<RwLock<HashSet<String>>>,
}

impl StateConnector for VisionConnector {
    async fn capture_state(&self, app_id: &str) -> Result<AppState> {
        // 1. Capture window screenshot
        let screenshot = capture_window(app_id).await?;

        // 2. Run OCR
        let ocr_blocks = self.ocr_engine.recognize(&screenshot).await?;

        // 3. Build pseudo-AX tree
        let mut elements = Vec::new();
        for block in ocr_blocks {
            elements.push(Element {
                id: format!("ocr_{}", block.block_id),
                role: "StaticText".into(),
                label: Some(block.text),
                rect: Some(block.bbox),
                source: ElementSource::Ocr,
                confidence: block.confidence,
                ..Default::default()
            });
        }

        // 4. Heuristic detection of interactive elements
        self.detect_interactive_elements(&screenshot, &mut elements).await?;

        Ok(AppState {
            app_id: app_id.to_string(),
            elements,
            source: StateSource::Vision,
            confidence: 0.7,
        })
    }

    fn supports_events(&self) -> bool {
        false  // Polling only
    }

    fn confidence(&self) -> f32 {
        0.7  // Lower confidence due to OCR errors
    }
}

impl VisionConnector {
    // Detect clickable regions using computer vision
    async fn detect_interactive_elements(
        &self,
        screenshot: &Image,
        elements: &mut Vec<Element>,
    ) -> Result<()> {
        // Use CV algorithms to detect:
        // 1. Rectangular borders (likely buttons)
        // 2. Cursor shape change regions (clickable)
        // 3. Text input field features (blinking cursor)

        let interactive_regions = self.detect_clickable_regions(screenshot).await?;

        for region in interactive_regions {
            elements.push(Element {
                id: format!("vision_{}", uuid::Uuid::new_v4()),
                role: "Button".into(),  // Inferred
                rect: Some(region.bbox),
                source: ElementSource::Vision,
                confidence: region.confidence,
                ..Default::default()
            });
        }

        Ok(())
    }
}
```

**Smart Polling Strategy**:
```rust
impl SystemStateBus {
    async fn start_vision_polling(&self) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));

            loop {
                interval.tick().await;

                // Only poll subscribed apps without AX support
                let apps_to_poll = self.get_vision_only_apps().await;

                for app_id in apps_to_poll {
                    // Skip if user is typing (avoid interference)
                    if self.is_user_typing(&app_id).await {
                        continue;
                    }

                    match self.vision_connector.capture_state(&app_id).await {
                        Ok(state) => {
                            // Diff with previous state (only here we need diffing)
                            let patches = self.diff_states(&app_id, &state).await;
                            if !patches.is_empty() {
                                self.publish_delta(&app_id, patches).await;
                            }
                        }
                        Err(e) => warn!("Vision capture failed for {}: {}", app_id, e),
                    }
                }
            }
        });
    }
}
```

## Implementation Plan

### Phase 1: Core Infrastructure (Week 1-2)

**Goal**: Establish SSB foundation with AX Connector.

**Tasks**:
1. Create `core/src/perception/state_bus/` module structure
2. Implement `SystemStateBus` with `StateCache` (HashMap)
3. Integrate with existing `GatewayEventBus` (topic: `system.state.*`)
4. Implement `AxObserver` with CFRunLoop thread isolation
5. Add RPC methods: `system.state.subscribe`, `system.state.unsubscribe`
6. Basic testing with Mail.app and Finder

**Deliverables**:
- [ ] `core/src/perception/state_bus/mod.rs`
- [ ] `core/src/perception/state_bus/ax_observer.rs`
- [ ] `core/src/perception/state_bus/state_cache.rs`
- [ ] `core/src/gateway/handlers/state_bus.rs` (RPC handlers)
- [ ] Integration tests with AX notifications

**Success Criteria**:
- Can subscribe to Mail.app state changes
- Receive delta events when email arrives
- < 10ms latency from AX notification to WebSocket event

### Phase 2: Robustness & Privacy (Week 3)

**Goal**: Add production-grade features.

**Tasks**:
1. Implement `StableElementId` with three-level fallback
2. Implement `StateHistory` with I-Frame + P-Frame
3. Implement `PrivacyFilter` middleware
4. Add `system.state.query` RPC method (historical queries)
5. Add configuration for sensitive apps and patterns
6. Comprehensive error handling and logging

**Deliverables**:
- [ ] `core/src/perception/state_bus/element_id.rs`
- [ ] `core/src/perception/state_bus/state_history.rs`
- [ ] `core/src/perception/state_bus/privacy_filter.rs`
- [ ] Privacy configuration in `~/.aleph/config.toml`
- [ ] Audit logging for filtered events

**Success Criteria**:
- IDs remain stable across UI changes
- Memory usage < 50MB for 30s history
- Password fields automatically redacted
- Can query state from 20 seconds ago

### Phase 3: Action Dispatcher (Week 4)

**Goal**: Close the perception-action loop.

**Tasks**:
1. Implement `ActionDispatcher` with coordinate mapping
2. Implement `SimulationExecutor` (click, type, scroll)
3. Add pre/post validation logic
4. Add `system.action.execute` RPC method
5. Fallback to visual anchors when ID fails
6. Integration with existing `exec` approval system

**Deliverables**:
- [ ] `core/src/perception/action_dispatcher.rs`
- [ ] `core/src/perception/simulation_executor.rs`
- [ ] RPC handler for `system.action.execute`
- [ ] Visual anchor fallback mechanism
- [ ] Approval workflow integration

**Success Criteria**:
- Can click buttons by ID with real-time coordinates
- Pre-validation prevents invalid actions
- Post-validation confirms action success
- Fallback to visual anchors when ID changes

### Phase 4: Vision Connector (Week 5-6)

**Goal**: Support applications without AX API.

**Tasks**:
1. Implement `ConnectorRegistry` with auto-selection
2. Implement `VisionConnector` with OCR polling
3. Implement interactive element detection (CV algorithms)
4. Smart polling strategy (debounce, skip during typing)
5. Diff algorithm for vision-captured states
6. Confidence scoring and error handling

**Deliverables**:
- [ ] `core/src/perception/connectors/registry.rs`
- [ ] `core/src/perception/connectors/vision.rs`
- [ ] `core/src/perception/connectors/plugin.rs` (stub)
- [ ] CV-based button/input detection
- [ ] Polling scheduler with smart triggers

**Success Criteria**:
- Can capture state from legacy Java apps
- Detect buttons and input fields with > 70% accuracy
- Polling overhead < 2% CPU
- Graceful degradation when OCR fails

### Phase 5: Polish & Documentation (Week 7)

**Goal**: Production readiness.

**Tasks**:
1. Performance optimization (profiling, benchmarking)
2. Comprehensive documentation
3. Example Skills using SSB
4. Control Plane UI for SSB monitoring
5. Security audit and penetration testing
6. Load testing (100+ subscriptions)

**Deliverables**:
- [ ] Performance benchmarks
- [ ] API documentation
- [ ] Example Skills (email auto-responder, Notion sync)
- [ ] Control Plane SSB dashboard
- [ ] Security audit report

**Success Criteria**:
- < 2% CPU overhead with 10 active subscriptions
- < 50MB memory with 30s history
- Zero privacy leaks in audit
- Can handle 100 concurrent subscriptions

## Risks and Mitigation

### Technical Risks

| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|------------|
| AX API instability on macOS updates | High | Medium | Version detection, fallback to Vision |
| CFRunLoop conflicts with tokio | High | Low | Dedicated thread isolation (already designed) |
| Memory leaks in state history | Medium | Medium | Strict capacity limits, automated tests |
| OCR accuracy too low | Medium | Medium | Hybrid approach (AX + OCR), confidence scoring |
| Privacy filter bypass | High | Low | Mandatory middleware, audit logging, security review |

### Operational Risks

| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|------------|
| High CPU usage on low-end devices | Medium | Medium | Adaptive polling, subscription limits |
| Network bandwidth for remote clients | Low | High | JSON Patch reduces bandwidth by 90% |
| User confusion about privacy | High | Low | Clear documentation, opt-in for sensitive apps |

## Success Metrics

### Performance Metrics

- **Latency**: < 10ms from AX event to WebSocket delivery
- **CPU Usage**: < 2% with 10 active subscriptions
- **Memory Usage**: < 50MB for 30s history
- **Bandwidth**: < 10KB/s per subscription (with JSON Patch)

### Quality Metrics

- **ID Stability**: > 95% IDs remain valid after UI changes
- **Privacy Compliance**: 100% password fields redacted
- **OCR Accuracy**: > 70% for interactive element detection
- **Uptime**: > 99.9% (no crashes from AX API)

### Adoption Metrics

- **Skill Usage**: > 5 Skills using SSB within 1 month
- **Subscription Count**: > 50 active subscriptions in production
- **Developer Satisfaction**: > 4.5/5 in feedback survey

## Future Enhancements

### Short-term (3 months)

1. **Browser Plugin Connector**: Deep integration with Chrome/Firefox via extensions
2. **Shadow Object Model (SOM)**: Maintain logical models of common apps (Notion, Slack)
3. **Cross-app Entity Linking**: Recognize "Project A" across Notion and Slack
4. **Proactive Triggers**: "When unread count > 10, notify me"

### Long-term (6-12 months)

1. **Multi-device SSB**: Sync state across macOS, iOS, Linux
2. **Collaborative State**: Multiple agents share same state view
3. **Predictive Caching**: Pre-fetch likely next states
4. **Semantic Compression**: LLM-based state summarization

## References

- [RFC 6902: JSON Patch](https://datatracker.ietf.org/doc/html/rfc6902)
- [macOS Accessibility API](https://developer.apple.com/documentation/applicationservices/axuielement)
- [Aleph Gateway Documentation](../GATEWAY.md)
- [Aleph Perception System](../ARCHITECTURE.md#perception-layer)

## Appendix

### A. Example Skill Using SSB

```rust
// Example: Auto-respond to urgent emails

pub async fn email_auto_responder(gateway: &Gateway) -> Result<()> {
    // Subscribe to Mail.app state
    let subscription = gateway.rpc_call("system.state.subscribe", json!({
        "patterns": ["system.state.com.apple.mail.*"],
        "include_snapshot": true
    })).await?;

    // Listen for events
    let mut events = gateway.event_bus.subscribe();
    while let Ok(event) = events.recv().await {
        if event.topic == "system.state.com.apple.mail.delta" {
            let patch: Vec<JsonPatch> = serde_json::from_value(event.data)?;

            // Check if unread count increased
            for p in patch {
                if p.path == "/app_context/unread_count" && p.op == "replace" {
                    let count: u32 = serde_json::from_value(p.value)?;
                    if count > 0 {
                        // Trigger action: click "Reply" button
                        gateway.rpc_call("system.action.execute", json!({
                            "target_id": "btn_reply_001",
                            "method": "click",
                            "expect": {
                                "condition": "state_changed",
                                "key": "focused",
                                "value": true,
                                "timeout_ms": 500
                            }
                        })).await?;
                    }
                }
            }
        }
    }

    Ok(())
}
```

### B. Performance Benchmarks (Target)

| Scenario | Latency | CPU | Memory |
|----------|---------|-----|--------|
| AX event → WebSocket | < 10ms | < 0.1% | - |
| Subscribe (with snapshot) | < 50ms | < 1% | +5MB |
| Query history (20s ago) | < 5ms | < 0.1% | - |
| Vision polling (1 app) | 200ms | 1-2% | +10MB |
| 10 concurrent subscriptions | - | < 2% | < 50MB |

### C. Configuration Example

```toml
# ~/.aleph/config.toml

[system_state_bus]
enabled = true
max_subscriptions = 100
history_duration_secs = 30

[system_state_bus.privacy]
sensitive_apps = [
    "com.agilebits.onepassword7",
    "com.apple.keychainaccess",
    "com.tencent.xinWeChat"
]
filter_patterns = ["credit_card", "ssn", "phone", "email"]
audit_log_path = "~/.aleph/privacy_audit.log"

[system_state_bus.connectors.vision]
enabled = true
polling_interval_secs = 2
ocr_confidence_threshold = 0.3
max_ocr_blocks = 200

[system_state_bus.connectors.ax]
enabled = true
max_depth = 12
max_nodes = 1500
```

---

**Document Status**: Design Complete
**Next Steps**: Begin Phase 1 implementation
**Review Date**: 2026-02-18

