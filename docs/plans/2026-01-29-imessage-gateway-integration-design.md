# iMessage Gateway Integration Design

> Date: 2026-01-29
> Status: Draft
> Author: Claude + User

## Overview

This document describes the design for integrating iMessage channel with the Gateway control plane, enabling end-to-end message flow from iMessage to Agent execution and back.

## Problem Statement

Current Aleph iMessage implementation has:
- Basic SQLite polling for receiving messages
- AppleScript-based message sending
- Target parsing and configuration

**Missing**: The bridge between `ChannelRegistry.inbound_stream` and `ExecutionEngine`. Messages enter the unified stream but are never consumed or routed to Agents.

## Goals

1. Route inbound iMessage messages to correct Agent/Session
2. Implement permission control (allowlist, pairing)
3. Route Agent replies back to the originating conversation
4. Support both DM and group message scenarios

## Architecture

### High-Level Flow

```
┌─────────────────────────────────────────────────────────────┐
│                     Message Flow                             │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  iMessage DB ──► IMessageChannel ──► ChannelRegistry        │
│                                           │                 │
│                                     inbound_stream          │
│                                           │                 │
│                                           ▼                 │
│                               InboundMessageRouter          │
│                                    │         │              │
│                              [Permission] [SessionKey]      │
│                                    │         │              │
│                                    ▼         ▼              │
│                               ExecutionEngine               │
│                                      │                      │
│                                      ▼                      │
│                                 AgentLoop                   │
│                                      │                      │
│                                      ▼                      │
│                                ReplyEmitter                 │
│                                      │                      │
│                                      ▼                      │
│                          ChannelRegistry.send()             │
│                                      │                      │
│                                      ▼                      │
│                              IMessageChannel                │
│                                      │                      │
│                                      ▼                      │
│                               Messages.app                  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Component: InboundMessageRouter

Core component responsible for:

1. Consuming `ChannelRegistry.inbound_stream`
2. Permission checking (allowlist / pairing)
3. SessionKey resolution (DM vs Group)
4. Triggering `ExecutionEngine.run()`
5. Reply routing (send Agent output back to Channel)

```rust
pub struct InboundMessageRouter {
    channel_registry: Arc<ChannelRegistry>,
    execution_engine: Arc<ExecutionEngine>,
    pairing_store: Arc<dyn PairingStore>,
    routing_config: RoutingConfig,
}

impl InboundMessageRouter {
    pub async fn start(&self) {
        let mut rx = self.channel_registry.take_inbound_receiver().await.unwrap();

        while let Some(msg) = rx.recv().await {
            if let Err(e) = self.handle_message(msg).await {
                tracing::error!("Failed to handle message: {}", e);
            }
        }
    }

    async fn handle_message(&self, msg: InboundMessage) -> Result<()> {
        // 1. Build context
        let ctx = self.build_context(&msg).await?;

        // 2. Permission check
        if !self.check_permission(&ctx).await? {
            return Ok(()); // Handled (sent pairing code or ignored)
        }

        // 3. Create ReplyEmitter
        let emitter = ReplyEmitter::new(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
        );

        // 4. Execute Agent
        let request = RunRequest {
            run_id: uuid::Uuid::new_v4().to_string(),
            input: ctx.message.text.clone(),
            session_key: ctx.session_key.clone(),
            ..Default::default()
        };

        self.execution_engine.run(request, emitter).await?;

        Ok(())
    }
}
```

### Component: SessionKey Resolution

SessionKey format:
```
agent:{agent_id}:{channel}:{scope}:{identifier}
```

| Message Type | SessionKey Example | Description |
|-------------|-------------------|-------------|
| DM (per-peer) | `agent:main:imessage:dm:+15551234567` | Per-user isolated session |
| DM (main) | `agent:main:main` | All DMs share main session |
| Group | `agent:main:imessage:group:chat_id:42` | Per-group isolated session |

```rust
fn resolve_session_key(msg: &InboundMessage, config: &RoutingConfig) -> SessionKey {
    let agent_id = &config.default_agent; // "main"

    if msg.is_group {
        // Group message → isolate by chat_id
        SessionKey::new(format!(
            "agent:{}:imessage:group:{}",
            agent_id, msg.conversation_id
        ))
    } else {
        // DM → based on dm_scope config
        match config.dm_scope {
            DmScope::Main => SessionKey::new(format!("agent:{}:main", agent_id)),
            DmScope::PerPeer => SessionKey::new(format!(
                "agent:{}:imessage:dm:{}",
                agent_id, msg.sender_id
            )),
        }
    }
}
```

Configuration:
```json
{
  "session": {
    "dmScope": "per-peer"
  }
}
```

### Component: Permission Check & Pairing

**Permission Flow:**

```
InboundMessage arrives
        │
   [Is Group?]
        │
   ┌────┴────┐
   Yes       No (DM)
   │         │
[GroupPolicy]  [DmPolicy]
   │            │
   ├─disabled → ignore
   ├─allowlist → check group_allow_from
   └─open → pass (check mention)
            │
            ├─disabled → ignore
            ├─open → pass
            ├─allowlist → check allow_from
            └─pairing → check allow_from
                         │
                    ┌────┴────┐
                  in list   not in list
                    │         │
                  pass    [generate pairing code]
                              │
                         [send pairing message]
                              │
                         [wait for approve]
