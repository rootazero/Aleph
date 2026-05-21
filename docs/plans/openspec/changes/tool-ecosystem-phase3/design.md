# Design: Tool Ecosystem Phase 3

## Overview

This phase implements three key tool systems to achieve Moltbot feature parity:

1. **Message Tools** - Unified messaging operations (reply, edit, react) across channels
2. **Webhooks** - External HTTP trigger system for integrations
3. **Canvas (A2UI)** - Visual rendering system with agent-driven UI

## 1. Message Tools

### Architecture

```
Agent Tool Call
      │
      ▼
┌─────────────────────────────────────────┐
│          MessageTool                     │
│  ┌─────────────────────────────────────┐│
│  │ action: reply|edit|react|send|...   ││
│  │ target: channel:conversation_id     ││
│  │ params: action-specific             ││
│  └─────────────────────────────────────┘│
└─────────────────┬───────────────────────┘
                  │
                  ▼
        ┌─────────────────┐
        │ ActionDispatcher│
        └────────┬────────┘
                 │
    ┌────────────┼────────────┐
    ▼            ▼            ▼
┌───────┐  ┌─────────┐  ┌─────────┐
│Telegram│  │ Discord │  │iMessage │
└───────┘  └─────────┘  └─────────┘
```

### Supported Actions

**Core Actions (Phase 3.1)**:
| Action | Telegram | Discord | iMessage | Slack |
|--------|----------|---------|----------|-------|
| reply  | ✅       | ✅      | ✅       | ✅    |
| edit   | ✅       | ✅      | ❌       | ✅    |
| react  | ✅       | ✅      | ✅ (tapback) | ✅    |
| delete | ✅       | ✅      | ❌       | ✅    |

**Extended Actions (Future)**:
- `send`, `broadcast`, `poll`, `pin`, `unpin`
- Thread operations: `thread_create`, `thread_reply`
- Moderation: `timeout`, `kick`, `ban`

### Data Types

```rust
/// Message action types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageAction {
    Reply,
    Edit,
    React,
    Delete,
    Send,
}

/// Parameters for message tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageToolParams {
    pub action: MessageAction,
    pub channel: Option<String>,
    pub target: String,              // channel:conversation_id
    pub message_id: Option<String>,  // For reply/edit/react/delete
    pub text: Option<String>,        // Message content
    pub emoji: Option<String>,       // For react
    pub remove: Option<bool>,        // Remove reaction
}

/// Channel-specific message operations
#[async_trait]
pub trait MessageOperations: Send + Sync {
    fn supported_actions(&self) -> Vec<MessageAction>;

    async fn reply(&self, params: ReplyParams) -> Result<MessageResult>;
    async fn edit(&self, params: EditParams) -> Result<MessageResult>;
    async fn react(&self, params: ReactParams) -> Result<()>;
    async fn delete(&self, params: DeleteParams) -> Result<()>;
}
```

### Implementation Plan

1. Define `MessageOperations` trait
2. Implement for each channel (Telegram, Discord, iMessage)
3. Create `MessageTool` that dispatches to channel adapters
4. Register in builtin tool registry

---

## 2. Webhooks

### Architecture

```
External Service (GitHub, Stripe, etc.)
              │
              ▼ HTTP POST
┌─────────────────────────────────────────┐
│           Webhook Handler                │
│  ┌─────────────────────────────────────┐│
│  │ Route: /webhooks/{id}               ││
│  │ Verify: HMAC signature              ││
│  │ Parse: JSON body                    ││
│  └─────────────────────────────────────┘│
└─────────────────┬───────────────────────┘
                  │
                  ▼
        ┌─────────────────┐
        │  AgentRouter    │
        │  (task session) │
        └────────┬────────┘
                 │
                 ▼
        ┌─────────────────┐
        │ ExecutionEngine │
        └─────────────────┘
```

### Configuration

```json
{
  "webhooks": [
    {
      "id": "github-push",
      "path": "/webhooks/github",
      "secret": "whsec_xxx",           // HMAC secret
      "agent": "main",
      "session_key_template": "task:webhook:{webhook_id}",
      "allowed_events": ["push", "pull_request"]
    },
    {
      "id": "stripe-payment",
      "path": "/webhooks/stripe",
      "secret": "whsec_yyy",
      "agent": "billing",
      "session_key_template": "task:webhook:stripe:{event_type}"
    }
  ]
}
```

### Data Types

```rust
/// Webhook configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub id: String,
    pub path: String,
    pub secret: Option<String>,
    pub agent: String,
    pub session_key_template: String,
    pub allowed_events: Option<Vec<String>>,
    pub enabled: bool,
}

/// Incoming webhook request
#[derive(Debug, Clone)]
pub struct WebhookRequest {
    pub webhook_id: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub timestamp: DateTime<Utc>,
}

/// Webhook handler result
#[derive(Debug, Clone, Serialize)]
pub struct WebhookResponse {
    pub accepted: bool,
    pub run_id: Option<String>,
    pub error: Option<String>,
}
```

