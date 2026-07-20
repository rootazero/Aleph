# Agent-Bot 1:1 Binding Simplification — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Simplify agent-bot binding from dynamic multi-agent switching to strict 1:1 channel-agent binding managed exclusively through Panel.

**Architecture:** Reuse `channel_active_agent` table, keep `peer_id` as-is (actual sender_id). Add 1:1 constraint. Simplify three-tier agent resolution to single-tier channel lookup. Remove all in-conversation switching mechanisms (tool, slash command, natural language intent). Remove agent name prefix from replies.

**Tech Stack:** Rust (core), Leptos/WASM (panel), SQLite (data layer)

**Spec:** `docs/superpowers/specs/2026-03-19-agent-bot-1to1-binding-design.md`

---

### Task 1: Add 1:1 constraint and reverse lookup to WorkspaceManager

Method signatures stay unchanged (3 params with `peer_id`). Add 1:1 constraint check, reverse lookup, and bulk bindings query.

**Files:**
- Modify: `src/gateway/workspace/manager_ops.rs:269-306`
- Modify: `src/gateway/workspace/mod.rs` (WorkspaceError enum)

- [ ] **Step 1: Add `AgentAlreadyBound` variant to `WorkspaceError`**

In `workspace/mod.rs`, find `enum WorkspaceError` and add:
```rust
AgentAlreadyBound { agent_id: String, channel: String },
```
Also add corresponding `Display` impl arm.

- [ ] **Step 2: Add 1:1 constraint to `set_active_agent`**

In `manager_ops.rs`, add constraint check at the top of `set_active_agent` (before the INSERT):

```rust
pub fn set_active_agent(&self, channel: &str, peer_id: &str, agent_id: &str) -> Result<(), WorkspaceError> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    let now = Utc::now().timestamp();

    // 1:1 constraint: check if agent is already bound to another channel
    let existing: Option<String> = conn.prepare(
        "SELECT channel FROM channel_active_agent WHERE agent_id = ?1 AND channel != ?2"
    ).map_err(|e| WorkspaceError::Database(e.to_string()))?
    .query_row(params![agent_id, channel], |row| row.get(0))
    .optional()
    .map_err(|e| WorkspaceError::Database(e.to_string()))?;

    if let Some(occupied_channel) = existing {
        return Err(WorkspaceError::AgentAlreadyBound {
            agent_id: agent_id.to_string(),
            channel: occupied_channel,
        });
    }

    conn.execute(
        "INSERT INTO channel_active_agent (channel, peer_id, agent_id, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(channel, peer_id) DO UPDATE SET agent_id = ?3, updated_at = ?4",
        params![channel, peer_id, agent_id, now],
    ).map_err(|e| WorkspaceError::Database(e.to_string()))?;
    Ok(())
}
```

Note: `.optional()` requires `use rusqlite::OptionalExtension;` — add import if not present.

- [ ] **Step 3: Add `get_channel_for_agent` reverse lookup**

```rust
/// Reverse lookup: which channel is this agent bound to?
pub fn get_channel_for_agent(&self, agent_id: &str) -> Result<Option<String>, WorkspaceError> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    let mut stmt = conn.prepare(
        "SELECT channel FROM channel_active_agent WHERE agent_id = ?1 LIMIT 1"
    ).map_err(|e| WorkspaceError::Database(e.to_string()))?;
    let result = stmt.query_row(params![agent_id], |row| row.get::<_, String>(0));
    match result {
        Ok(ch) => Ok(Some(ch)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(WorkspaceError::Database(e.to_string())),
    }
}
```

- [ ] **Step 4: Add `get_all_agent_bindings` for Panel**

