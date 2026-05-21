# Agent Link Access Control Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add per-agent link (bot instance) access control so administrators can restrict which Links can access each agent.

**Architecture:** Add `allowed_links: Option<Vec<String>>` to `AgentDefinition` and `AgentPatch`. Enforce at two points: InboundRouter (message routing) and agent switch (slash command + intent detection). UI toggles in the Channels tab.

**Tech Stack:** Rust (core), Leptos (panel UI), TOML (config), JSON-RPC (API)

---

### Task 1: Add `allowed_links` field to AgentDefinition

**Files:**
- Modify: `src/config/types/agents_def.rs:186-239`

**Step 1: Add the field to AgentDefinition**

In `src/config/types/agents_def.rs`, add after line 238 (`pub subagents: Option<SubagentPolicy>,`):

```rust
    /// Link access whitelist.
    /// None or empty = all links allowed (default).
    /// Some(list) = only listed link IDs can access this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_links: Option<Vec<String>>,
```

**Step 2: Add a test for allowed_links deserialization**

Add after the existing `test_subagent_policy_wildcard` test (line 498):

```rust
    #[test]
    fn test_allowed_links_deserialize() {
        let toml_str = r#"
            [[list]]
            id = "private"
            name = "Private Agent"
            allowed_links = ["telegram-bot-1", "discord-bot"]
        "#;
        let config: AgentsConfig = toml::from_str(toml_str).unwrap();
        let agent = &config.list[0];
        assert_eq!(
            agent.allowed_links,
            Some(vec!["telegram-bot-1".to_string(), "discord-bot".to_string()])
        );
    }

    #[test]
    fn test_allowed_links_none_by_default() {
        let toml_str = r#"
            [[list]]
            id = "open"
            name = "Open Agent"
        "#;
        let config: AgentsConfig = toml::from_str(toml_str).unwrap();
        assert!(config.list[0].allowed_links.is_none());
    }
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib config::types::agents_def`
Expected: All tests pass, including new ones.

**Step 4: Commit**

```bash
git add src/config/types/agents_def.rs
git commit -m "config: add allowed_links field to AgentDefinition"
```

---

### Task 2: Add `allowed_links` to AgentPatch and apply logic

**Files:**
- Modify: `src/config/agent_manager.rs:47-54` (AgentPatch struct)
- Modify: `src/config/agent_manager.rs:268-296` (update method, patch apply)

**Step 1: Add field to AgentPatch**

In `src/config/agent_manager.rs`, add after `pub subagents: Option<SubagentPolicy>,` (line 54):

```rust
    pub allowed_links: Option<Vec<String>>,
```

**Step 2: Add patch apply logic**

In the `update` method, add after the `subagents` block (after line 292, before `self.save_document(&doc)?;`):

```rust
        if let Some(allowed_links) = &patch.allowed_links {
            if allowed_links.is_empty() {
                // Empty list = all allowed, remove the key
                agent_table.remove("allowed_links");
            } else {
                let mut arr = Array::new();
                for l in allowed_links {
                    arr.push(l.as_str());
                }
                agent_table["allowed_links"] = toml_edit::value(arr);
            }
        }
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib config::agent_manager`
Expected: All existing tests pass.

**Step 4: Commit**

```bash
git add src/config/agent_manager.rs
git commit -m "config: add allowed_links to AgentPatch with apply logic"
```

---

### Task 3: Add link access check function

**Files:**
- Modify: `src/gateway/inbound_router.rs` (add function, add to RoutingError)

**Step 1: Add LinkNotAllowed variant to RoutingError**

In `src/gateway/inbound_router.rs`, add to the `RoutingError` enum (after line 77):

```rust
    #[error("Link access denied: link \"{link_id}\" is not allowed to access agent \"{agent_id}\"")]
    LinkNotAllowed { link_id: String, agent_id: String },
```

**Step 2: Add the check function**

Add a free function in the same file (after the `RoutingError` enum, before `InboundDedupTracker`):