### Security

1. **HMAC Verification**: `X-Hub-Signature-256` (GitHub), `Stripe-Signature`, etc.
2. **Immediate Response**: Return 200 OK immediately, process async
3. **Idempotency**: Track processed webhook IDs to prevent replay
4. **Rate Limiting**: Per-webhook request limits

### Implementation Plan

1. Add webhook routes to Gateway HTTP server
2. Implement HMAC verification (SHA256)
3. Create session key renderer
4. Route to agent via ExecutionEngine
5. Add webhook config to GatewayConfig

---

## 3. Canvas (A2UI)

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Canvas System                           │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────────┐  │
│  │ Canvas Tool │───▶│ Node RPC    │───▶│ Node (macOS/iOS)│  │
│  │ (7 actions) │    │ (Gateway)   │    │ WKWebView       │  │
│  └─────────────┘    └─────────────┘    └─────────────────┘  │
│                                                              │
│  ┌─────────────────────────────────────────────────────────┐│
│  │                    Canvas Host                           ││
│  │  HTTP: /__moltbot__/a2ui/*   (A2UI static assets)       ││
│  │  HTTP: /__moltbot__/canvas/* (user content)              ││
│  │  WS:   /__moltbot/ws         (live reload)               ││
│  └─────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────┘
```

### Canvas Tool Actions

| Action | Description | Parameters |
|--------|-------------|------------|
| `present` | Show canvas window | `url?`, `x?`, `y?`, `width?`, `height?` |
| `hide` | Hide canvas window | - |
| `navigate` | Load URL in canvas | `url` |
| `eval` | Execute JavaScript | `javascript` |
| `snapshot` | Capture screenshot | `format?`, `max_width?`, `quality?` |
| `a2ui_push` | Push A2UI JSONL | `jsonl` |
| `a2ui_reset` | Clear A2UI state | - |

### A2UI Protocol (v0.8)

**Server → Client (JSONL)**:
```jsonl
{"surfaceUpdate":{"surfaceId":"main","components":[...]}}
{"beginRendering":{"surfaceId":"main","root":"root"}}
{"dataModelUpdate":{"surfaceId":"main","updates":[...]}}
```

**Client → Server (userAction)**:
```json
{"userAction":{"name":"submit","surfaceId":"main","sourceComponentId":"btn-1","context":{...}}}
```

### Data Types

```rust
/// Canvas tool action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CanvasAction {
    Present {
        url: Option<String>,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
    },
    Hide,
    Navigate { url: String },
    Eval { javascript: String },
    Snapshot {
        format: Option<SnapshotFormat>,
        max_width: Option<u32>,
        quality: Option<f32>,
    },
    A2uiPush { jsonl: String },
    A2uiReset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotFormat {
    Png,
    Jpg,
    Jpeg,
}

/// Canvas host configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasHostConfig {
    pub port: u16,
    pub bind: String,
    pub root_dir: Option<PathBuf>,
    pub live_reload: bool,
}

impl Default for CanvasHostConfig {
    fn default() -> Self {
        Self {
            port: 18793,
            bind: "127.0.0.1".to_string(),
            root_dir: None,
            live_reload: true,
        }
    }
}
```

### Implementation Plan

1. **Canvas Host Server** (HTTP + WS)
   - Static file serving for A2UI assets
   - User content serving from root_dir
   - WebSocket for live reload

2. **Canvas Tool**
   - 7 actions implementation
   - Node RPC invocation
   - Snapshot file handling

3. **A2UI Integration**
   - JSONL validation
   - userAction callback routing

---

## Implementation Priority

### Phase 3.1: Message Tools (Core)
- [ ] `MessageOperations` trait
- [ ] Telegram adapter
- [ ] Discord adapter
- [ ] iMessage adapter (tapback only)
- [ ] Tool registration

### Phase 3.2: Webhooks
- [ ] Webhook config schema
- [ ] HTTP handler registration
- [ ] HMAC verification
- [ ] Agent routing
- [ ] Integration tests

### Phase 3.3: Canvas
- [ ] Canvas Host HTTP server
- [ ] Static file serving
- [ ] Canvas Tool (7 actions)
- [ ] A2UI JSONL validation
- [ ] macOS WKWebView integration

---

## Test Plan

### Message Tools
- Unit tests for each channel adapter
- Integration test: send/reply/edit cycle
- Channel capability detection

### Webhooks
- HMAC verification tests
- Session key template rendering
- Agent routing tests

### Canvas
- Canvas Host file serving
- A2UI JSONL parsing
- Tool action execution

---

## References

- **Moltbot Message Tool**: `/src/agents/tools/message-tool.ts`
- **Moltbot Webhooks**: `/src/telegram/webhook.ts`, `/src/line/webhook.ts`
- **Moltbot Canvas**: `/src/canvas-host/`, `/src/agents/tools/canvas-tool.ts`
- **A2UI Spec**: `vendor/a2ui/specification/0.8/`