```rust
/// Get all agent→channel bindings (for Panel agents.bindings RPC).
pub fn get_all_agent_bindings(&self) -> Result<std::collections::HashMap<String, String>, WorkspaceError> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    let mut stmt = conn.prepare(
        "SELECT DISTINCT agent_id, channel FROM channel_active_agent"
    ).map_err(|e| WorkspaceError::Database(e.to_string()))?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }).map_err(|e| WorkspaceError::Database(e.to_string()))?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (agent_id, channel) = row.map_err(|e| WorkspaceError::Database(e.to_string()))?;
        map.insert(agent_id, channel);
    }
    Ok(map)
}
```

- [ ] **Step 5: Run `cargo check -p alephcore`**

Expected: Clean compile — no signature changes, so no downstream breakage.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/workspace/manager_ops.rs src/gateway/workspace/mod.rs
git commit -m "workspace: add 1:1 binding constraint and reverse lookup"
```

---

### Task 2: Simplify agent resolution to single-tier

Replace three-tier resolution with direct channel config lookup.

**Files:**
- Modify: `src/gateway/inbound_router/agent_resolver.rs`

- [ ] **Step 1: Replace `resolve_agent_id_async` with single-tier lookup**

Replace the entire `resolve_agent_id_async` method:

```rust
/// Resolve agent ID from channel binding (single-tier).
///
/// Looks up the 1:1 channel-agent binding. Returns None if unbound.
pub(super) async fn resolve_agent_id_async(&self, channel: &str, sender_id: &str) -> Option<String> {
    if let Some(ref manager) = self.workspace_manager {
        if let Ok(Some(agent_id)) = manager.get_active_agent(channel, sender_id) {
            debug!("Channel '{}' bound to agent '{}'", channel, agent_id);
            return Some(agent_id);
        }
    }
    debug!("Channel '{}' has no agent binding", channel);
    None
}
```

Note: Return type changes from `String` to `Option<String>`. This requires updating the caller in `mod.rs` to handle `None` by sending the fixed "unbound" message.

- [ ] **Step 2: Remove `AgentRouter` import and field usage**

In `agent_resolver.rs`, remove the `AgentRouter` reference (layers 2 and 3 of the old resolution). The `agent_router` field on `InboundMessageRouter` can stay for now (unused fields produce warnings, not errors).

- [ ] **Step 3: Update the caller in `mod.rs` to handle unbound channels**

Find where `resolve_agent_id_async` is called in `src/gateway/inbound_router/mod.rs`. Update to handle `None`:

```rust
// Replace:
let agent_id = self.resolve_agent_id_async(&channel, &sender_id).await;
// With:
let agent_id = match self.resolve_agent_id_async(&channel, &sender_id).await {
    Some(id) => id,
    None => {
        // Unbound channel — send fixed message
        let reply = OutboundMessage::text(
            msg.conversation_id.as_str(),
            "此频道未绑定 Agent，请在 Panel 中配置",
        );
        if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
            error!("[Router] Failed to send unbound-channel message: {}", e);
        }
        return Ok(());
    }
};
```

- [ ] **Step 4: Run `cargo check -p alephcore`**

Expected: May see warnings about unused `agent_router` field. Errors from switch_intent and other deleted modules are expected (fixed in later tasks).

- [ ] **Step 5: Commit**

```bash
git add src/gateway/inbound_router/agent_resolver.rs src/gateway/inbound_router/mod.rs
git commit -m "routing: simplify agent resolution to single-tier channel binding"
```

---

### Task 3: Delete switch tool, intent detector, and /switch command

Remove all in-conversation agent switching mechanisms.

**Files:**
- Delete: `src/builtin_tools/agent_manage/switch.rs`
- Delete: `src/gateway/intent_detector.rs`
- Delete: `src/gateway/inbound_router/switch_intent.rs`
- Modify: `src/builtin_tools/agent_manage/mod.rs` — remove `pub mod switch` and re-exports
- Modify: `src/gateway/mod.rs` — remove `pub mod intent_detector` and re-export
- Modify: `src/gateway/inbound_router/mod.rs` — remove `intent_detector` field, `with_intent_detector()`, and `try_handle_switch_intent()` call
- Modify: `src/gateway/inbound_router/command_handler.rs` — remove `/switch` handler
- Modify: `src/bin/aleph/commands/start/builder/subsystems.rs` — remove IntentDetector wiring block
- Modify: `src/executor/builtin_registry/registry.rs` — remove `agent_switch_tool` field and match arm
- Modify: `src/executor/builtin_registry/builder.rs` — remove `agent_switch_tool` construction
- Modify: `src/executor/builtin_registry/definitions.rs` — remove `agent_switch` definition
- Modify: `src/executor/builtin_registry/groups.rs` — remove `agent_switch` from group

- [ ] **Step 1: Delete switch.rs file**

```bash
rm src/builtin_tools/agent_manage/switch.rs
```

- [ ] **Step 2: Delete intent_detector.rs file**

```bash
rm src/gateway/intent_detector.rs
```

- [ ] **Step 3: Delete switch_intent.rs file**

```bash
rm src/gateway/inbound_router/switch_intent.rs
```

- [ ] **Step 4: Update `builtin_tools/agent_manage/mod.rs`**

Remove `pub mod switch;` line and `pub use switch::...` re-export line.

- [ ] **Step 5: Update `gateway/mod.rs`**

Remove `pub mod intent_detector;` and any `pub use intent_detector::...` re-exports (search for `IntentDetector` in the re-exports).

- [ ] **Step 6: Update `gateway/inbound_router/mod.rs`**

Remove:
- `pub(super) intent_detector: Option<IntentDetector>` field (line ~92)
- `intent_detector: None` in constructors
- `pub fn with_intent_detector(...)` builder method (lines ~187-191)
- The `try_handle_switch_intent` call block (lines ~361-364):
  ```rust
  // DELETE THIS BLOCK:
  if let Some(result) = self.try_handle_switch_intent(&msg).await {
      return result;
  }
  ```
- `mod switch_intent;` declaration if present
- Any import of `IntentDetector`

- [ ] **Step 7: Update `command_handler.rs` — remove /switch handler**

Remove `handle_switch_command` method entirely. Remove any match arm dispatching to it (search for `"switch"` in the command dispatch logic).

- [ ] **Step 8: Update `subsystems.rs` — remove IntentDetector wiring**

In `src/bin/aleph/commands/start/builder/subsystems.rs`, remove the entire block (lines ~337-362):
```rust
// Wire intent detector for natural language agent switching (LLM-based)
{
    use alephcore::gateway::IntentDetector;
    ...
    inbound_router = inbound_router.with_intent_detector(detector);
    ...
}
```

- [ ] **Step 9: Update builtin registry — remove agent_switch_tool**

In `registry.rs`:
- Remove `pub(crate) agent_switch_tool: Option<...>` field (line ~73)
- Remove `"agent_switch"` match arm in `call_tool` (lines ~374-379)

In `builder.rs`:
- Remove `AgentSwitchTool::new(...)` construction (line ~182-184)
- Update the destructuring tuple to remove `agent_switch_tool`
- Remove `agent_switch_tool` from the struct construction

In `definitions.rs`:
- Remove the `BuiltinToolDefinition` entry for `"agent_switch"` (lines ~154-158)
- Remove `"agent_switch"` from the match arm (line ~306)

In `groups.rs`:
- Remove `"agent_switch"` from the `"Agent 管理"` group (line ~73)

- [ ] **Step 10: Run `cargo check -p alephcore`**

Expected: Should compile. Fix any remaining references to deleted types.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "cleanup: remove agent switching (tool, intent detector, /switch command)"
```

