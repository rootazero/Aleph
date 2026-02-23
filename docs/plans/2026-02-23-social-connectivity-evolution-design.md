# Aleph Social Connectivity Evolution Design

> *"From a hardcoded multi-channel bot to a runtime-configurable omni-channel communication hub."*

**Date**: 2026-02-23
**Status**: Approved
**Scope**: Full blueprint + Phase 1 detailed design

---

## 1. Background & Gap Analysis

### Current Architecture

Aleph currently operates in a **hybrid mode** for social connectivity:

| Type | Channels | Implementation |
|------|----------|---------------|
| **Native** | Telegram, Discord, iMessage | Rust crates compiled into Core (teloxide, serenity) |
| **External** | WhatsApp | Go binary (`bridges/whatsapp`), JSON-RPC 2.0 over Unix Socket |

**Existing Strengths:**
- Well-designed `Channel` trait with `start/stop/send/capabilities/pairing_data`
- `ChannelRegistry` with factory pattern + unified message stream (`mpsc::Receiver<InboundMessage>`)
- `InboundMessageRouter` with permission checking, agent routing, pairing flow
- Type-safe ID system (`ChannelId`, `ConversationId`, `MessageId`)
- Feature flag driven compilation (`telegram`, `discord`, `whatsapp`)

**Gaps:**
- Configuration hardcoded in `config.toml` + `start.rs`; no multi-instance support
- No universal bridge protocol standard (WhatsApp is the only case)
- Control Plane UI lacks channel management interface
- No `BridgeSupervisor` for process health monitoring and auto-recovery
- Adding new channels requires Core code changes

### Design Goal

Build a **Manifest-Driven Plugin Architecture** that:
1. Supports multi-instance (e.g., 2 Telegram bots with different configs)
2. Defines a universal bridge specification for community plugin development
3. Presents native and external bridges through a unified management interface
4. Enables runtime hot-reload of channel configurations

---

## 2. Core Architecture: Universal Social Link Layer

### 2.1 Architecture Overview

```
┌─────────────────────────────────────────────────────┐
│                    LinkManager                        │
│  (scan config → instantiate → lifecycle orchestration)│
├────────────────┬────────────────┬────────────────────┤
│  BridgeRegistry│  ChannelRegistry │ BridgeSupervisor │
│  (bridge.yaml  │  (existing,      │ (process mgmt)   │
│   type registry)│  enhanced)      │                  │
└────────────────┴────────────────┴────────────────────┘
         │                │                │
         ▼                ▼                ▼
   bridge.yaml      Channel trait     External Process
   parse + validate  instance mgmt    lifecycle + heartbeat
```

### 2.2 Dual-Track Plugin Model

To balance performance and extensibility:

| Track | Channels | Implementation | Overhead |
|-------|----------|---------------|----------|
| **Native (Builtin)** | Telegram, Discord, iMessage | Rust crates compiled into Core | Zero |
| **Process (External)** | WhatsApp, WeChat, community plugins | Independent OS process, any language | ~μs IPC (negligible vs 50-300ms network RTT) |

Both tracks are **transparent to users** — managed through the same `link.yaml` config and Control Plane UI.

### 2.3 Relationship to Existing Components

| Existing Component | Evolution |
|-------------------|-----------|
| `Channel` trait | **Retained**, add `bridge_id()` method |
| `ChannelRegistry` | **Internal component** of LinkManager for instance registration |
| `BridgeManager` | **Generalized** into `BridgeSupervisor` for all external bridges |
| `InboundMessageRouter` | **Unchanged**, continues consuming unified message stream |
| `ChannelFactory` | **Refactored**, dynamically created from bridge definitions |
| `config.toml [channels]` | **Removed**, migrated to `~/.aleph/links/` |

---

## 3. Aleph Bridge Specification (ABS)

### 3.1 bridge.yaml — Plugin Type Manifest

Defines **what a bridge is** — type identity, runtime configuration, capabilities, and settings schema.

