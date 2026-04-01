# Gateway Evolution P2: Smart Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connect the new hierarchical routing system (`resolve_route()`) to the InboundMessageRouter, add `/btw` sidebar conversations, and enhance Presence with connection roles.

**Architecture:** Replace `workspace_manager.get_active_agent(channel)` in `resolve_agent_id_async()` with `resolve_route()` for multi-tier resolution. AgentRouter stays for JSON-RPC `agent.run` handler (separate concern). `/btw` uses existing `SessionKey::Ephemeral` for zero-persistence sidebar.

**Tech Stack:** Existing `routing::resolve_route()`, `SessionKey::Ephemeral`, `PresenceTracker`

**Spec:** `docs/superpowers/specs/2026-03-25-gateway-evolution-design.md` (Phase 2)

---

## Key Discovery: Scope Correction

AgentRouter is NOT used in InboundMessageRouter. InboundRouter uses `workspace_manager.get_active_agent(channel)` (single-tier). P2 replaces this with `resolve_route()` (multi-tier: peer → guild → team → account → channel → default). AgentRouter cleanup is deferred — it's used by the JSON-RPC `agent.run` handler which is a separate subsystem.

## File Map

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `src/gateway/inbound_router/mod.rs` | Add route_bindings + session_config fields, builder method |
| Modify | `src/gateway/inbound_router/agent_resolver.rs` | Replace single-tier with resolve_route() |
| Modify | `src/gateway/inbound_router/command_handler.rs` | Add /btw handler |
| Modify | `src/gateway/inbound_router/executor.rs` | Pass workspace from ResolvedRoute |
| Modify | `src/gateway/presence.rs` | Add ConnectionRole enum to PresenceEntry |
| Modify | `src/gateway/server/handler.rs` | Set ConnectionRole on auth |
| Modify | `src/gateway/hello_snapshot.rs` | Serialize role field |
| Modify | `src/bin/aleph-server/commands/start/builder/subsystems.rs` | Wire route_bindings into InboundRouter |

---

### Task 1: Add resolve_route() to InboundRouter agent resolution

**Files:**
- Modify: `src/gateway/inbound_router/mod.rs` — add fields + builder
- Modify: `src/gateway/inbound_router/agent_resolver.rs` — new resolution logic

- [ ] **Step 1: Add route_bindings and session_config fields to InboundMessageRouter**

In `src/gateway/inbound_router/mod.rs`, add these imports at the top (after existing imports):

```rust
use crate::routing::config::{RouteBinding, SessionConfig};
```

Add two new fields to `InboundMessageRouter` struct (after `workspace_manager` at line 79):

```rust
    /// Route bindings for multi-tier agent resolution (peer → guild → channel → default)
    pub(super) route_bindings: Vec<RouteBinding>,
    /// Session configuration (dm_scope, identity_links)
    pub(super) route_session_config: SessionConfig,
    /// Default agent ID when no binding matches
    pub(super) default_agent_id: String,
```

Initialize them in `new()` and `with_execution()`:

```rust
            route_bindings: Vec::new(),
            route_session_config: SessionConfig::default(),
            default_agent_id: "main".to_string(),
```

Add builder method (after existing `with_*` methods):

```rust
    /// Set route bindings for multi-tier agent resolution
    pub fn with_route_bindings(
        mut self,
        bindings: Vec<RouteBinding>,
        session_config: SessionConfig,
        default_agent: impl Into<String>,
    ) -> Self {
        self.route_bindings = bindings;
        self.route_session_config = session_config;
        self.default_agent_id = default_agent.into();
        self
    }
```

- [ ] **Step 2: Replace resolve_agent_id_async() with multi-tier resolution**

Replace the entire `resolve_agent_id_async()` in `agent_resolver.rs` (lines 22-31):

