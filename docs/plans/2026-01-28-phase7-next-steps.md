# Phase 7: Next Steps - Aleph vs Moltbot Gap Analysis

**Date**: 2026-01-28
**Status**: Planning

---

## Current Progress Summary

### Completed (Phase 6a/b/c)

| Component | Status | Files |
|-----------|--------|-------|
| **Gateway WebSocket** | ✅ Complete | `server.rs`, `protocol.rs` |
| **Session Management** | ✅ Complete | `session_manager.rs` (SQLite) |
| **Event Distribution** | ✅ Complete | `event_bus.rs` |
| **Channel Abstraction** | ✅ Complete | `channel.rs`, `channel_registry.rs` |
| **iMessage Channel** | ✅ Complete | `channels/imessage/` |
| **Agent Runtime (RPC)** | ✅ Complete | `execution_engine.rs` |
| **Tool Streaming** | ✅ Complete | `agent_loop.rs` |
| **macOS App (Swift)** | ✅ Complete | `platforms/macos/` |

### Key Gaps (vs Moltbot)

| Gap | Priority | Moltbot Reference | Impact |
|-----|----------|-------------------|--------|
| **Hot Config Reload** | High | `config-reload.ts` | Runtime flexibility |
| **Device Pairing** | High | `pairing.ts` | Security model |
| **Telegram Channel** | High | `extensions/telegram/` | User reach |
| **WebChat UI** | High | `src/web/`, `ui/` | Browser access |
| **Cron Jobs** | Medium | `server-cron.ts` | Automation |
| **Browser Control** | Medium | `src/browser/` | Tool capability |
| **Model Failover** | Medium | `models-config.ts` | Reliability |
| **Discord Channel** | Low | `extensions/discord/` | User reach |
| **Slack Channel** | Low | `extensions/slack/` | User reach |

---

## Recommended Next Phase: Phase 7

### Option A: "Core Stability" (Recommended)

Focus on completing Gateway core features before adding more channels.

**Week 1-2: Hot Config Reload + Auth Enhancement**

```
Task 7.1: Hot Configuration Reload
├── File watcher (notify crate)
├── Config validation before apply
├── Atomic reload (no partial state)
├── Event broadcast on change
└── CLI: `aether config reload`

Task 7.2: Device Pairing System
├── Device fingerprint generation
├── Pairing code flow
├── Token issuance after approval
├── Device registry (SQLite)
└── CLI: `aether pairing approve/reject/list`
```

**Week 3-4: WebChat UI**

```
Task 7.3: Embedded WebChat
├── Static file serving (Axum)
├── React/Lit-based chat UI
├── WebSocket connection to Gateway
├── Message rendering (Markdown)
└── Session selection UI
```

### Option B: "Channel Expansion"

Add more messaging channels to increase user reach.

**Week 1-2: Telegram Channel**

```
Task 7.1: Telegram Bot Integration
├── teloxide crate integration
├── Bot token configuration
├── Message send/receive
├── Inline keyboards
├── File/media handling
└── Webhook or long-polling mode
```

**Week 3-4: Discord Channel**

```
Task 7.2: Discord Bot Integration
├── serenity crate integration
├── Bot token + OAuth2
├── Guild/DM message handling
├── Slash commands
├── Reactions/embeds
└── Voice channel (future)
```

### Option C: "Tools & Automation"

Add powerful automation features.

**Week 1-2: Cron Jobs**

```
Task 7.1: Scheduled Job System
├── Cron expression parser (cron crate)
├── Job storage (SQLite)
├── Execution engine
├── Gateway RPC: cron.list/create/delete
├── CLI: `aether cron`
└── Agent tool: schedule_job()
```

**Week 3-4: Browser Control**

```
Task 7.2: CDP Browser Integration
├── chromiumoxide crate
├── Managed browser lifecycle
├── Page navigation
├── Element interaction
├── Screenshots/PDF
└── Agent tool: browser_*()
```

---

## Detailed Implementation: Option A (Recommended)

### Task 7.1: Hot Configuration Reload

**Moltbot Reference**: `/src/config/config-reload.ts`