```yaml
# Built-in bridges are compiled into the binary (no file needed)
# External bridges: ~/.aleph/bridges/<bridge-id>/bridge.yaml

spec_version: "1.0"
id: "telegram-native"
name: "Telegram"
version: "0.1.0"
author: "Aleph Team"
description: "Native Telegram bot integration via teloxide"

runtime:
  type: "builtin"                    # builtin | process
  # Process-only fields:
  # executable: "./bin/whatsapp-bridge"
  # args: ["--mode", "jsonrpc"]
  # transport: "unix-socket"          # unix-socket | stdio
  # health_check_interval_secs: 30
  # max_restarts: 5
  # restart_delay_secs: 3

capabilities:
  messaging:
    - send_text
    - receive_text
    - send_image
    - receive_image
    - send_file
    - receive_file
  interactions:
    - inline_keyboard
    - reply_threading
  lifecycle:
    - pairing_token
  optional:
    - typing_indicator
    - read_receipts
    - message_editing
    - message_deletion
    - reactions

settings_schema:                     # JSON Schema for auto-generated UI forms
  type: object
  required: ["token"]
  properties:
    token:
      type: string
      title: "Bot Token"
      description: "Telegram Bot API token from @BotFather"
      format: "password"
    allowed_users:
      type: array
      items: { type: integer }
      title: "Allowed User IDs"
    polling_mode:
      type: string
      enum: ["long_polling", "webhook"]
      default: "long_polling"
```

### 3.2 link.yaml — Instance Configuration

Defines **how to use a bridge** — specific instance with concrete parameters.

```yaml
# ~/.aleph/links/my-personal-telegram.yaml

spec_version: "1.0"
id: "my-personal-telegram"
bridge: "telegram-native"            # References bridge.yaml id
name: "My Personal Bot"
enabled: true

settings:                            # Validated against bridge's settings_schema
  token: "${env.TELEGRAM_BOT_TOKEN}" # Environment variable expansion
  allowed_users: [12345678, 87654321]
  polling_mode: "long_polling"

routing:
  agent: "main"                      # Route to which Agent
  dm_policy: "pairing"              # open | pairing | allowlist | disabled
  group_policy: "disabled"
```

Multi-instance example — same bridge type, different config:

```yaml
# ~/.aleph/links/work-telegram.yaml

spec_version: "1.0"
id: "work-telegram"
bridge: "telegram-native"
name: "Work Bot"
enabled: true

settings:
  token: "${env.WORK_TG_TOKEN}"
  allowed_users: [99999999]

routing:
  agent: "work-assistant"
  dm_policy: "allowlist"
```

### 3.3 Aleph Bridge Protocol (ABP) — External Bridge JSON-RPC

Standardized from the existing WhatsApp bridge experience.

**Core → Bridge (Requests):**

| Method | Description |
|--------|------------|
| `aleph.handshake` | Exchange version info, confirm capabilities |
| `aleph.link.start` | Start connection (begin login flow) |
| `aleph.link.stop` | Stop connection |
| `aleph.link.send` | Send message (text, image, file) |
| `aleph.link.get_pairing` | Get pairing info (QR code / token) |
| `aleph.link.mark_read` | Mark message as read |
| `aleph.link.react` | Send reaction |
| `system.ping` | Health check heartbeat |

**Bridge → Core (Event Notifications):**

| Method | Description |
|--------|------------|
| `event.message` | New inbound message |
| `event.status_change` | Status change (Connected/Disconnected/Error) |
| `event.pairing_update` | Pairing state update (QR Code/Scanned/Success) |
| `event.receipt` | Message receipt (delivered/read) |

### 3.4 Directory Structure

```
~/.aleph/
├── config.toml                  # Main config (no longer contains channels)
├── bridges/                     # External bridge type registrations
│   ├── whatsapp-go/
│   │   ├── bridge.yaml
│   │   └── bin/whatsapp-bridge
│   └── wechat-python/
│       ├── bridge.yaml
│       └── main.py
├── links/                       # Running instance configurations
│   ├── my-telegram.yaml
│   ├── work-telegram.yaml
│   ├── my-whatsapp.yaml
│   └── family-discord.yaml
└── run/                         # Runtime state (auto-managed)
    ├── my-whatsapp.sock         # Unix Socket
    └── my-whatsapp.pid          # PID file
```

Built-in bridges (Telegram, Discord, iMessage) have their `bridge.yaml` equivalent compiled into the binary. LinkManager auto-registers builtin bridge types at startup.

---

## 4. Phase 1 Detailed Design: Infrastructure

### 4.1 Data Model