```rust
    /// Resolve agent ID using multi-tier route bindings with workspace fallback.
    ///
    /// Priority: resolve_route(bindings) → workspace_manager → default_agent_id
    pub(super) async fn resolve_agent_id_async(
        &self,
        msg: &InboundMessage,
    ) -> Option<String> {
        // Tier 1: Try hierarchical route bindings (if configured)
        if !self.route_bindings.is_empty() {
            use crate::routing::{resolve_route, RouteInput, RoutePeer, RoutePeerKind};

            let peer = if msg.is_group {
                Some(RoutePeer {
                    kind: RoutePeerKind::Group,
                    id: msg.conversation_id.as_str().to_string(),
                })
            } else {
                Some(RoutePeer {
                    kind: RoutePeerKind::Dm,
                    id: msg.sender_id.as_str().to_string(),
                })
            };

            let input = RouteInput {
                channel: msg.channel_id.as_str().to_string(),
                account_id: None, // TODO: multi-account support
                peer,
                guild_id: None, // Channels that support guilds set this in InboundMessage metadata
                team_id: None,
            };

            let resolved = resolve_route(
                &self.route_bindings,
                &self.route_session_config,
                &self.default_agent_id,
                &input,
            );

            debug!(
                "Route resolved: channel='{}' → agent='{}' (matched_by={:?})",
                msg.channel_id.as_str(),
                resolved.agent_id,
                resolved.matched_by,
            );
            return Some(resolved.agent_id);
        }

        // Tier 2: Fallback to workspace_manager (backward compat for zero-config)
        let channel = msg.channel_id.as_str();
        if let Some(ref manager) = self.workspace_manager {
            if let Ok(Some(agent_id)) = manager.get_active_agent(channel) {
                debug!("Channel '{}' bound to agent '{}' via workspace", channel, agent_id);
                return Some(agent_id);
            }
        }

        // Tier 3: Default agent
        debug!("Channel '{}' using default agent '{}'", msg.channel_id.as_str(), self.default_agent_id);
        Some(self.default_agent_id.clone())
    }
```

Note: The function signature changes from `&self, channel: &str` to `&self, msg: &InboundMessage` to access peer info.

- [ ] **Step 3: Update the call site in handle_message()**

In `mod.rs`, the call at line 271 changes from:
```rust
        let agent_id = match self.resolve_agent_id_async(channel_id).await {
```
to:
```rust
        let agent_id = match self.resolve_agent_id_async(&msg).await {
```

Also, the `None` branch (lines 273-283) can be simplified since the new resolver always returns Some (default agent fallback). Replace the entire block:

```rust
        let agent_id = self.resolve_agent_id_async(&msg).await
            .unwrap_or_else(|| self.default_agent_id.clone());
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: Compiles (warnings OK)

- [ ] **Step 5: Commit**

```bash
git add src/gateway/inbound_router/
git commit -m "gateway: connect resolve_route() to InboundRouter with workspace fallback"
```

---

### Task 2: Add /btw sidebar conversation

**Files:**
- Modify: `src/gateway/inbound_router/mod.rs` — intercept /btw before command parser
- Modify: `src/gateway/inbound_router/command_handler.rs` — add handle_btw()

- [ ] **Step 1: Add handle_btw() to command_handler.rs**

Add this method to the `impl InboundMessageRouter` block in `command_handler.rs`:

```rust
    /// Handle /btw command: ephemeral sidebar conversation that doesn't affect context.
    ///
    /// Creates a SessionKey::Ephemeral so the question/answer is not persisted
    /// to the current session history.
    pub(super) async fn handle_btw(
        &self,
        msg: &InboundMessage,
        agent_id: &str,
        btw_text: &str,
    ) -> Result<(), RoutingError> {
        use crate::gateway::inbound_context::{InboundContext, ReplyRoute};

        let reply_route = ReplyRoute::new(
            msg.channel_id.clone(),
            msg.conversation_id.clone(),
        ).with_inbound_message_id(msg.id.clone());

        // Use ephemeral session — no persistence, no context pollution
        let session_key = SessionKey::ephemeral(agent_id);

        // Create a modified message with just the btw text
        let mut btw_msg = msg.clone();
        btw_msg.text = btw_text.to_string();

        let ctx = InboundContext::new(btw_msg, reply_route, session_key);

        // Execute with btw metadata marker
        let metadata = serde_json::json!({"btw": true}).to_string();
        self.execute_for_context_with_metadata(&ctx, metadata).await?;

        info!("[Router] /btw handled as ephemeral session for agent '{}'", agent_id);
        Ok(())
    }