**Implementation Plan**:

```rust
// core/src/config/hot_reload.rs

pub struct ConfigWatcher {
    config_path: PathBuf,
    watcher: RecommendedWatcher,
    event_tx: broadcast::Sender<ConfigEvent>,
}

impl ConfigWatcher {
    pub fn new(config_path: PathBuf) -> Result<Self>;
    pub async fn start(&mut self) -> Result<()>;
    pub fn subscribe(&self) -> broadcast::Receiver<ConfigEvent>;
}

pub enum ConfigEvent {
    Reloaded(Arc<GatewayConfig>),
    ValidationFailed(String),
    FileError(std::io::Error),
}
```

**Gateway Integration**:

```rust
// In aleph_gateway.rs
let config_watcher = ConfigWatcher::new(config_path)?;
let mut config_rx = config_watcher.subscribe();

tokio::spawn(async move {
    while let Ok(event) = config_rx.recv().await {
        match event {
            ConfigEvent::Reloaded(new_config) => {
                // Apply new config atomically
                server.apply_config(new_config).await;
                event_bus.emit("config.reloaded", json!({}));
            }
            ConfigEvent::ValidationFailed(err) => {
                tracing::warn!("Config validation failed: {}", err);
            }
        }
    }
});
```

**RPC Methods**:
- `config.reload` - Force reload config
- `config.get` - Get current config
- `config.validate` - Validate config file

**CLI Commands**:
```bash
aether config reload        # Force reload
aether config show          # Show current config
aether config validate      # Validate config file
```

---

### Task 7.2: Device Pairing System

**Moltbot Reference**: `/src/gateway/protocol/connect.ts`, `/src/channels/plugins/pairing.ts`

**Implementation Plan**:

```rust
// core/src/gateway/device_pairing.rs

pub struct DeviceRegistry {
    db: Arc<Mutex<Connection>>,
}

pub struct DeviceInfo {
    pub id: String,           // Fingerprint
    pub name: String,
    pub platform: String,     // macos, ios, android, cli
    pub public_key: String,
    pub paired_at: i64,
    pub last_seen_at: i64,
    pub status: DeviceStatus,
}

pub enum DeviceStatus {
    Pending,
    Approved,
    Rejected,
    Revoked,
}

impl DeviceRegistry {
    pub async fn register_device(&self, info: DeviceInfo) -> Result<String>; // Returns pairing code
    pub async fn approve_device(&self, device_id: &str) -> Result<DeviceToken>;
    pub async fn reject_device(&self, device_id: &str) -> Result<()>;
    pub async fn revoke_device(&self, device_id: &str) -> Result<()>;
    pub async fn list_devices(&self) -> Result<Vec<DeviceInfo>>;
    pub async fn validate_token(&self, token: &str) -> Result<DeviceInfo>;
}
```

**Pairing Flow**:

```
1. New device connects to Gateway
2. Gateway generates pairing code (6 digits)
3. User approves via CLI: `aether pairing approve <code>`
4. Gateway issues device token
5. Device stores token for future connections
```

**RPC Methods**:
- `pairing.list` - List pending/approved devices
- `pairing.approve` - Approve device by code
- `pairing.reject` - Reject device
- `pairing.revoke` - Revoke approved device

---

### Task 7.3: WebChat UI

**Moltbot Reference**: `/ui/src/`, `/src/web/`

**Implementation Plan**:

```
ui/
├── webchat/
│   ├── package.json       # React + Vite
│   ├── src/
│   │   ├── App.tsx        # Main app
│   │   ├── Gateway.ts     # WebSocket client
│   │   ├── Chat.tsx       # Chat UI
│   │   ├── Message.tsx    # Message bubble
│   │   ├── Input.tsx      # Input area
│   │   ├── Sidebar.tsx    # Session list
│   │   └── styles/        # Tailwind CSS
│   └── dist/              # Built output
```

**Gateway Static Serving**:

```rust
// In aleph_gateway.rs

// Serve WebChat UI at /
let app = Router::new()
    .route("/ws", get(websocket_handler))
    .nest_service("/", ServeDir::new("ui/webchat/dist"))
    .fallback_service(ServeFile::new("ui/webchat/dist/index.html"));
```

**WebChat Features**:
- Real-time message streaming
- Markdown rendering
- Code syntax highlighting
- Session switching
- Dark/light theme
- Mobile responsive

---

## Implementation Order

| Week | Tasks | Deliverables |
|------|-------|--------------|
| **Week 1** | Hot Config Reload | File watcher, validation, atomic reload |
| **Week 2** | Device Pairing | Registry, pairing flow, token auth |
| **Week 3** | WebChat UI (Backend) | Static serving, WebSocket integration |
| **Week 4** | WebChat UI (Frontend) | React app, chat rendering, styling |

---

## Success Criteria

### Phase 7.1: Hot Config Reload ✅ COMPLETE
- [x] Config changes detected within 1s
- [x] Invalid config rejected with clear error
- [x] No service interruption during reload
- [x] `config.reloaded` event emitted

**Implementation Summary:**
- Created `hot_reload.rs` with ConfigWatcher using notify-debouncer-full
- Added RPC handlers: config.reload, config.get, config.validate, config.path
- Integrated hot reload into aleph_gateway.rs
- Events broadcast to connected clients on config change

### Phase 7.2: Device Pairing ✅ ALREADY IMPLEMENTED
- [x] New devices require approval
- [x] Pairing code valid for 5 minutes
- [x] Device tokens persist across restarts (SQLite-backed DeviceStore)
- [x] CLI commands work: approve/reject/list
- [x] RPC handlers: pairing.approve, pairing.reject, pairing.list, devices.list, devices.revoke
- [x] TokenManager with validation and device-scoped tokens
- [x] PairingManager with 6-digit code generation

**Already implemented in:**
- `core/src/gateway/device_store.rs` - SQLite device registry
- `core/src/gateway/security/pairing.rs` - Pairing flow management
- `core/src/gateway/security/token.rs` - Token generation/validation
- `core/src/gateway/handlers/auth.rs` - RPC handlers
- `core/src/bin/aleph_gateway.rs` - CLI commands (pairing/devices subcommands)

### Phase 7.3: WebChat UI ✅ COMPLETE
- [x] Access at http://127.0.0.1:18790/ (separate HTTP port)
- [x] Real-time message streaming via WebSocket
- [x] Session list with switching
- [x] Mobile responsive design (Tailwind CSS)
- [x] Dark/light theme toggle
- [x] Markdown rendering with syntax highlighting

**Implementation:**
- React + Vite + TypeScript + Tailwind CSS
- WebSocket hook (`useGateway.ts`) for Gateway communication
- Components: Sidebar, ChatInput, MessageBubble
- Static file serving via Axum (tower-http)
- CLI args: `--webchat-dir`, `--webchat-port`

---

## Technical Dependencies

### New Crates

```toml
# For hot config reload
notify = "8.0"              # File system notifications

# For WebChat static serving (already have axum)
tower-http = "0.6"          # ServeDir, ServeFile

# For WebChat UI (npm packages)
# React, Vite, Tailwind CSS
```

### Moltbot Code to Reference

| Feature | Moltbot File | Purpose |
|---------|--------------|---------|
| Config Reload | `src/config/config-reload.ts` | Watcher + validation |
| Device Registry | `src/gateway/node-registry.ts` | Device management |
| Pairing Flow | `src/gateway/protocol/connect.ts` | Handshake protocol |
| WebChat | `ui/src/app.ts` | Chat UI architecture |
| Static Serve | `src/gateway/server-http.ts` | HTTP endpoints |

---

## Decision: Recommendation

**我推荐 Option A: "Core Stability"**

理由：
1. **热配置重载** 是生产环境必需的功能
2. **设备配对** 完善安全模型，为多渠道做准备
3. **WebChat** 让用户可以通过浏览器使用，降低使用门槛
4. 在添加更多渠道（Telegram、Discord）之前，核心基础设施应该稳固

下一步：确认你的选择，然后我可以开始实施。