---

### Task 4: Remove agent name prefix from replies

Delete `apply_agent_prefix` and related fields.

**Files:**
- Modify: `src/gateway/reply_emitter.rs`
- Modify: `src/gateway/channel.rs`
- Modify: `src/gateway/inbound_router/executor.rs`

- [ ] **Step 1: Remove `apply_agent_prefix` function and `format_content` prefix logic**

In `reply_emitter.rs`:

Delete the `apply_agent_prefix` function (lines 90-95).

Simplify `format_content` to just return the content:
```rust
fn format_content(&self, content: &str, _is_first: bool) -> String {
    content.to_string()
}
```

Remove `agent_display_name` and `native_identity` fields from the `ReplyEmitter` struct (lines ~79-80).

Update `new()` and `with_config()` constructors — remove `agent_display_name` and `native_identity` parameters. Remove corresponding field assignments.

- [ ] **Step 2: Remove `agent_display_name` from `OutboundMessage`**

In `channel.rs`, remove `pub agent_display_name: Option<String>` from `OutboundMessage` struct (line ~255). Remove `agent_display_name: None` from `Default` or constructor. Remove `self.agent_display_name.clone()` from all `OutboundMessage` constructions in `reply_emitter.rs`.

- [ ] **Step 3: Update `executor.rs` — remove display_name param**