```

**Pairing Message Example:**
```
Hi! I'm Aleph, a personal AI assistant.

To chat with me, please have my owner approve your access.

Your iMessage ID: +15551234567
Pairing code: ABC123

Once approved, just send me a message!
```

**PairingStore Interface:**

```rust
pub struct PairingRequest {
    pub channel: String,           // "imessage"
    pub sender_id: String,         // "+15551234567"
    pub code: String,              // "ABC123"
    pub created_at: DateTime<Utc>,
    pub metadata: HashMap<String, String>,
}

#[async_trait]
pub trait PairingStore: Send + Sync {
    /// Create or get existing pairing request
    async fn upsert(&self, channel: &str, sender_id: &str) -> Result<(String, bool)>;

    /// Approve pairing request, add sender to allowlist
    async fn approve(&self, channel: &str, code: &str) -> Result<PairingRequest>;

    /// Reject pairing request
    async fn reject(&self, channel: &str, code: &str) -> Result<()>;

    /// List pending pairing requests
    async fn list_pending(&self, channel: Option<&str>) -> Result<Vec<PairingRequest>>;
}
```

**RPC Methods:**
- `pairing.list` - List pending requests
- `pairing.approve { channel, code }` - Approve
- `pairing.reject { channel, code }` - Reject

### Component: Reply Routing

**InboundContext** carries routing info through the entire flow:

```rust
pub struct InboundContext {
    // Original message
    pub message: InboundMessage,

    // Routing info (for reply)
    pub reply_route: ReplyRoute,

    // Session info
    pub session_key: SessionKey,

    // Permission info
    pub is_authorized: bool,
    pub is_mentioned: bool,
}

pub struct ReplyRoute {
    pub channel_id: ChannelId,
    pub conversation_id: ConversationId,
    pub reply_to: Option<MessageId>,
}
```

**ReplyEmitter** implements EventEmitter to route Agent output back:

```rust
pub struct ReplyEmitter {
    channel_registry: Arc<ChannelRegistry>,
    route: ReplyRoute,
    buffer: String,
}

impl EventEmitter for ReplyEmitter {
    async fn emit(&self, event: StreamEvent) {
        match event {
            StreamEvent::TextDelta { text, .. } => {
                self.buffer_and_maybe_send(text).await;
            }
            StreamEvent::RunEnd { .. } => {
                self.flush().await;
            }
            _ => {}
        }
    }
}
```

## File Structure

**New/Modified Files:**

```
core/src/gateway/
├── mod.rs                      # Add new module exports
├── inbound_router.rs           # [NEW] InboundMessageRouter
├── inbound_context.rs          # [NEW] InboundContext + ReplyRoute
├── reply_emitter.rs            # [NEW] ReplyEmitter implementation
├── pairing_store.rs            # [NEW] PairingStore trait + SQLite impl
│
├── handlers/
│   ├── mod.rs                  # Register new handlers
│   ├── pairing.rs              # [NEW] pairing.list/approve/reject
│   └── channel.rs              # Existing, no changes needed
│
└── channels/
    └── imessage/
        ├── mod.rs              # Modify: remove internal routing
        ├── db.rs               # Existing, minor enhancements
        ├── sender.rs           # Existing, add chat_id sending
        ├── config.rs           # Existing, no changes
        └── target.rs           # Existing, no changes
```

## Startup Sequence

```rust
// GatewayServer::run()
async fn run(&self) {
    // 1. Initialize ChannelRegistry
    let channel_registry = Arc::new(ChannelRegistry::new());

    // 2. Register Channel Factories
    channel_registry.register_factory(Arc::new(IMessageChannelFactory::new())).await;

    // 3. Create Channels from config
    for channel_config in &self.config.channels {
        channel_registry.create_channel(channel_config.clone()).await?;
    }

    // 4. Initialize InboundMessageRouter
    let router = InboundMessageRouter::new(
        channel_registry.clone(),
        self.execution_engine.clone(),
        self.pairing_store.clone(),
        self.config.routing.clone(),
    );

    // 5. Start Channels
    channel_registry.start_all().await;

    // 6. Start Router (consume inbound stream)
    tokio::spawn(async move {
        router.start().await;
    });

    // 7. Start WebSocket server...
}
```

## Implementation Plan

### Phase 1: Core Infrastructure
1. Create `InboundContext` and `ReplyRoute` types
2. Create `InboundMessageRouter` skeleton
3. Implement SessionKey resolution logic
4. Wire up with `ChannelRegistry.inbound_stream`

### Phase 2: Permission System
1. Create `PairingStore` trait and SQLite implementation
2. Implement permission checking logic
3. Implement pairing code generation and message sending
4. Add `pairing.*` RPC handlers

### Phase 3: Agent Integration
1. Create `ReplyEmitter` implementing `EventEmitter`
2. Integrate with `ExecutionEngine.run()`
3. Handle streaming replies and buffering
4. Test end-to-end message flow

### Phase 4: Group Support
1. Add mention detection for groups
2. Implement group history context (optional)
3. Test group message scenarios

## Future Enhancements

- **Inbound Debouncing**: Merge rapid consecutive messages (500ms window)
- **Reply Context**: Parse and include reply-to message content
- **Remote Host**: Support SSH to remote Mac for iMessage access
- **Markdown Tables**: Convert markdown tables to plain text for iMessage
