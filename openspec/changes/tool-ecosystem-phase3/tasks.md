# Tasks: Tool Ecosystem Phase 3

## 1. Message Tools

### 1.1 Core Infrastructure
- [ ] Define `MessageAction` enum (reply, edit, react, delete, send)
- [ ] Define `MessageOperations` trait with async methods
- [ ] Define `MessageToolParams` struct
- [ ] Define `MessageResult` struct

### 1.2 Channel Adapters
- [ ] Telegram `MessageOperations` implementation
  - [ ] `reply` - reply_to_message_id
  - [ ] `edit` - editMessageText
  - [ ] `react` - setMessageReaction
  - [ ] `delete` - deleteMessage
- [ ] Discord `MessageOperations` implementation
  - [ ] `reply` - message_reference
  - [ ] `edit` - PATCH /channels/{id}/messages/{id}
  - [ ] `react` - PUT /channels/{id}/messages/{id}/reactions/{emoji}
  - [ ] `delete` - DELETE /channels/{id}/messages/{id}
- [ ] iMessage `MessageOperations` implementation
  - [ ] `reply` - thread_originator_guid
  - [ ] `react` - tapback (associated_message_guid)

### 1.3 Tool Registration
- [ ] Create `MessageTool` struct implementing `AetherTool`
- [ ] Tool schema with JSON Schema parameters
- [ ] Register in builtin tool registry
- [ ] Unit tests for dispatch logic

## 2. Webhooks

### 2.1 Configuration
- [ ] Define `WebhookConfig` struct
- [ ] Add webhooks field to `GatewayConfig`
- [ ] Config validation (path uniqueness, valid agent refs)

### 2.2 HTTP Handler
- [ ] Create webhook router (Axum)
- [ ] Route registration: `POST /webhooks/{id}`
- [ ] Request parsing (headers, body)
- [ ] Health check endpoint

### 2.3 Security
- [ ] HMAC-SHA256 verification function
- [ ] Support multiple signature formats:
  - [ ] GitHub: `X-Hub-Signature-256`
  - [ ] Stripe: `Stripe-Signature`
  - [ ] Generic: `X-Webhook-Signature`
- [ ] Signature timing attack prevention

### 2.4 Agent Routing
- [ ] Session key template renderer
- [ ] Create `InboundMessage` from webhook
- [ ] Route to agent via `ExecutionAdapter`
- [ ] Return 200 OK immediately, process async

### 2.5 Testing
- [ ] Unit tests: HMAC verification
- [ ] Unit tests: session key rendering
- [ ] Integration test: GitHub webhook → agent

## 3. Canvas

### 3.1 Canvas Host Server
- [ ] Axum HTTP server on configurable port (default 18793)
- [ ] Routes:
  - [ ] `GET /__moltbot__/a2ui/*` - A2UI static assets
  - [ ] `GET /__moltbot__/canvas/*` - User content
  - [ ] `GET /` - Default index
- [ ] Static file serving with MIME detection
- [ ] Path traversal prevention

### 3.2 WebSocket Live Reload
- [ ] WebSocket upgrade handler at `/__moltbot/ws`
- [ ] File watcher (notify crate)
- [ ] Broadcast reload signal on file change
- [ ] Client-side script injection

### 3.3 Canvas Tool
- [ ] Define `CanvasAction` enum (7 variants)
- [ ] Define `CanvasTool` struct
- [ ] Tool schema registration
- [ ] Action handlers:
  - [ ] `present` - Show canvas with optional placement
  - [ ] `hide` - Hide canvas
  - [ ] `navigate` - Load URL
  - [ ] `eval` - Execute JavaScript
  - [ ] `snapshot` - Capture screenshot
  - [ ] `a2ui_push` - Push JSONL
  - [ ] `a2ui_reset` - Clear state

### 3.4 Node Integration
- [ ] Node RPC protocol for canvas commands
- [ ] macOS node: WKWebView canvas presenter
- [ ] Snapshot capture and base64 encoding

### 3.5 A2UI Protocol
- [ ] JSONL parser/validator (v0.8)
- [ ] userAction callback routing
- [ ] Surface management

### 3.6 Testing
- [ ] Unit tests: static file serving
- [ ] Unit tests: A2UI JSONL validation
- [ ] Integration test: canvas present → snapshot

## 4. Documentation

- [ ] Update CLAUDE.md with new tools
- [ ] Configuration examples
- [ ] Webhook integration guide

---

## Implementation Order

**Week 1: Message Tools**
1. Core trait and types
2. Telegram adapter (most complete)
3. Discord adapter
4. iMessage adapter (tapback only)
5. Tool registration and tests

**Week 2: Webhooks**
1. Config and handler
2. HMAC verification
3. Agent routing
4. Integration tests

**Week 3: Canvas**
1. Canvas Host HTTP server
2. Canvas Tool basics
3. A2UI integration
4. macOS node integration

---

## Acceptance Criteria

### Message Tools
- [ ] `message.reply` works on Telegram, Discord, iMessage
- [ ] `message.edit` works on Telegram, Discord
- [ ] `message.react` works on all channels (tapback for iMessage)
- [ ] Channel capability detection prevents unsupported actions

### Webhooks
- [ ] GitHub webhook triggers agent run
- [ ] Invalid signature returns 401
- [ ] Webhook returns 200 within 1 second

### Canvas
- [ ] Canvas Host serves static files on :18793
- [ ] `canvas.present` opens WKWebView on macOS
- [ ] `canvas.snapshot` returns PNG image
- [ ] `canvas.a2ui_push` renders A2UI components