In `executor.rs` (line ~90), remove `agent.config().display_name.clone()` from the `ReplyEmitter::with_config(...)` call. Also remove the `false` (native_identity) argument.

- [ ] **Step 4: Remove related tests**

In `reply_emitter.rs`, delete:
- `test_apply_agent_prefix_with_name`
- `test_apply_agent_prefix_none`
- `test_apply_agent_prefix_empty_name`
- `test_apply_agent_prefix_chinese_name`

In `channel.rs`, delete:
- `test_outbound_message_agent_display_name`

- [ ] **Step 5: Run `cargo check -p alephcore`**

Expected: Clean compile. Fix any remaining references to removed fields.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/reply_emitter.rs src/gateway/channel.rs src/gateway/inbound_router/executor.rs
git commit -m "reply: remove agent name prefix from bot messages"
```

---

### Task 5: Update agent_create, agent_delete, agent_list tools

Adapt remaining agent management tools for the new 1:1 model.

**Files:**
- Modify: `src/builtin_tools/agent_manage/create.rs`
- Modify: `src/builtin_tools/agent_manage/delete.rs`
- Modify: `src/builtin_tools/agent_manage/list.rs`

- [ ] **Step 1: `create.rs` — remove auto-switch on create**

Find the auto-switch block (lines ~356-363) and remove it entirely:
```rust
// DELETE THIS BLOCK:
let switched = if !channel.is_empty() && !peer_id.is_empty() {
    self.workspace_mgr
        .set_active_agent(&channel, &peer_id, &args.id)
        .map(|_| true)
        .unwrap_or(false)
} else {
    false
};
```

Also remove any reference to `switched` in the output message construction.

- [ ] **Step 2: `delete.rs` — unbind on delete using `get_channel_for_agent`**

Replace the old "if active, switch to main" logic (lines ~114-132) with:
```rust
// Unbind agent from its channel if bound
if let Ok(Some(bound_channel)) = self.workspace_mgr.get_channel_for_agent(&args.agent_id) {
    let _ = self.workspace_mgr.clear_active_agent(&bound_channel, &peer_id);
}
```

- [ ] **Step 3: `list.rs` — show binding status instead of per-peer active**

Replace the active agent lookup (lines ~124-131):
```rust
// OLD: get per-peer active
let active_agent = if !channel.is_empty() && !peer_id.is_empty() { ... }
```

With binding info per agent:
```rust
let bindings = self.workspace_mgr
    .get_all_agent_bindings()
    .unwrap_or_default();
```

Then when building each agent's output, replace `is_active` with `bound_channel`:
```rust
// For each agent_id in the list:
let bound_channel = bindings.get(agent_id).cloned();
```

Update `AgentListInfo` struct to replace `is_active: bool` with `bound_channel: Option<String>`.

- [ ] **Step 4: Run `cargo check -p alephcore`**

Expected: Clean compile. Also run `cargo test -p alephcore --lib` to check for test failures.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/agent_manage/create.rs src/builtin_tools/agent_manage/delete.rs src/builtin_tools/agent_manage/list.rs
git commit -m "agent tools: adapt create/delete/list for 1:1 binding model"
```