```

- [ ] **Step 2: Intercept /btw in handle_message()**

In `mod.rs`, in the `handle_message()` method, add /btw interception BEFORE the existing slash command block (before line 444 `// Unified slash command interception`):

```rust
        // /btw sidebar: ephemeral question without affecting context
        if ctx.message.text.trim().starts_with("/btw ") || ctx.message.text.trim().starts_with("/btw\n") {
            let btw_text = ctx.message.text.trim().strip_prefix("/btw").unwrap_or("").trim();
            if !btw_text.is_empty() {
                return self.handle_btw(&msg, &agent_id, btw_text).await;
            }
        }
```

- [ ] **Step 3: Compile check and test**

Run: `cargo check -p alephcore`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add src/gateway/inbound_router/
git commit -m "gateway: add /btw ephemeral sidebar conversation command"
```

---

### Task 3: Add ConnectionRole to Presence

**Files:**
- Modify: `src/gateway/presence.rs` — add ConnectionRole enum + field
- Modify: `src/gateway/server/handler.rs` — set role on auth
- Modify: `src/gateway/hello_snapshot.rs` — update test helpers

- [ ] **Step 1: Add ConnectionRole enum and field to PresenceEntry**

In `src/gateway/presence.rs`, add before `PresenceEntry` struct:

```rust
/// Connection role for presence classification.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionRole {
    /// Panel UI or CLI user
    User,
    /// Mobile device bridge (nodes)
    Node,
    /// External webhook connection
    Webhook,
    /// Messaging channel (Telegram, Discord, etc.)
    Channel,
}

impl Default for ConnectionRole {
    fn default() -> Self {
        Self::User
    }
}
```

Add `role` field to `PresenceEntry` (after `platform`):

```rust
    /// Connection role (user, node, webhook, channel)
    #[serde(default)]
    pub role: ConnectionRole,
```

Update `make_entry` in tests to include `role: ConnectionRole::User`.

- [ ] **Step 2: Set role in handler.rs when creating PresenceEntry**

In `src/gateway/server/handler.rs`, wherever `PresenceEntry` is created (search for `PresenceEntry {`), add:

```rust
                                            role: crate::gateway::presence::ConnectionRole::User,
```

(For now, all WS connections default to User. Channel/Webhook/Node detection can be added later based on auth metadata.)

- [ ] **Step 3: Update hello_snapshot.rs test helper**

In the test `sample_snapshot()`, update the `PresenceEntry` construction to include `role`:

```rust
            presence: vec![PresenceEntry {
                conn_id: "conn-1".to_string(),
                device_id: None,
                device_name: "MacBook Pro".to_string(),
                platform: "macos".to_string(),
                role: crate::gateway::presence::ConnectionRole::User,
                connected_at: now,
                last_heartbeat: now,
            }],
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib presence -- --nocapture`
Run: `cargo test -p alephcore --lib hello_snapshot -- --nocapture`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add src/gateway/presence.rs src/gateway/server/handler.rs src/gateway/hello_snapshot.rs
git commit -m "gateway: add ConnectionRole to PresenceEntry for device classification"
```

---

### Task 4: Wire route_bindings from server startup

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs` — pass bindings to InboundRouter

- [ ] **Step 1: Pass route_bindings to InboundRouter builder**

In `subsystems.rs`, find `initialize_inbound_router()` function. Where the router is constructed via builder chain, add `.with_route_bindings()` call using bindings from `full_config`:

```rust
    // After existing builder chain, before .start()
    let router = router.with_route_bindings(
        full_config.bindings.clone(),
        crate::routing::config::SessionConfig::default(), // Will be configurable later
        default_agent.clone(),
    );
```

The exact location depends on the builder chain in this function. Read the file first to find the right insertion point.

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore && cargo check`
Expected: Full workspace compiles

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph-server/
git commit -m "gateway: wire route_bindings from config into InboundRouter"
```

---

### Task 5: Final validation

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass (except pre-existing 2 failures)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -W clippy::all`
Expected: No new warnings

- [ ] **Step 3: Final commit if needed**

```bash
git add -A && git commit -m "gateway: fix clippy warnings in P2 changes"
```
