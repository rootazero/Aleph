# Gateway System

> WebSocket control plane, JSON-RPC protocol, and multi-channel messaging

---

## Overview

The Gateway is Aleph's control plane, providing:
- WebSocket server for real-time communication
- JSON-RPC 2.0 protocol for structured requests
- Multi-channel message routing (Telegram, Discord, iMessage, CLI)
- Event distribution and streaming
- Session management and persistence

**Location**: `core/src/gateway/`

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Gateway Server                            │
│                     ws://127.0.0.1:18789                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │   Inbound    │     │   Handler    │     │   Outbound   │    │
│  │   Router     │ ──▶ │   Registry   │ ──▶ │   Emitter    │    │
│  │              │     │              │     │              │    │
│  │ • Parse req  │     │ • Route      │     │ • Stream     │    │
│  │ • Validate   │     │ • Execute    │     │ • Events     │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │   Session    │     │    Event     │     │   Channel    │    │
│  │   Manager    │     │     Bus      │     │   Registry   │    │
│  │              │     │              │     │              │    │
│  │ • SQLite     │     │ • Pub/Sub    │     │ • Telegram   │    │
│  │ • Compaction │     │ • Topics     │     │ • Discord    │    │
│  │ • History    │     │ • Subscribe  │     │ • iMessage   │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## JSON-RPC Protocol

### Message Format

**Request (Client → Gateway)**:
```json
{
  "jsonrpc": "2.0",
  "id": "uuid-xxx",
  "method": "agent.run",
  "params": {
    "message": "Hello",
    "session_key": "agent:main:main"
  }
}
```

**Response (Gateway → Client)**:
```json
{
  "jsonrpc": "2.0",
  "id": "uuid-xxx",
  "result": {
    "run_id": "run-123",
    "status": "running"
  }
}
```

**Event (Gateway → Client)**:
```json
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "topic": "stream.chunk",
    "data": {
      "run_id": "run-123",
      "content": "Hello! How can I help you?"
    }
  }
}
```

---

## RPC Methods

### Agent Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `agent.run` | Start agent execution | `message`, `session_key`, `thinking?`, `model?` |
| `agent.status` | Get run status | `run_id` |
| `agent.cancel` | Cancel running agent | `run_id` |
| `agent.abort` | Force abort | `run_id` |

### Session Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `session.get` | Get session info | `session_key` |
| `session.list` | List all sessions | `filter?` |
| `session.history` | Get message history | `session_key`, `limit?` |
| `session.compact` | Compress session | `session_key` |
| `session.delete` | Delete session | `session_key` |

### Config Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `config.get` | Get current config | - |
| `config.patch` | Partial update | `patch` (JSON Merge Patch) |
| `config.apply` | Full replace | `config` |
| `config.reload` | Reload from file | - |

### Event Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `events.subscribe` | Subscribe to topic | `pattern` (glob) |
| `events.unsubscribe` | Unsubscribe | `pattern` |
| `events.list` | List subscriptions | - |

### Memory Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `memory.store` | Store fact | `content`, `metadata?` |
| `memory.search` | Search facts | `query`, `limit?` |
| `memory.delete` | Delete fact | `fact_id` |
| `memory.stats` | Get statistics | - |

### Browser Methods (CDP)

| Method | Description | Parameters |
|--------|-------------|------------|
| `browser.navigate` | Go to URL | `url` |
| `browser.click` | Click element | `selector` |
| `browser.type` | Type text | `selector`, `text` |
| `browser.screenshot` | Take screenshot | `selector?` |
| `browser.evaluate` | Run JavaScript | `script` |

### Other Methods

| Domain | Methods |
|--------|---------|
| `auth.*` | `connect`, `pairing.approve`, `pairing.reject`, `devices.list` |
| `channel.*` | `status`, `config`, `login` |
| `mcp.*` | `start`, `stop`, `list`, `call` |
| `plugins.*` | `install`, `uninstall`, `list`, `enable`, `disable` |
| `skills.*` | `list`, `install`, `activate` |
| `runs.*` | `list`, `status`, `wait`, `queue` |
| `models.*` | `list`, `config` |
| `generation.*` | `image`, `video` |
| `cron.*` | `list`, `add`, `remove`, `run` |

---

## Event Topics

Subscribe to events using glob patterns:

| Pattern | Events |
|---------|--------|
| `stream.*` | All streaming events |
| `stream.chunk` | Text chunks |
| `stream.tool_start` | Tool execution start |
| `stream.tool_end` | Tool execution end |
| `agent.*` | Agent lifecycle events |
| `agent.started` | Run started |
| `agent.completed` | Run completed |
| `agent.error` | Run error |
| `session.*` | Session events |
| `config.*` | Configuration changes |