---

### Task 6: Remove AgentLifecycleEvent::Switched and clean up AgentRouter

Remove dead event variant. Clean up unused AgentRouter references.

**Files:**
- Modify: `src/gateway/agent_lifecycle.rs`
- Modify: `src/gateway/inbound_router/mod.rs` — remove `agent_router` field and builder/constructor refs
- Modify: `src/bin/aleph/commands/start/builder/subsystems.rs` — remove AgentRouter wiring if present

- [ ] **Step 1: Remove `Switched` variant from `AgentLifecycleEvent`**

In `agent_lifecycle.rs`, remove:
```rust
/// Active agent was switched for a session
Switched {
    agent_id: String,
    channel: String,
    peer_id: String,
    previous_agent_id: String,
},
```

And remove the corresponding `topic()` match arm:
```rust
Self::Switched { .. } => "agent.lifecycle.switched",
```

- [ ] **Step 2: Clean up `agent_router` from `InboundMessageRouter`**

In `mod.rs`:
- Remove `pub(super) agent_router: Option<Arc<AgentRouter>>` field (line ~80)
- Remove `agent_router: None` from constructors
- Remove `with_agent_router()` builder method (lines ~165-168)
- Remove `with_unified_routing()` constructor (lines ~148-162) or simplify to not take `agent_router`
- Remove `AgentRouter` import

- [ ] **Step 3: Remove AgentRouter wiring in subsystems.rs**

Search for `agent_router` or `AgentRouter` in `subsystems.rs` and remove any wiring code.

- [ ] **Step 4: Run `cargo check -p alephcore` and `cargo test -p alephcore --lib`**

Expected: Clean compile and passing tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "cleanup: remove AgentLifecycleEvent::Switched and AgentRouter from inbound router"
```

---

### Task 7: Remove workspace.switch and workspace.getActive RPC handlers

Replace with new `channels.set_agent` and `agents.bindings` RPCs.

**Files:**
- Modify: `src/gateway/handlers/workspace.rs` — remove `handle_switch` and `handle_get_active`
- Modify: `src/gateway/handlers/mod.rs` — remove `workspace.switch` and `workspace.getActive` registration, add new RPCs

- [ ] **Step 1: Remove old RPC handlers from `workspace.rs`**

Delete:
- `SwitchParams` struct and `handle_switch` function
- `GetActiveParams` struct and `handle_get_active` function

- [ ] **Step 2: Add `channels.set_agent` handler**

In `workspace.rs` (or a new section), add:

```rust
#[derive(Debug, Deserialize)]
pub struct SetAgentParams {
    pub channel_id: String,
    pub agent_id: Option<String>,
}

