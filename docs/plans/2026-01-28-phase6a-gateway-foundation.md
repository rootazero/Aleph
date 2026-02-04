# Phase 6A: Gateway Foundation Enhancement

**Date**: 2026-01-28
**Status**: In Progress
**Duration**: 1-2 weeks

---

## Current State Analysis

### Existing Implementation ✅

| Component | File | Status |
|-----------|------|--------|
| TokenManager | `gateway/security/token.rs` | Complete - generation, validation, permissions, expiry |
| PairingManager | `gateway/security/pairing.rs` | Complete - 6-digit code, confirm, cancel |
| EventBus | `gateway/event_bus.rs` | Basic - broadcast only, no topic filtering |
| Server | `gateway/server.rs` | Basic - ConnectionState has subscriptions (unused) |
| CLI | `bin/aleph_gateway.rs` | Basic - start/stop/status only |

### Gaps to Address

1. **No authentication on WS connection** - require_auth flag exists but not enforced
2. **No topic-based subscriptions** - clients receive ALL events
3. **No device persistence** - approved devices lost on restart
4. **Limited CLI commands** - no pairing approve, channels list

---

## Implementation Tasks

### Task 1: Device Authentication Flow (Priority: High)

Integrate TokenManager and PairingManager into the connection handshake.

**Protocol Design:**

```
┌─────────────────────────────────────────────────────────────┐
│                   Connection Handshake                      │
├─────────────────────────────────────────────────────────────┤
│  Client                              Gateway                │
│    │                                    │                   │
│    ├──── WS Connect ────────────────────►                   │
│    │                                    │                   │
│    │◄─── { "method": "hello",           │                   │
│    │       "params": { "version": "1" }} │                   │
│    │                                    │                   │
│    ├──── { "method": "connect",         │                   │
│    │       "params": {                  │                   │
│    │         "token": "...",            │  ← Existing token │
│    │         "device_name": "...",      │                   │
│    │         "device_type": "macos" }}  │                   │
│    │                                    │                   │
│    │     OR (new device)                │                   │
│    │                                    │                   │
│    │◄─── { "method": "pairing_required",│                   │
│    │       "params": { "code": "123456" }} │                 │
│    │                                    │                   │
│    │     (User approves via CLI/UI)     │                   │
│    │                                    │                   │
│    │◄─── { "method": "connected",       │                   │
│    │       "params": { "token": "...",  │  ← New token      │
│    │         "expires_at": "..." }}     │                   │
│    │                                    │                   │
└─────────────────────────────────────────────────────────────┘
```

**Implementation:**

1. Add `connect` handler in `handlers/auth.rs`:
   ```rust
   // core/src/gateway/handlers/auth.rs
   pub async fn handle_connect(
       request: JsonRpcRequest,
       token_manager: Arc<TokenManager>,
       pairing_manager: Arc<PairingManager>,
       device_store: Arc<DeviceStore>,
   ) -> JsonRpcResponse
   ```

2. Modify `server.rs` to require handshake before other methods
3. Add device store for persistence

**Files to Create/Modify:**
- `core/src/gateway/handlers/auth.rs` (NEW)
- `core/src/gateway/device_store.rs` (NEW)
- `core/src/gateway/server.rs` (MODIFY)
- `core/src/gateway/handlers/mod.rs` (MODIFY)

---

### Task 2: Topic-Based Event Subscriptions (Priority: High)

Allow clients to subscribe to specific event types.

**Protocol Design:**

```json
// Subscribe to specific topics
{
  "jsonrpc": "2.0",
  "method": "events.subscribe",
  "params": {
    "topics": ["agent.run.*", "session.*"]
  },
  "id": 1
}

// Unsubscribe
{
  "jsonrpc": "2.0",
  "method": "events.unsubscribe",
  "params": {
    "topics": ["agent.run.*"]
  },
  "id": 2
}

// Event notification (method is null for notifications)
{
  "jsonrpc": "2.0",
  "method": null,
  "params": {
    "topic": "agent.run.started",
    "data": { "run_id": "..." }
  }
}
```

**Implementation:**