```rust
// ═══ Bridge Type Definition (from bridge.yaml) ═══

pub struct BridgeDefinition {
    pub id: BridgeId,
    pub name: String,
    pub version: String,
    pub runtime: BridgeRuntime,
    pub capabilities: BridgeCapabilities,
    pub settings_schema: Option<serde_json::Value>,
}

pub enum BridgeRuntime {
    Builtin,
    Process {
        executable: PathBuf,
        args: Vec<String>,
        transport: TransportType,
        health_check_interval: Duration,
        max_restarts: u32,
        restart_delay: Duration,
    },
}

pub enum TransportType {
    UnixSocket,
    Stdio,
}

// ═══ Link Instance Definition (from link.yaml) ═══

pub struct LinkConfig {
    pub id: LinkId,
    pub bridge: BridgeId,
    pub name: String,
    pub enabled: bool,
    pub settings: serde_json::Value,
    pub routing: LinkRoutingConfig,
}

pub struct LinkRoutingConfig {
    pub agent: String,
    pub dm_policy: DmPolicy,
    pub group_policy: GroupPolicy,
}
```

### 4.2 LinkManager

Central orchestrator that scans configuration, instantiates channels, and manages lifecycle.

```rust
pub struct LinkManager {
    bridge_registry: RwLock<HashMap<BridgeId, BridgeDefinition>>,
    builtin_factories: HashMap<BridgeId, Arc<dyn ChannelFactory>>,
    channel_registry: Arc<ChannelRegistry>,
    bridge_supervisor: Arc<BridgeSupervisor>,
    links_dir: PathBuf,       // ~/.aleph/links/
    bridges_dir: PathBuf,     // ~/.aleph/bridges/
    _watcher: Option<notify::RecommendedWatcher>,
}
```

**Startup flow:**
1. Register builtin bridge types (Telegram, Discord, iMessage)
2. Scan `~/.aleph/bridges/` for external bridge types
3. Scan `~/.aleph/links/` for instance configurations
4. Validate each link's settings against its bridge's `settings_schema`
5. Instantiate and start all enabled links
6. Start file watcher for hot-reload

**Hot-reload on link.yaml change:**
1. Parse changed YAML file
2. Stop and remove old instance (if exists)
3. If enabled, create and start new instance

### 4.3 BridgeSupervisor

Manages external bridge process lifecycle with heartbeat monitoring and auto-recovery.

```rust
pub struct BridgeSupervisor {
    processes: RwLock<HashMap<LinkId, ManagedProcess>>,
    run_dir: PathBuf,   // ~/.aleph/run/
}

struct ManagedProcess {
    link_id: LinkId,
    child: Child,
    transport: Box<dyn Transport>,
    config: BridgeRuntime,
    restart_count: u32,
    last_heartbeat: Instant,
    status: ProcessStatus,
}

enum ProcessStatus {
    Starting,
    Running,
    Unhealthy,
    Restarting,
    Stopped,
    Failed(String),
}
```

**Process spawn flow:**
1. Clean up stale socket file
2. Spawn process with environment variables: `ALEPH_INSTANCE_ID`, `ALEPH_SOCKET_PATH`, `ALEPH_LOG_LEVEL`
3. Create Transport (wait for socket or attach to stdio)
4. Perform `aleph.handshake`
5. Create `BridgedChannel` wrapper
6. Start heartbeat monitor (30s interval, `system.ping`)

**Heartbeat & self-healing:**
- `system.ping` every 30 seconds
- On failure: attempt restart (up to `max_restarts`)
- On max restarts exceeded: mark as `Failed`, emit error event

### 4.4 Transport Trait

Abstract IPC layer supporting multiple transport mechanisms.

```rust
#[async_trait]
pub trait Transport: Send + Sync {
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value>;

    async fn next_event(&self) -> Option<BridgeEvent>;

    async fn close(&self) -> Result<()>;
}
```

**Implementations:**
- `UnixSocketTransport`: Extracted from existing WhatsApp `BridgeRpcClient`
- `StdioTransport`: New, reads JSON-RPC from stdout, writes to stdin

### 4.5 BridgedChannel

Proxy that wraps an external process as a `Channel` trait implementation.

```rust
pub struct BridgedChannel {
    id: ChannelId,
    name: String,
    capabilities: ChannelCapabilities,
    transport: Arc<dyn Transport>,
    status: RwLock<ChannelStatus>,
    inbound_tx: mpsc::Sender<InboundMessage>,
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
}
```