pub async fn handle_set_agent(
    request: JsonRpcRequest,
    workspace_manager: Arc<WorkspaceManager>,
) -> JsonRpcResponse {
    let params: SetAgentParams = match serde_json::from_value(request.params.unwrap_or_default()) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e.to_string()),
    };

    match params.agent_id {
        Some(agent_id) => {
            match workspace_manager.set_active_agent(&params.channel_id, &agent_id) {
                Ok(()) => JsonRpcResponse::success(request.id, serde_json::json!({"ok": true})),
                Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
            }
        }
        None => {
            match workspace_manager.clear_active_agent(&params.channel_id) {
                Ok(()) => JsonRpcResponse::success(request.id, serde_json::json!({"ok": true})),
                Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
            }
        }
    }
}
```

- [ ] **Step 3: Add `agents.bindings` handler**

```rust
pub async fn handle_agent_bindings(
    request: JsonRpcRequest,
    workspace_manager: Arc<WorkspaceManager>,
) -> JsonRpcResponse {
    match workspace_manager.get_all_agent_bindings() {
        Ok(bindings) => JsonRpcResponse::success(request.id, serde_json::json!({"bindings": bindings})),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}
```

- [ ] **Step 4: Update `handlers/mod.rs` — register new RPCs, remove old**

Replace:
```rust
registry.register("workspace.switch", ...);
registry.register("workspace.getActive", ...);
```

With:
```rust
registry.register("channels.set_agent", ...);
registry.register("agents.bindings", ...);
```

Wire them to the new handlers with the `workspace_manager` dependency.

- [ ] **Step 5: Update test assertions in `handlers/mod.rs`**

Replace `assert!(registry.has_method("workspace.switch"))` and `assert!(registry.has_method("workspace.getActive"))` with the new method names.

- [ ] **Step 6: Run `cargo check -p alephcore` and `cargo test -p alephcore --lib`**

Expected: Clean compile and passing tests.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/workspace.rs src/gateway/handlers/mod.rs
git commit -m "rpc: replace workspace.switch/getActive with channels.set_agent and agents.bindings"
```

---

### Task 8: Update Panel — workspace API and agent channels tab

Update Panel to use new RPCs and show binding status.

**Files:**
- Modify: `apps/panel/src/api/workspace.rs` — remove `get_active` and `switch`, add `set_agent`
- Modify: `apps/panel/src/views/agents/channels.rs` — show binding info (read-only)

- [ ] **Step 1: Update `apps/panel/src/api/workspace.rs`**

Remove `get_active()` and `switch()` methods. Add:

```rust
/// Set the agent binding for a channel (or unbind with None)
pub async fn set_channel_agent(
    state: &DashboardState,
    channel_id: &str,
    agent_id: Option<&str>,
) -> Result<(), String> {
    let params = serde_json::json!({
        "channel_id": channel_id,
        "agent_id": agent_id,
    });
    state.rpc_call("channels.set_agent", params).await?;
    Ok(())
}

/// Get all agent→channel bindings
pub async fn agent_bindings(
    state: &DashboardState,
) -> Result<std::collections::HashMap<String, String>, String> {
    let result = state.rpc_call("agents.bindings", serde_json::Value::Null).await?;
    result.get("bindings")
        .ok_or_else(|| "Invalid response: missing bindings".to_string())
        .and_then(|b| {
            serde_json::from_value(b.clone())
                .map_err(|e| format!("Failed to parse bindings: {}", e))
        })
}
```

- [ ] **Step 2: Update `views/agents/channels.rs` — show read-only binding**

Replace the routing rules loading with binding lookup. The ChannelsTab should:
1. Call `agents.bindings` to get the mapping
2. Show which channel this agent is bound to (read-only)
3. Show "未绑定" if not bound to any channel

Simplify the view to just display binding status:

```rust
#[component]
pub fn ChannelsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let agent_id = StoredValue::new(agent_id);
    let bound_channel = RwSignal::new(Option::<String>::None);
    let is_loading = RwSignal::new(true);

    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        let id = agent_id.get_value();
        spawn_local(async move {
            if let Ok(result) = dash.rpc_call("agents.bindings", serde_json::Value::Null).await {
                if let Some(bindings) = result.get("bindings") {
                    if let Some(ch) = bindings.get(&id).and_then(|v| v.as_str()) {
                        bound_channel.set(Some(ch.to_string()));
                    }
                }
            }
            is_loading.set(false);
        });
    });

    view! {
        <div class="space-y-6">
            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="text-text-secondary py-8 text-center">"Loading..."</div>
                    }.into_any();
                }

                view! {
                    <div class="bg-surface-raised border border-border rounded-xl p-6">
                        <h2 class="text-lg font-semibold text-text-primary mb-4">"Channel Binding"</h2>
                        {move || {
                            match bound_channel.get() {
                                Some(ch) => view! {
                                    <div class="flex items-center gap-2">
                                        <span class="px-3 py-1 rounded-full text-xs font-medium bg-success/20 text-success">"BOUND"</span>
                                        <span class="text-sm text-text-primary">{ch}</span>
                                    </div>
                                }.into_any(),
                                None => view! {
                                    <div class="flex items-center gap-2">
                                        <span class="px-3 py-1 rounded-full text-xs font-medium bg-surface-sunken text-text-tertiary">"未绑定"</span>
                                        <span class="text-sm text-text-secondary">"请在 Settings → Channels 中绑定此 Agent"</span>
                                    </div>
                                }.into_any(),
                            }
                        }}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
```

- [ ] **Step 3: Run Panel build to verify**

```bash
cd apps/panel && trunk build 2>&1 | tail -20
```

Expected: Clean build.

- [ ] **Step 4: Commit**

```bash
git add apps/panel/src/api/workspace.rs apps/panel/src/views/agents/channels.rs
git commit -m "panel: update agent channels tab for 1:1 binding (read-only)"
```

---

### Task 9: Add agent dropdown to Channel settings page

Add agent selector to channel configuration pages.

**Files:**
- Modify: `apps/panel/src/views/settings/channels/discord.rs` — add agent dropdown section
- May need to modify other channel config pages (telegram, etc.) similarly

- [ ] **Step 1: Identify all channel config pages**

```bash
ls apps/panel/src/views/settings/channels/
```

Determine which channel config views need the agent dropdown. The pattern will be identical for each.

- [ ] **Step 2: Create a shared `AgentBindingSelector` component**

Create a reusable component that can be embedded in any channel settings page:

The component should:
1. Fetch agent list via `agents.list` RPC
2. Fetch current bindings via `agents.bindings` RPC
3. Show a dropdown with all agents + "未绑定" option
4. Disable agents already bound to other channels (show which channel)
5. On selection, call `channels.set_agent` RPC

Place in `apps/panel/src/components/ui/agent_binding_selector.rs` and register in `components/ui/mod.rs`.

- [ ] **Step 3: Add `AgentBindingSelector` to Discord channel config**

In `discord.rs`, add the component at the top of the settings sections, before the token configuration:

```rust
<AgentBindingSelector channel_id="discord".to_string() />
```

- [ ] **Step 4: Add to other channel configs as needed**

Repeat for telegram, etc. — any channel config page that exists.

- [ ] **Step 5: Run Panel build**

```bash
cd apps/panel && trunk build 2>&1 | tail -20
```

- [ ] **Step 6: Commit**

```bash
git add apps/panel/src/components/ui/agent_binding_selector.rs apps/panel/src/components/ui/mod.rs apps/panel/src/views/settings/channels/
git commit -m "panel: add agent binding selector to channel settings pages"
```

---

### Task 10: Final integration test and cleanup

Verify everything compiles, tests pass, and clean up any loose ends.

**Files:**
- Various — any remaining compiler warnings or test failures

- [ ] **Step 1: Full compile check**

```bash
cargo check -p alephcore
```

Expected: Clean compile, no errors.

- [ ] **Step 2: Run all core tests**

```bash
cargo test -p alephcore --lib
```

Expected: All tests pass. Some pre-existing failures in `tools::markdown_skill::loader::tests` are known and not our issue.

- [ ] **Step 3: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | head -40
```

Fix any new clippy warnings from our changes (unused imports, dead code, etc.).

- [ ] **Step 4: Clean up any remaining `agent_router` references**

Search for any remaining `AgentRouter` imports or usages:
```bash
grep -r "AgentRouter\|agent_router" src/ --include="*.rs" | grep -v target
```

Remove any that are now unused.

- [ ] **Step 5: Clean up any remaining `workspace.switch` / `workspace.getActive` Panel references**

```bash
grep -r "workspace\.switch\|workspace\.getActive\|get_active\|ActiveWorkspaceInfo" apps/panel/src/ --include="*.rs"
```

Remove any stale references.

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "cleanup: final integration pass for 1:1 agent-bot binding"
```
