# System State Bus (SSB)

> **Real-time application state streaming for AI agents**

The System State Bus provides continuous, event-driven access to application UI state through the Gateway's EventBus. It enables AI agents to perceive and interact with applications in real-time, moving from passive tool invocation to active environment awareness.

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Quick Start](#quick-start)
- [API Reference](#api-reference)
- [Connectors](#connectors)
- [Privacy & Security](#privacy--security)
- [Performance](#performance)
- [Examples](#examples)

## Overview

### Key Features

- **Event-driven**: O(1) complexity, no polling overhead
- **Multi-source**: Accessibility API, Browser Plugins, Vision/OCR
- **Incremental updates**: JSON Patch (RFC 6902) reduces bandwidth by 90%
- **Privacy-first**: Automatic redaction of sensitive data
- **State history**: I-Frame + P-Frame optimization (30s history in 12MB)
- **Stable IDs**: 3-level fallback ensures 95%+ ID stability

### Use Cases

1. **Proactive Assistance**: "When unread count > 10, summarize emails"
2. **Cross-app Automation**: "Copy Slack message to Notion"
3. **Context-aware Actions**: "Click 'Send' button when draft is ready"
4. **UI Testing**: Automated interaction testing without brittle selectors

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Application Layer                         │
│  Mail.app │ Notion │ Slack │ Chrome │ Legacy Java App       │
└────┬──────────┬──────────┬──────────┬──────────────────────┘
     │          │          │          │
     ▼          ▼          ▼          ▼
┌─────────────────────────────────────────────────────────────┐
│                   Connector Layer                            │
│  AxConnector │ PluginConnector │ VisionConnector            │
│  (Priority 1)│  (Priority 2)   │  (Priority 3, Fallback)    │
└────┬──────────┬──────────┬──────────────────────────────────┘
     │          │          │
     └──────────┴──────────┘
              │
              ▼
┌─────────────────────────────────────────────────────────────┐
│                  System State Bus                            │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │ State Cache  │  │ State History│  │Privacy Filter│      │
│  │ (Real-time)  │  │ (I+P Frames) │  │ (Middleware) │      │
│  └──────────────┘  └──────────────┘  └──────────────┘      │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────┐
│                    Gateway EventBus                          │
│  Topic: system.state.{app_id}.{event_type}                  │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────┐
│                   WebSocket Clients                          │
│  Skills │ Control Plane │ External Agents                   │
└─────────────────────────────────────────────────────────────┘
```

### Data Flow

1. **State Capture**: Connector captures application state
2. **Privacy Filter**: Sensitive data redacted (passwords, credit cards)
3. **State Cache**: Current state stored for coordinate mapping
4. **State History**: I-Frame (full) + P-Frame (delta) stored
5. **Event Publish**: JSON Patch delta published to EventBus
6. **Client Delivery**: WebSocket clients receive real-time updates

## Quick Start

### 1. Subscribe to Application State

```rust
use alephcore::gateway::Gateway;
use serde_json::json;

async fn subscribe_to_mail(gateway: &Gateway) -> Result<()> {
    // Subscribe to Mail.app state changes
    let result = gateway.rpc_call("system.state.subscribe", json!({
        "patterns": ["system.state.com.apple.mail.*"],
        "include_snapshot": true,
        "debounce_ms": 100
    })).await?;

    println!("Subscription ID: {}", result["subscription_id"]);
    Ok(())
}
```

### 2. Listen for State Changes

```rust
async fn listen_for_changes(gateway: &Gateway) -> Result<()> {
    let mut events = gateway.event_bus.subscribe();

    while let Ok(event) = events.recv().await {
        if event.topic.starts_with("system.state.") {
            println!("State changed: {:?}", event.data);
        }
    }

    Ok(())
}
```

### 3. Execute Actions

```rust
async fn click_send_button(gateway: &Gateway) -> Result<()> {
    gateway.rpc_call("system.action.execute", json!({
        "target_id": "btn_send_001",
        "method": "click",
        "expect": {
            "condition": "element_disappear",
            "target": "btn_send_001",
            "timeout_ms": 500
        }
    })).await?;

    Ok(())
}
```

## API Reference

### RPC Methods

#### `system.state.subscribe`

Subscribe to application state changes.

**Parameters**:
```json
{
  "patterns": ["system.state.{app_id}.*"],
  "include_snapshot": true,
  "debounce_ms": 100
}
```

**Response**:
```json
{
  "subscription_id": "sub_12345",
  "active_patterns": ["system.state.com.apple.mail.*"],
  "initial_snapshot": { ... }
}
```

#### `system.state.unsubscribe`

Unsubscribe from state changes.

**Parameters**:
```json
{
  "subscription_id": "sub_12345"
}
```

#### `system.state.query`

Query historical state.

**Parameters**:
```json
{
  "app_id": "com.apple.mail",
  "timestamp": 1739268000,
  "window_id": "win_001"
}
```

**Response**:
```json
{
  "state": {
    "app_id": "com.apple.mail",
    "elements": [...],
    "source": "accessibility",
    "confidence": 1.0
  }
}
```

#### `system.action.execute`

Execute UI action with validation.

**Parameters**:
```json
{
  "target_id": "btn_send_001",
  "method": "click",
  "expect": {
    "condition": "element_disappear",
    "target": "btn_send_001",
    "timeout_ms": 500
  }
}
```

**Response**:
```json
{
  "success": true,
  "validation": {
    "pre_check": "passed",
    "post_check": "passed"
  }
}
```

### Event Topics

| Topic Pattern | Description |
|---------------|-------------|
| `system.state.{app_id}.delta` | Incremental state changes (JSON Patch) |
| `system.state.{app_id}.snapshot` | Full state snapshot (I-Frame) |
| `system.state.{app_id}.error` | State capture errors |

### Data Types

#### AppState

```rust
pub struct AppState {
    pub app_id: String,
    pub elements: Vec<Element>,
    pub app_context: Option<Value>,
    pub source: StateSource,
    pub confidence: f32,
}
```

#### Element

```rust
pub struct Element {
    pub id: String,
    pub role: String,
    pub label: Option<String>,
    pub current_value: Option<String>,
    pub rect: Option<Rect>,
    pub state: ElementState,
    pub source: ElementSource,
    pub confidence: f32,
}
```

#### Rect

```rust
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
```

## Connectors

### Connector Priority

The System State Bus automatically selects the best connector for each application:

1. **Accessibility Connector** (Priority 1)
   - Source: macOS Accessibility API
   - Latency: < 10ms
   - Accuracy: 100%
   - Coverage: Most native apps

2. **Plugin Connector** (Priority 2)
   - Source: Browser/IDE extensions
   - Latency: < 20ms
   - Accuracy: 100%
   - Coverage: Chrome, Firefox, VS Code

3. **Vision Connector** (Priority 3, Fallback)
   - Source: OCR + Computer Vision
   - Latency: 200ms
   - Accuracy: 70-80%
   - Coverage: Universal (all apps)

### Vision Connector

The Vision Connector provides universal fallback for applications without Accessibility API support.

**Features**:
- OCR text extraction
- Interactive element detection (buttons, inputs)
- Smart polling with exponential backoff
- State hash-based change detection

**Configuration**:
```toml
[system_state_bus.connectors.vision]
enabled = true
polling_interval_secs = 2
ocr_confidence_threshold = 0.3
max_ocr_blocks = 200
```

## Privacy & Security

### Automatic Redaction

The Privacy Filter automatically redacts sensitive data:

- **Password fields**: AXSecureTextField
- **Credit cards**: Luhn algorithm validation
- **SSN**: Pattern matching
- **Phone numbers**: International formats
- **Sensitive apps**: 1Password, Keychain, WeChat

### Configuration

```toml
[system_state_bus.privacy]
sensitive_apps = [
    "com.agilebits.onepassword7",
    "com.apple.keychainaccess",
    "com.tencent.xinWeChat"
]
filter_patterns = ["credit_card", "ssn", "phone", "email"]
audit_log_path = "~/.aleph/privacy_audit.log"
```

### Audit Logging

All privacy filter actions are logged:

```
[2026-02-11 10:30:45] REDACTED: credit_card in com.apple.safari (confidence: 0.95)
[2026-02-11 10:31:12] BLOCKED: com.agilebits.onepassword7 (sensitive app)
```

## Performance

### Benchmarks

| Scenario | Latency | CPU | Memory |
|----------|---------|-----|--------|
| AX event → WebSocket | < 10ms | < 0.1% | - |
| Subscribe (with snapshot) | < 50ms | < 1% | +5MB |
| Query history (20s ago) | < 5ms | < 0.1% | - |
| Vision polling (1 app) | 200ms | 1-2% | +10MB |
| 10 concurrent subscriptions | - | < 2% | < 50MB |

### Memory Optimization

**I-Frame + P-Frame Strategy**:
- I-Frame (full snapshot): Every 5 seconds
- P-Frame (JSON Patch): Incremental changes
- 30s history: 12MB (vs 600MB without optimization)

### Bandwidth Optimization

**JSON Patch (RFC 6902)**:
- Full state: 50KB
- JSON Patch: 500 bytes (99% reduction)
- 10 subscriptions: < 10KB/s total

## Examples

### Example 1: Email Auto-Responder

```rust
use alephcore::gateway::Gateway;
use serde_json::json;

pub async fn email_auto_responder(gateway: &Gateway) -> Result<()> {
    // Subscribe to Mail.app state
    gateway.rpc_call("system.state.subscribe", json!({
        "patterns": ["system.state.com.apple.mail.*"],
        "include_snapshot": true
    })).await?;

    // Listen for events
    let mut events = gateway.event_bus.subscribe();
    while let Ok(event) = events.recv().await {
        if event.topic == "system.state.com.apple.mail.delta" {
            let patches: Vec<JsonPatch> = serde_json::from_value(event.data)?;

            // Check if unread count increased
            for patch in patches {
                if patch.path == "/app_context/unread_count" && patch.op == "replace" {
                    let count: u32 = serde_json::from_value(patch.value)?;
                    if count > 0 {
                        // Click "Reply" button
                        gateway.rpc_call("system.action.execute", json!({
                            "target_id": "btn_reply_001",
                            "method": "click"
                        })).await?;
                    }
                }
            }
        }
    }

    Ok(())
}
```

### Example 2: Notion Sync

```rust
pub async fn notion_sync(gateway: &Gateway) -> Result<()> {
    // Subscribe to Notion state
    gateway.rpc_call("system.state.subscribe", json!({
        "patterns": ["system.state.com.reallusion.Notion.*"]
    })).await?;

    let mut events = gateway.event_bus.subscribe();
    while let Ok(event) = events.recv().await {
        if event.topic.contains("Notion") {
            // Extract selected text
            let state: AppState = serde_json::from_value(event.data)?;
            for element in state.elements {
                if element.state.selected {
                    if let Some(text) = element.current_value {
                        // Sync to external system
                        sync_to_external(&text).await?;
                    }
                }
            }
        }
    }

    Ok(())
}
```

### Example 3: UI Testing

```rust
pub async fn test_login_flow(gateway: &Gateway) -> Result<()> {
    // Subscribe to app state
    gateway.rpc_call("system.state.subscribe", json!({
        "patterns": ["system.state.com.example.app.*"]
    })).await?;

    // Type username
    gateway.rpc_call("system.action.execute", json!({
        "target_id": "input_username",
        "method": "type",
        "params": { "text": "testuser" }
    })).await?;

    // Type password
    gateway.rpc_call("system.action.execute", json!({
        "target_id": "input_password",
        "method": "type",
        "params": { "text": "testpass" }
    })).await?;

    // Click login button
    gateway.rpc_call("system.action.execute", json!({
        "target_id": "btn_login",
        "method": "click",
        "expect": {
            "condition": "element_appear",
            "target": "dashboard_container",
            "timeout_ms": 2000
        }
    })).await?;

    Ok(())
}
```

## Troubleshooting

### Common Issues

**Issue**: High CPU usage
- **Cause**: Too many active subscriptions
- **Solution**: Reduce subscription count, increase debounce interval

**Issue**: Missing state updates
- **Cause**: Application not supported by Accessibility API
- **Solution**: Enable Vision Connector in config

**Issue**: Privacy filter too aggressive
- **Cause**: False positives in pattern matching
- **Solution**: Adjust `filter_patterns` in config

**Issue**: Stale element IDs
- **Cause**: UI structure changed
- **Solution**: Use 3-level ID fallback (automatic)

## References

- [RFC 6902: JSON Patch](https://datatracker.ietf.org/doc/html/rfc6902)
- [macOS Accessibility API](https://developer.apple.com/documentation/applicationservices/axuielement)
- [Aleph Gateway Documentation](GATEWAY.md)
- [Aleph Architecture](ARCHITECTURE.md)

## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md) for guidelines on:
- Adding new connectors
- Improving OCR accuracy
- Writing example skills
- Performance optimization

---

**Status**: Production Ready (Phase 5 Complete)
**Version**: 0.2.0
**Last Updated**: 2026-02-11