---

## Channels

**Location**: `core/src/gateway/channels/`

### Channel Trait

```rust
#[async_trait]
pub trait Channel: Send + Sync {
    fn name(&self) -> &str;

    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;

    async fn send_message(
        &self,
        target: &ChannelTarget,
        message: &str,
    ) -> Result<()>;

    fn is_running(&self) -> bool;
}
```

### Available Channels

| Channel | Feature Flag | Description |
|---------|--------------|-------------|
| CLI | `cli` | Command-line interface |
| Telegram | `telegram` | Telegram Bot API |
| Discord | `discord` | Discord Bot |
| iMessage | (macOS only) | Apple iMessage |
| WebChat | `gateway` | Built-in web chat |

### Channel Configuration

```json5
{
  "channels": {
    "telegram": {
      "token": "BOT_TOKEN",
      "allowFrom": ["+1234567890"],
      "groups": {
        "*": { "requireMention": true }
      }
    },
    "discord": {
      "token": "BOT_TOKEN",
      "guilds": ["guild-id-1"]
    }
  }
}
```

---

## Session Routing

**Location**: `core/src/routing/session_key.rs`

### Session Key Variants

| Variant | Format | Use Case |
|---------|--------|----------|
| **Main** | `agent:main:main` | Cross-channel shared session |
| **DirectMessage** | `agent:main:telegram:dm:user123` | Per-user DM |
| **Group** | `agent:main:discord:group:guild-id` | Group/channel chat |
| **Task** | `agent:main:cron:daily-summary` | Cron jobs, webhooks |
| **Subagent** | `subagent:agent:main:translator` | Sub-agent delegation |
| **Ephemeral** | `agent:main:ephemeral:uuid` | Temporary, no persistence |

### DM Scope Strategies

```rust
pub enum DmScope {
    Main,           // All DMs share main session
    PerPeer,        // Isolated per user (default)
    PerChannelPeer, // Isolated per channel + user
}
```

---

## Session Manager

**Location**: `core/src/gateway/session_manager.rs`

### Storage Schema

```sql
CREATE TABLE sessions (
    session_key TEXT PRIMARY KEY,
    messages TEXT,           -- JSON array
    created_at INTEGER,
    updated_at INTEGER,
    message_count INTEGER,
    token_count INTEGER
);

CREATE TABLE session_metadata (
    session_key TEXT PRIMARY KEY,
    agent_id TEXT,
    channel TEXT,
    last_compaction INTEGER
);
```

### Compaction

When session exceeds token threshold:

1. Extract key facts from old messages
2. Store facts in memory system
3. Replace old messages with summary
4. Update token count

---

## Security

**Location**: `core/src/gateway/security/`

### Authentication Flow

```
Client Connect
    │
    ▼
┌─────────────────────────────────┐
│ require_auth: true?             │
│   Yes → First frame must be     │
│         "connect" method        │
│   No  → Direct access allowed   │
└─────────────────────────────────┘
    │
    ▼ (if auth required)
┌─────────────────────────────────┐
│ Validate token / device pairing │
│   • Bearer token                │
│   • Device fingerprint          │
│   • Public key signature        │
└─────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────┐
│ Grant session token             │
│ Set client role (operator/node) │
└─────────────────────────────────┘
```

### Connect Request

```json
{
  "method": "connect",
  "params": {
    "minProtocol": 1,
    "maxProtocol": 1,
    "client": {
      "id": "macos-app",
      "version": "1.0.0",
      "platform": "macos"
    },
    "role": "operator",
    "auth": {
      "token": "bearer_token"
    }
  }
}
```

---

## Hot Reload

**Location**: `core/src/gateway/hot_reload.rs`

Configuration changes are detected via file watcher:

```
~/.aleph/config.json modified
    │
    ▼
┌─────────────────────────────────┐
│ Debounce (500ms)                │
└─────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────┐
│ Parse new config                │
│ Validate against schema         │
└─────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────┐
│ Apply changes                   │
│ • Restart affected channels     │
│ • Update routing rules          │
│ • Emit config.changed event     │
└─────────────────────────────────┘
```

---

## HTTP Server

**Location**: `core/src/gateway/http_server.rs`

Alongside WebSocket, Gateway serves:
- Static files (WebChat UI)
- Health check endpoint (`/health`)
- Metrics endpoint (`/metrics`)

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Agent System](AGENT_SYSTEM.md) - Agent loop
- [Security](SECURITY.md) - Exec approval system