```rust
/// Check if a link (channel) is allowed to access an agent.
///
/// Returns Ok if access is allowed, Err if denied.
/// - `allowed_links` is None or empty → all links allowed (default)
/// - `allowed_links` has entries → only listed link IDs may access
fn check_link_access(
    allowed_links: &Option<Vec<String>>,
    link_id: &str,
    agent_id: &str,
) -> Result<(), RoutingError> {
    match allowed_links {
        None => Ok(()),
        Some(list) if list.is_empty() => Ok(()),
        Some(list) => {
            if list.iter().any(|l| l == link_id) {
                Ok(())
            } else {
                Err(RoutingError::LinkNotAllowed {
                    link_id: link_id.to_string(),
                    agent_id: agent_id.to_string(),
                })
            }
        }
    }
}
```

**Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles (function is not yet called, but should compile cleanly).

**Step 4: Commit**

```bash
git add src/gateway/inbound_router.rs
git commit -m "gateway: add check_link_access function and LinkNotAllowed error"
```

---

### Task 4: Enforce link access in InboundRouter message handling

**Files:**
- Modify: `src/gateway/inbound_router.rs:531-557` (handle_message method)

**Step 1: Add access check after agent resolution**

In `handle_message`, after the agent_id is resolved (line 542) and before building the context (line 545), add the link access check. The router needs access to the agent config. Look up the agent config via `agent_registry`:

After line 542 (`let agent_id = self.resolve_agent_id_async(channel_id, sender_id).await;`), add:

```rust
        // Check link access control
        if let Some(ref registry) = self.agent_registry {
            if let Some(agent_instance) = registry.get(&agent_id).await {
                let allowed_links = agent_instance.allowed_links().await;
                if let Err(e) = check_link_access(&allowed_links, channel_id, &agent_id) {
                    warn!("[Router] {}", e);
                    let reply = OutboundMessage::text(
                        msg.conversation_id.as_str(),
                        format!("⛔ {}", e),
                    );
                    let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                    return Ok(());
                }
            }
        }
```

**Note:** This requires `AgentInstance` to expose `allowed_links()`. If AgentInstance doesn't have this method, we need to look up the config via AgentManager or ConfigManager instead. Check how the agent's config is accessed at runtime. The `agent_registry` may store `AgentInstance` which holds a `ResolvedAgent`. Look at `ResolvedAgent` to see if it has `allowed_links`, or if we need to add it.

**Step 2: Ensure ResolvedAgent has allowed_links**

Check `src/config/agent_resolver.rs` — the `ResolvedAgent` struct. Add `allowed_links: Option<Vec<String>>` if not present, and populate it from `AgentDefinition` during resolution.

Check `src/gateway/agent_instance.rs` — the `AgentInstance` struct. Add a method `pub async fn allowed_links(&self) -> Option<Vec<String>>` that returns the resolved agent's allowed_links.

**Step 3: Verify compilation**