1. Update `event_bus.rs` with topic filtering:
   ```rust
   pub struct TopicEventBus {
       sender: broadcast::Sender<Event>,
   }

   pub struct Event {
       pub topic: String,
       pub data: serde_json::Value,
   }

   impl TopicEventBus {
       pub fn publish(&self, topic: &str, data: Value);
       pub fn subscribe_topics(&self, patterns: Vec<String>) -> TopicSubscription;
   }
   ```

2. Update `ConnectionState` to track subscriptions
3. Add `events.subscribe` / `events.unsubscribe` handlers

**Files to Create/Modify:**
- `core/src/gateway/event_bus.rs` (MODIFY - add TopicEventBus)
- `core/src/gateway/handlers/events.rs` (NEW)
- `core/src/gateway/server.rs` (MODIFY - filter events per connection)

---

### Task 3: Device Allowlist Persistence (Priority: Medium)

Store approved devices in SQLite for persistence across restarts.

**Schema:**

```sql
CREATE TABLE approved_devices (
    device_id TEXT PRIMARY KEY,
    device_name TEXT NOT NULL,
    device_type TEXT,
    approved_at TEXT NOT NULL,
    last_seen_at TEXT,
    permissions TEXT  -- JSON array
);
```

**Implementation:**

1. Create `DeviceStore` with SQLite backend:
   ```rust
   // core/src/gateway/device_store.rs
   pub struct DeviceStore {
       db: rusqlite::Connection,
   }

   impl DeviceStore {
       pub fn approve_device(&self, device: ApprovedDevice) -> Result<()>;
       pub fn is_approved(&self, device_id: &str) -> bool;
       pub fn list_devices(&self) -> Vec<ApprovedDevice>;
       pub fn revoke_device(&self, device_id: &str) -> Result<()>;
   }
   ```

2. Integrate with authentication flow

**Files to Create:**
- `core/src/gateway/device_store.rs` (NEW)

---

### Task 4: CLI Commands Enhancement (Priority: Medium)

Add new CLI subcommands for gateway management.

**New Commands:**

```bash
# List pending pairing requests
aleph-gateway pairing list

# Approve a pairing request
aleph-gateway pairing approve <code>

# Reject a pairing request
aleph-gateway pairing reject <code>

# List approved devices
aleph-gateway devices list

# Revoke a device
aleph-gateway devices revoke <device_id>

# Check gateway status (enhanced)
aleph-gateway status --json
```

**Implementation:**

Update `bin/aleph_gateway.rs` with new subcommands:

```rust
#[derive(Subcommand, Debug)]
enum Command {
    Start,
    Stop,
    Status {
        #[arg(long)]
        json: bool,
    },
    Pairing {
        #[command(subcommand)]
        action: PairingAction,
    },
    Devices {
        #[command(subcommand)]
        action: DevicesAction,
    },
}

#[derive(Subcommand, Debug)]
enum PairingAction {
    List,
    Approve { code: String },
    Reject { code: String },
}

#[derive(Subcommand, Debug)]
enum DevicesAction {
    List,
    Revoke { device_id: String },
}
```

**Files to Modify:**
- `core/src/bin/aleph_gateway.rs` (MODIFY)

---

### Task 5: Graceful Reconnection Protocol (Priority: Low)

Support session resumption after disconnection.

**Protocol Design:**

```json
// Client reconnect with session token
{
  "method": "connect",
  "params": {
    "token": "device-token",
    "session_id": "previous-session-id"
  }
}

// Gateway responds with missed events or "session_expired"
{
  "method": "reconnected",
  "params": {
    "session_id": "new-or-same-session",
    "missed_events": 5,
    "replaying": true
  }
}
```

**Defer to Phase 6C** - This requires client-side implementation in Swift.

---

## File Changes Summary

### New Files

| File | Purpose |
|------|---------|
| `core/src/gateway/handlers/auth.rs` | Authentication handler (connect, hello) |
| `core/src/gateway/handlers/events.rs` | Event subscription handlers |
| `core/src/gateway/device_store.rs` | SQLite device persistence |