Implements `Channel` trait by delegating all operations to the Transport:
- `start()` → `aleph.link.start` + spawn event listener task
- `send()` → `aleph.link.send`
- `get_pairing_data()` → `aleph.link.get_pairing`
- `mark_read()` → `aleph.link.mark_read`
- etc.

### 4.6 Configuration Migration

**Changes:**
1. **Remove** `[channels]` section from `config.toml`
2. **Create** `~/.aleph/links/` directory with YAML files for each channel
3. **Refactor** `start.rs`: replace hardcoded channel initialization with `LinkManager.start()`

**Server startup before/after:**

```rust
// Before (hardcoded in start.rs):
#[cfg(feature = "telegram")]
{
    let config = TelegramConfig::default();
    let channel = TelegramChannel::new("telegram", config);
    channel_registry.register(Box::new(channel)).await;
}

// After (LinkManager):
let link_manager = Arc::new(LinkManager::new(
    channel_registry.clone(),
    dirs::home_dir().unwrap().join(".aleph"),
));
link_manager.start().await?;
```

### 4.7 Phase 1 Deliverables

| Deliverable | Location | Description |
|------------|----------|-------------|
| `BridgeDefinition` types | `core/src/gateway/bridge/` | bridge.yaml data model |
| `LinkConfig` types | `core/src/gateway/link/` | link.yaml data model |
| `Transport` trait | `core/src/gateway/transport/` | IPC abstraction layer |
| `UnixSocketTransport` | `core/src/gateway/transport/` | Extracted from WhatsApp RPC |
| `StdioTransport` | `core/src/gateway/transport/` | New stdin/stdout transport |
| `BridgedChannel` | `core/src/gateway/bridge/` | External bridge Channel proxy |
| `LinkManager` | `core/src/gateway/link_manager.rs` | Config scanning + instance orchestration |
| `BridgeSupervisor` | `core/src/gateway/bridge_supervisor.rs` | External process mgmt + heartbeat |
| YAML parsing | `core/src/gateway/link/` | serde_yaml + jsonschema validation |
| start.rs refactor | `core/src/bin/aleph_server/` | Remove hardcoding, integrate LinkManager |

---

## 5. Phase 2-4 Overview

### Phase 2: Standardization & SDK

- Publish complete ABS protocol documentation
- Extract Go SDK template from existing WhatsApp bridge (`aleph-bridge-go`)
- Create Python SDK template (`aleph-bridge-python`) based on Stdio transport
- Build `aleph bridge validate` CLI tool for bridge.yaml validation + mock handshake

### Phase 3: Visual Panel

- **Channel Control Center** page in Control Plane (Leptos WASM)
- Auto-generated config forms from `settings_schema` (zero frontend code for new plugins)
- QR Code pairing modal (WhatsApp/WeChat)
- Instance-level debug console (real-time message log stream)
- New RPC methods: `links.list`, `links.create`, `links.update`, `links.delete`, `bridges.list`, `bridges.schema`, `links.stream`

### Phase 4: Ecosystem Expansion

| Platform | Implementation | Priority | Rationale |
|----------|---------------|----------|-----------|
| **WeChat** | Python external bridge (gewechat) | P0 | Essential for Chinese users |
| **Slack** | Rust native (slack-morphism) | P1 | Workplace scenario |
| **Lark** | Rust native or Go bridge | P2 | Enterprise users |
| **LINE** | Go external bridge | P3 | Japan/SEA users |
| **Signal** | Go external bridge (signal-cli) | P3 | Privacy-focused users |

### Phase Timeline

```
Phase 1: Infrastructure     ██████████████  (foundation, largest effort)
Phase 2: Standardization     ████████        (docs + SDK)
Phase 3: Visual Panel         ██████████     (UI development)
Phase 4: Ecosystem              ████████████ (continuous expansion)
```

Phase 1 is the foundation. After completion, Phases 2-4 can proceed in parallel.

---

## 6. Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Config format | YAML (full migration) | Project not yet distributed; clean slate |
| IPC protocol | Unix Socket + JSON-RPC 2.0 | Proven by WhatsApp bridge; Stdio as alternative |
| Native vs External | Unified plugin interface | Transparent to users; native channels retain compile-time performance |
| Plugin model | Manifest-driven (bridge.yaml) | Inspired by LSP/MCP; enables community ecosystem |
| Settings UI | Auto-generated from JSON Schema | Zero frontend code for new plugins |
| Transport abstraction | `Transport` trait | Supports UnixSocket + Stdio; extensible to TCP |