Run: `cargo check -p alephcore`

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib`

**Step 5: Commit**

```bash
git add src/gateway/inbound_router.rs src/config/agent_resolver.rs src/gateway/agent_instance.rs
git commit -m "gateway: enforce link access control in InboundRouter"
```

---

### Task 5: Enforce link access in agent switch (slash command)

**Files:**
- Modify: `src/gateway/inbound_router.rs:636-679` (handle_switch_command)

**Step 1: Add access check before switching**

In `handle_switch_command`, after verifying the agent exists (line 653, `let reply_text = if agent_exists {`), add a link access check before calling `manager.set_active_agent`:

```rust
            let reply_text = if agent_exists {
                // Check link access control
                let access_denied = if let Some(ref registry) = self.agent_registry {
                    if let Some(agent_instance) = registry.get(agent_name).await {
                        let allowed_links = agent_instance.allowed_links().await;
                        check_link_access(&allowed_links, channel_id, agent_name).is_err()
                    } else {
                        false
                    }
                } else {
                    false
                };

                if access_denied {
                    format!("⛔ Access denied: this bot is not allowed to access agent '{}'", agent_name)
                } else {
                    match manager.set_active_agent(channel_id, sender_id, agent_name) {
                        // ... existing Ok/Err handling ...
                    }
                }
            } else {
                // ... existing "not found" handling ...
            };
```

**Step 2: Verify compilation**

Run: `cargo check -p alephcore`

**Step 3: Commit**

```bash
git add src/gateway/inbound_router.rs
git commit -m "gateway: enforce link access control in /switch command"
```

---

### Task 6: Enforce link access in intent-detected agent switch

**Files:**
- Modify: `src/gateway/inbound_router.rs:760-835` (try_handle_switch_intent)

**Step 1: Add access check before switching in intent handler**

In `try_handle_switch_intent`, after confirming the agent exists (around line 790, after `registry.create_dynamic` if needed), add before the `set_active_agent` call at line 793:

```rust
                // Check link access control before switching
                if let Some(agent_instance) = registry.get(id).await {
                    let allowed_links = agent_instance.allowed_links().await;
                    if let Err(e) = check_link_access(&allowed_links, channel_id, id) {
                        let reply = OutboundMessage::text(
                            msg.conversation_id.as_str(),
                            format!("⛔ {}", e),
                        );
                        let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                        return Some(Ok(()));
                    }
                }
```

**Step 2: Verify compilation**

Run: `cargo check -p alephcore`

**Step 3: Commit**

```bash
git add src/gateway/inbound_router.rs
git commit -m "gateway: enforce link access control in intent-based agent switch"
```

---

### Task 7: Add Link Access Control UI to Channels tab

**Files:**
- Modify: `apps/panel/src/views/agents/channels.rs`

**Step 1: Add state signals for link access**

In the `ChannelsTab` component, after the existing signal declarations (around line 26), add:

```rust
    let all_channels = RwSignal::new(Vec::<ChannelInfo>::new());
    let allowed_links = RwSignal::new(Option::<Vec<String>>::None);
    let is_all_allowed = RwSignal::new(true);
```

Add the ChannelInfo struct at the top of the file (after RoutingRule):

```rust
#[derive(Debug, Clone, Deserialize)]
struct ChannelInfo {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    channel_type: String,
}
```

**Step 2: Load channels and agent's allowed_links in the Effect**

In the existing Effect (line 30-50), after loading routing rules, add:

```rust
            // Load all channels (links)
            if let Ok(result) = dash.rpc_call("channels.list", serde_json::Value::Null).await {
                if let Ok(channels) = serde_json::from_value::<Vec<ChannelInfo>>(result) {
                    all_channels.set(channels);
                }
            }

            // Load agent's allowed_links from definition
            if let Ok(detail) = AgentsApi::get(&dash, &id).await {
                if let Some(links) = detail.definition.get("allowed_links") {
                    if let Ok(links_vec) = serde_json::from_value::<Vec<String>>(links.clone()) {
                        if links_vec.is_empty() {
                            is_all_allowed.set(true);
                            allowed_links.set(None);
                        } else {
                            is_all_allowed.set(false);
                            allowed_links.set(Some(links_vec));
                        }
                    }
                } else {
                    is_all_allowed.set(true);
                    allowed_links.set(None);
                }
            }
```

**Step 3: Add the Link Access Control section in the view**

After the "Channel Bindings" card (after line 126, before the info div), add:

```rust
                        // Link Access Control
                        <div class="bg-surface-raised border border-border rounded-xl p-6">
                            <h2 class="text-lg font-semibold text-text-primary mb-4">"Link Access Control"</h2>
                            <p class="text-sm text-text-tertiary mb-4">"Control which bots (links) can access this agent. All links are allowed by default."</p>
                            {move || {
                                let channels = all_channels.get();
                                if channels.is_empty() {
                                    return view! {
                                        <p class="text-sm text-text-tertiary">"No active links found"</p>
                                    }.into_any();
                                }

                                let current_allowed = allowed_links.get();
                                let all = is_all_allowed.get();

                                view! {
                                    <div class="divide-y divide-border">
                                        {channels.into_iter().map(|ch| {
                                            let ch_id = ch.id.clone();
                                            let ch_id2 = ch.id.clone();
                                            let display_name = if ch.name.is_empty() { ch.id.clone() } else { ch.name.clone() };

                                            let is_on = if all {
                                                true
                                            } else if let Some(ref list) = current_allowed {
                                                list.contains(&ch_id)
                                            } else {
                                                true
                                            };

                                            view! {
                                                <div class="py-3 flex items-center justify-between">
                                                    <div>
                                                        <span class="text-sm font-medium text-text-primary">{display_name}</span>
                                                        <span class="text-xs text-text-tertiary ml-2">{format!("({})", ch.channel_type)}</span>
                                                    </div>
                                                    <button
                                                        on:click={
                                                            let ch_id = ch_id2.clone();
                                                            let dash = state;
                                                            let aid = agent_id.get_value();
                                                            move |_| {
                                                                let ch_id = ch_id.clone();
                                                                let aid = aid.clone();
                                                                let dash = dash;
                                                                spawn_local(async move {
                                                                    let current = allowed_links.get();
                                                                    let all_chs = all_channels.get();
                                                                    let all_ids: Vec<String> = all_chs.iter().map(|c| c.id.clone()).collect();

                                                                    let new_list = if is_all_allowed.get_untracked() {
                                                                        // Currently all allowed, user toggled one OFF
                                                                        let mut list: Vec<String> = all_ids.into_iter().filter(|id| id != &ch_id).collect();
                                                                        list
                                                                    } else {
                                                                        let mut list = current.unwrap_or_default();
                                                                        if list.contains(&ch_id) {
                                                                            // Toggle OFF
                                                                            list.retain(|id| id != &ch_id);
                                                                        } else {
                                                                            // Toggle ON
                                                                            list.push(ch_id.clone());
                                                                        }
                                                                        list
                                                                    };

                                                                    // If all are now ON, save as empty (= None = all allowed)
                                                                    let all_chs = all_channels.get();
                                                                    let all_on = all_chs.iter().all(|c| new_list.contains(&c.id));

                                                                    let patch = if all_on {
                                                                        serde_json::json!({"allowed_links": []})
                                                                    } else {
                                                                        serde_json::json!({"allowed_links": new_list.clone()})
                                                                    };

                                                                    if AgentsApi::update(&dash, &aid, patch).await.is_ok() {
                                                                        if all_on {
                                                                            is_all_allowed.set(true);
                                                                            allowed_links.set(None);
                                                                        } else {
                                                                            is_all_allowed.set(false);
                                                                            allowed_links.set(Some(new_list));
                                                                        }
                                                                    }
                                                                });
                                                            }
                                                        }
                                                        class=move || {
                                                            let on = if is_all_allowed.get() {
                                                                true
                                                            } else if let Some(ref list) = allowed_links.get() {
                                                                list.contains(&ch_id)
                                                            } else {
                                                                true
                                                            };
                                                            if on {
                                                                "px-3 py-1 rounded-full text-xs font-medium bg-success/20 text-success"
                                                            } else {
                                                                "px-3 py-1 rounded-full text-xs font-medium bg-error/20 text-error"
                                                            }
                                                        }
                                                    >
                                                        {move || {
                                                            let on = if is_all_allowed.get() {
                                                                true
                                                            } else if let Some(ref list) = allowed_links.get() {
                                                                list.contains(&ch_id)
                                                            } else {
                                                                true
                                                            };
                                                            if on { "ON" } else { "OFF" }
                                                        }}
                                                    </button>
                                                </div>
                                            }
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            }}
                        </div>
```

**Step 4: Verify WASM compilation**

Run: `just build` or the WASM build command for the panel.

**Step 5: Commit**

```bash
git add apps/panel/src/views/agents/channels.rs
git commit -m "panel: add Link Access Control UI to Channels tab"
```

---

### Task 8: Integration test

**Files:**
- Test manually or add integration test

**Step 1: Manual verification**

1. Start the server: `cargo run --bin aleph`
2. Open the panel UI
3. Navigate to an agent's Channels tab
4. Verify the "Link Access Control" section shows all active links with ON toggles
5. Toggle one link OFF, verify it saves (check `aleph.toml` for `allowed_links`)
6. Toggle all links back ON, verify `allowed_links` is removed from config
7. Send a message from the denied link, verify the error response

**Step 2: Commit final state**

```bash
git add -A
git commit -m "agent-link-acl: complete implementation with tests"
```