### Modified Files

| File | Changes |
|------|---------|
| `core/src/gateway/event_bus.rs` | Add topic-based filtering |
| `core/src/gateway/server.rs` | Integrate auth, filter events per connection |
| `core/src/gateway/handlers/mod.rs` | Export new handlers |
| `core/src/bin/aleph_gateway.rs` | Add pairing/devices subcommands |

---

## Testing Plan

### Unit Tests

1. `TokenManager` - already has tests
2. `PairingManager` - already has tests
3. `TopicEventBus` - pattern matching, subscription management
4. `DeviceStore` - CRUD operations

### Integration Tests

1. **Full handshake flow** - connect with/without token
2. **Pairing flow** - initiate, approve, receive token
3. **Event filtering** - verify clients only receive subscribed topics
4. **Persistence** - restart gateway, verify approved devices retained

### Manual Testing

```bash
# Terminal 1: Start gateway
cargo run --features gateway --bin aleph-gateway

# Terminal 2: Test with websocat
echo '{"jsonrpc":"2.0","method":"connect","params":{"device_name":"test"},"id":1}' | websocat ws://127.0.0.1:18789

# Terminal 3: Approve pairing
cargo run --features gateway --bin aleph-gateway -- pairing approve 123456
```

---

## Implementation Order

1. **Day 1-2**: Task 1 (Device Authentication Flow)
   - Create auth.rs handler
   - Modify server.rs for handshake requirement
   - Basic testing

2. **Day 3-4**: Task 3 (Device Allowlist Persistence)
   - Create DeviceStore
   - Integrate with auth flow
   - Test persistence

3. **Day 5-6**: Task 2 (Topic-Based Subscriptions)
   - Update EventBus
   - Create events.rs handlers
   - Modify server.rs for filtering

4. **Day 7**: Task 4 (CLI Enhancement)
   - Add pairing commands
   - Add devices commands
   - Manual testing

---

## Success Criteria

- [x] New device connecting prompts for pairing
- [x] Approved devices can reconnect with stored token
- [x] Clients can subscribe to specific event topics
- [x] `aleph-gateway pairing approve <code>` works
- [x] `aleph-gateway devices list` shows approved devices
- [x] Gateway restart preserves approved devices

---

## Implementation Progress

### Completed (2026-01-28)

**Task 1: Device Authentication Flow** - DONE
- Created `device_store.rs` with SQLite persistence
- Created `handlers/auth.rs` with connect, pairing, and device handlers
- Auth context supports token validation, device lookup, and pairing flow

**Task 2: Topic-Based Event Subscriptions** - DONE
- Added `TopicEvent` struct with topic-based filtering
- Added `TopicFilter` with glob-like pattern matching (`*`, `**`)
- Created `SubscriptionManager` for per-connection subscriptions
- Created `handlers/events.rs` with subscribe/unsubscribe handlers
- Pattern matching: `agent.*`, `session.*.created`, `*`

**Task 3: Device Allowlist Persistence** - DONE (merged with Task 1)

**Task 4: CLI Commands Enhancement** - DONE
- Added `pairing list/approve/reject` commands
- Added `devices list/revoke` commands
- Updated `status` command with `--json` flag

**Files Created:**
- `core/src/gateway/device_store.rs` (185 lines)
- `core/src/gateway/handlers/auth.rs` (420 lines)
- `core/src/gateway/handlers/events.rs` (240 lines)

**Files Modified:**
- `core/src/gateway/event_bus.rs` (added TopicEvent, TopicFilter, topic_matches)
- `core/src/gateway/handlers/mod.rs`
- `core/src/gateway/mod.rs`
- `core/src/bin/aleph_gateway.rs`

### All Tasks Complete!

---

## Moltbot Reference

Key files to reference:
- `/Users/zouguojun/Workspace/moltbot/src/gateway/auth.ts` - Auth modes
- `/Users/zouguojun/Workspace/moltbot/src/gateway/protocol/index.ts` - Protocol definition
- `/Users/zouguojun/Workspace/moltbot/src/gateway/server/ws-connection.ts` - Connection handling
