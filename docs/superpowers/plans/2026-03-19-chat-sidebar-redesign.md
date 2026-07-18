# Chat Sidebar Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign the panel ChatSidebar into a two-level agent/session hierarchy with automatic topic generation on first message.

**Architecture:** Backend extends `SessionListRow` with `agent_id` and `topic` fields, adds async LLM topic generation triggered from `handle_chat_send_with_engine`. Frontend rewrites `ChatSidebar` as collapsible agent groups, adds `agent_id` signal to `ChatState`, simplifies `InputArea` agent routing.

**Tech Stack:** Rust (alephcore), Leptos (WASM panel), SQLite (session metadata)

**Spec:** `docs/superpowers/specs/2026-03-19-chat-sidebar-redesign.md`

---

### Task 1: Extend `SessionListRow` with `agent_id` and `topic`

**Files:**
- Modify: `src/builtin_tools/sessions/list_tool.rs:46-61` (SessionListRow struct)
- Modify: `src/builtin_tools/sessions/list_tool.rs:119-138` (metadata_to_row function)
- Modify: `src/builtin_tools/sessions/list_tool.rs:284-577` (tests)

- [ ] **Step 1: Add `agent_id` and `topic` fields to `SessionListRow`**

In `list_tool.rs`, add two fields to the struct:

```rust
pub struct SessionListRow {
    pub key: String,
    pub kind: String,
    pub channel: String,
    pub updated_at: Option<i64>,
    pub message_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<StoredMessage>>,
    /// Agent that owns this session
    pub agent_id: String,
    /// Session topic (generated from first message)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}
```

- [ ] **Step 2: Update `metadata_to_row` to populate new fields**

In the `metadata_to_row` method, extract `agent_id` from `SessionMetadata` directly and parse `topic` from `metadata_json`:

```rust
fn metadata_to_row(&self, meta: &SessionMetadata) -> SessionListRow {
    let (kind, channel) = if let Some(parsed) = SessionKey::parse(&meta.key) {
        let kind = classify_session_kind(&parsed);
        let channel = derive_channel(&parsed);
        (kind.as_str().to_string(), channel)
    } else {
        (meta.session_type.clone(), "unknown".to_string())
    };

    // Extract topic from metadata_json
    let topic = meta.metadata_json.as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.get("topic").and_then(|t| t.as_str()).map(String::from));

    SessionListRow {
        key: meta.key.clone(),
        kind,
        channel,
        updated_at: Some(meta.last_active_at),
        message_count: meta.message_count as usize,
        messages: None,
        agent_id: meta.agent_id.clone(),
        topic,
    }
}
```

- [ ] **Step 3: Update existing tests to include new fields**

Existing tests that construct `SessionListRow` or assert on its fields need updating. In `test_list_with_sessions`, verify that `agent_id` is populated:

```rust
// In test_list_with_sessions, add assertion:
assert_eq!(result.sessions[0].agent_id, "main");
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib sessions::list_tool`
Expected: All existing tests pass with new fields populated.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/sessions/list_tool.rs
git commit -m "sessions: expose agent_id and topic in SessionListRow"
```

---

### Task 2: Add `set_topic` method to `SessionManager`

The `close_session` method stores topic in metadata but we need a way to set topic without closing the session (for the async first-message topic generation).

**Files:**
- Modify: `src/gateway/session_manager/ops.rs` (add `set_topic` method near `close_session` at line 388)

- [ ] **Step 1: Add `set_topic` method**

Add after `close_session` in `ops.rs`:

```rust
/// Set the topic for a session without closing it.
/// Used by async topic generation on first message.
pub async fn set_topic(
    &self,
    key: &SessionKey,
    topic: &str,
) -> Result<(), SessionManagerError> {
    let key_str = key.to_key_string();
    let conn = self.conn.lock().map_err(|e|
        SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

    // Get existing metadata
    let existing_json: Option<String> = conn
        .query_row("SELECT metadata FROM sessions WHERE key = ?", params![&key_str], |row| row.get(0))
        .optional()
        .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
        .flatten();

    let mut meta: serde_json::Value = existing_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    if let Some(obj) = meta.as_object_mut() {
        obj.insert("topic".to_string(), serde_json::json!(topic));
    }

    let meta_json = serde_json::to_string(&meta)
        .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

    conn.execute(
        "UPDATE sessions SET metadata = ? WHERE key = ?",
        params![&meta_json, &key_str],
    ).map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

    Ok(())
}
```

- [ ] **Step 2: Run compile check**

Run: `cargo check -p alephcore`
Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add src/gateway/session_manager/ops.rs
git commit -m "session_manager: add set_topic method for async topic generation"
```

---

### Task 3: Add async topic generation to `handle_chat_send_with_engine`

**Files:**
- Modify: `src/bin/aleph/server_init.rs:218-357` (handle_chat_send_with_engine)
- Modify: `src/bin/aleph/commands/start/builder/agent_init.rs:534-542` (handler registration closure)

- [ ] **Step 1: Add `provider_registry` and `session_manager` params to `handle_chat_send_with_engine`**

Update the function signature to accept provider registry and session manager:

```rust
pub async fn handle_chat_send_with_engine<P, R>(
    request: alephcore::gateway::JsonRpcRequest,
    engine: Arc<ExecutionEngine<P, R>>,
    event_bus: Arc<GatewayEventBus>,
    router: Arc<AgentRouter>,
    agent_registry: Arc<AgentRegistry>,
    app_config: Arc<tokio::sync::RwLock<alephcore::Config>>,
    workspace_manager: Option<Arc<alephcore::gateway::WorkspaceManager>>,
    provider_registry: Arc<P>,       // NEW
    session_manager: Arc<alephcore::gateway::SessionManager>,  // NEW
) -> alephcore::gateway::JsonRpcResponse
where
    P: alephcore::thinker::ProviderRegistry + 'static,
    R: alephcore::executor::ToolRegistry + 'static,
```

- [ ] **Step 2: Add topic generation spawn after the execution task spawn**

After the existing `tokio::spawn` for execution (line 335-347), before the return statement, add:

```rust
// Spawn async topic generation for new sessions
// Note: params.session_key.is_none() means a brand-new session was created by router.
// session_key here is the legacy gateway::router::SessionKey (returned by router.route()).
// Event topic mapping: backend emits "stream.session_updated" via event_bus,
// the panel message loop converts it to GatewayEvent { topic: "run.session_updated" },
// and the sidebar handler checks for "run.session_updated" — this chain already works.
let is_new_session = params.session_key.is_none();
if is_new_session {
    let topic_provider = {
        // Prefer lightweight model, fallback to agent's model, then default
        provider_registry
            .get("haiku")
            .or_else(|| provider_registry.get(&agent.model))
            .unwrap_or_else(|| provider_registry.default_provider())
    };
    let topic_session_key = session_key.clone();  // gateway::router::SessionKey
    let topic_event_bus = event_bus.clone();
    let topic_session_manager = session_manager.clone();
    let topic_message = params.message.clone();
    tokio::spawn(async move {
        use alephcore::providers::adapter::RequestPayload;
        use alephcore::providers::message::UnifiedMessage;

        let prompt = format!(
            "Generate a concise topic title (5-10 characters, same language as the message) \
             for a conversation that starts with: {}",
            topic_message
        );
        let messages = vec![UnifiedMessage::user(&prompt)];
        let payload = RequestPayload {
            messages: &messages,
            system_prompt: Some("You are a title generator. Output ONLY the title, nothing else."),
            tools: None,
            think_level: None,
            temperature: Some(0.3),
            max_tokens: Some(30),
        };

        match topic_provider.process(payload).await {
            Ok(response) => {
                // ProviderResponse.text_content() returns String (convenience method)
                let topic_text = response.text_content().trim().to_string();
                if !topic_text.is_empty() {
                    // session_key is already legacy gateway::router::SessionKey — use directly
                    if let Err(e) = topic_session_manager.set_topic(&topic_session_key, &topic_text).await {
                        tracing::warn!(error = %e, "Failed to set session topic");
                    } else {
                        // GatewayEventBus::publish takes a JSON string, not a struct
                        let event_json = serde_json::json!({
                            "method": "stream.session_updated",
                            "params": {
                                "session_key": topic_session_key.to_key_string(),
                                "topic": topic_text,
                            }
                        });
                        topic_event_bus.publish(event_json.to_string());
                        tracing::debug!(
                            session_key = %topic_session_key.to_key_string(),
                            topic = %topic_text,
                            "Auto-generated session topic"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Topic generation failed, session will show as 'New Chat'");
            }
        }
    });
}
```

- [ ] **Step 3: Update handler registration in `agent_init.rs`**

In `agent_init.rs` line 527-544, pass provider registry and session manager to the handler. The `provider_registry` is moved into `ExecutionEngine::new()` at line 411. We must clone the Arc **inside** the `if let Some(provider_registry)` block, **before** it's moved into `ExecutionEngine::new()`.

Inside the `if let Some(provider_registry)` block, before line 409 (`let engine_config = ...`), add:
```rust
let topic_provider_registry = provider_registry.clone();
```

Then update the `chat.send` handler registration (lines 527-544):
```rust
// chat.send also uses real ExecutionEngine
let engine_chat = engine.clone();
let event_bus_chat = event_bus.clone();
let router_chat = router.clone();
let agent_registry_chat = agent_registry.clone();
let app_config_chat = app_config_arc.clone();
let wm_chat = workspace_manager.clone();
let pr_chat = topic_provider_registry.clone();  // NEW
let sm_chat = session_manager.clone();           // NEW
server.handlers_mut().register("chat.send", move |req| {
    let engine = engine_chat.clone();
    let event_bus = event_bus_chat.clone();
    let router = router_chat.clone();
    let agent_registry = agent_registry_chat.clone();
    let cfg = app_config_chat.clone();
    let wm = wm_chat.clone();
    let pr = pr_chat.clone();   // NEW
    let sm = sm_chat.clone();   // NEW
    async move {
        handle_chat_send_with_engine(req, engine, event_bus, router, agent_registry, cfg, wm, pr, sm).await
    }
});
```

Also update `handle_run_with_engine` callsite if it shares the same signature (check if it needs updating too — it likely doesn't since topic gen only applies to chat.send).

- [ ] **Step 4: Run compile check**

Run: `cargo check -p alephcore && cargo check --bin aleph`
Expected: Compiles without errors. Key API calls verified: `UnifiedMessage::user()`, `response.text_content()`, `event_bus.publish(String)`.

- [ ] **Step 5: Commit**

```bash
git add src/bin/aleph/server_init.rs src/bin/aleph/commands/start/builder/agent_init.rs
git commit -m "chat.send: spawn async LLM topic generation for new sessions"
```

---

### Task 4: Add `agent_id` signal and `clear_session()` to `ChatState`

**Files:**
- Modify: `apps/panel/src/views/chat/state.rs:42-57` (ChatState struct)
- Modify: `apps/panel/src/views/chat/state.rs:60-70` (ChatState::new)
- Modify: `apps/panel/src/views/chat/state.rs:173-181` (ChatState::clear)

- [ ] **Step 1: Add `agent_id` field to `ChatState`**

```rust
#[derive(Clone, Copy)]
pub struct ChatState {
    pub messages: RwSignal<Vec<ChatMessage>>,
    pub phase: RwSignal<ChatPhase>,
    pub active_run_id: RwSignal<Option<String>>,
    pub session_key: RwSignal<Option<String>>,
    pub reasoning_text: RwSignal<String>,
    pub error_message: RwSignal<Option<String>>,
    next_msg_id: RwSignal<u64>,
    /// Currently selected agent ID for routing
    pub agent_id: RwSignal<Option<String>>,  // NEW
}
```

- [ ] **Step 2: Initialize `agent_id` in `new()`**

```rust
pub fn new() -> Self {
    Self {
        messages: RwSignal::new(Vec::new()),
        phase: RwSignal::new(ChatPhase::Idle),
        active_run_id: RwSignal::new(None),
        session_key: RwSignal::new(None),
        reasoning_text: RwSignal::new(String::new()),
        error_message: RwSignal::new(None),
        next_msg_id: RwSignal::new(0),
        agent_id: RwSignal::new(None),  // NEW
    }
}
```

- [ ] **Step 3: Update `clear()` to also reset `agent_id`**

```rust
pub fn clear(&self) {
    self.messages.set(Vec::new());
    self.phase.set(ChatPhase::Idle);
    self.active_run_id.set(None);
    self.session_key.set(None);
    self.reasoning_text.set(String::new());
    self.error_message.set(None);
    self.agent_id.set(None);  // NEW
}
```

- [ ] **Step 4: Add `clear_session()` method**

Add after `clear()`:

```rust
/// Clear session state but keep agent_id (for new chat within same agent).
pub fn clear_session(&self) {
    self.messages.set(Vec::new());
    self.phase.set(ChatPhase::Idle);
    self.active_run_id.set(None);
    self.session_key.set(None);
    self.reasoning_text.set(String::new());
    self.error_message.set(None);
    // agent_id is intentionally preserved
}
```

- [ ] **Step 5: Compile check**

Run: `cd apps/panel && cargo check`
Expected: Compiles. `RwSignal<Option<String>>` is `Copy` so `#[derive(Clone, Copy)]` still works.

- [ ] **Step 6: Commit**

```bash
git add apps/panel/src/views/chat/state.rs
git commit -m "panel: add agent_id signal and clear_session() to ChatState"
```

---

### Task 5: Simplify `InputArea` agent_id handling

**Files:**
- Modify: `apps/panel/src/views/chat/view.rs:258-267` (agent_id extraction in send_message)

- [ ] **Step 1: Replace session_key parsing with direct agent_id read**

In `send_message` closure (around line 258), replace:

```rust
let session_key = chat.session_key.get();
// Extract agent_id from session_key (format: "agent:AGENT_ID:...")
let agent_id = session_key.as_deref().and_then(|sk| {
    let parts: Vec<&str> = sk.split(':').collect();
    if parts.len() >= 2 && parts[0] == "agent" {
        Some(parts[1].to_string())
    } else {
        None
    }
});
```

With:

```rust
let session_key = chat.session_key.get();
let agent_id = chat.agent_id.get();
```

And update the `ChatApi::send` call to pass `agent_id.as_deref()` (it already does, just with the new source).

- [ ] **Step 2: Compile check**

Run: `cd apps/panel && cargo check`
Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add apps/panel/src/views/chat/view.rs
git commit -m "panel: simplify InputArea to read agent_id directly from ChatState"
```

---

### Task 6: Rewrite `ChatSidebar` with two-level agent/session hierarchy

**Files:**
- Modify: `apps/panel/src/components/chat_sidebar.rs` (full rewrite)

- [ ] **Step 1: Update `SessionEntry` struct**

Replace the existing struct with snake_case fields matching backend:

```rust
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct SessionEntry {
    key: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    message_count: u32,
    /// Backend sends `updated_at: Option<i64>` (Unix epoch seconds)
    #[serde(default)]
    updated_at: Option<i64>,
}
```

Note: removed `#[serde(rename_all = "camelCase")]` — backend serializes snake_case.
Changed `last_active_at: String` → `updated_at: Option<i64>` to match backend `SessionListRow.updated_at`.

- [ ] **Step 2: Add `AgentEntry` struct**

```rust
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct AgentEntry {
    id: String,
    name: String,
    #[serde(default)]
    is_active: bool,
}
```

- [ ] **Step 3: Rewrite data fetching to load both agents and sessions**

Replace the single `reload_sessions` closure with a combined loader:

```rust
let agents = RwSignal::new(Vec::<AgentEntry>::new());
let sessions = RwSignal::new(Vec::<SessionEntry>::new());
let is_loading = RwSignal::new(false);
let collapsed = RwSignal::new(std::collections::HashSet::<String>::new());

let reload_data = Arc::new(move |dash: DashboardState| {
    is_loading.set(true);
    leptos::task::spawn_local(async move {
        // Fetch agents and sessions in parallel
        let (agents_result, sessions_result) = futures::join!(
            dash.rpc_call("agents.list", serde_json::json!({})),
            dash.rpc_call("sessions.list", serde_json::json!({})),
        );

        if let Ok(result) = agents_result {
            if let Some(arr) = result.get("agents") {
                if let Ok(list) = serde_json::from_value::<Vec<AgentEntry>>(arr.clone()) {
                    agents.set(list);
                }
            }
        }

        if let Ok(result) = sessions_result {
            if let Some(arr) = result.get("sessions") {
                if let Ok(list) = serde_json::from_value::<Vec<SessionEntry>>(arr.clone()) {
                    sessions.set(list);
                }
            }
        }

        is_loading.set(false);
    });
});
```

Note: check if `futures::join!` is available in the panel crate. If not, use two sequential awaits or add `futures` to panel dependencies.

- [ ] **Step 4: Rewrite rendering logic with collapsible agent groups**

Replace the view body with the two-level hierarchy. Key structure:

```rust
view! {
    <div class="flex flex-col h-full">
        // Search (keep existing)
        <div class="p-3">/* ... existing search placeholder ... */</div>

        // Agent/Session list
        <div class="flex-1 overflow-y-auto px-3 py-2 space-y-1">
            {move || {
                let agent_list = agents.get();
                let session_list = sessions.get();
                let active_key = chat.session_key.get();
                let active_agent = chat.agent_id.get();

                if is_loading.get() {
                    return view! {
                        <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                            "Loading..."
                        </p>
                    }.into_any();
                }

                // Group sessions by agent_id
                let mut grouped: std::collections::HashMap<String, Vec<SessionEntry>> =
                    std::collections::HashMap::new();
                for session in &session_list {
                    grouped.entry(session.agent_id.clone())
                        .or_default()
                        .push(session.clone());
                }
                // Sort each group by updated_at desc (most recent first)
                for group in grouped.values_mut() {
                    group.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                }

                let collapsed_set = collapsed.get();

                view! {
                    <div>
                        {agent_list.into_iter().map(|agent| {
                            let agent_id = agent.id.clone();
                            let agent_sessions = grouped.get(&agent_id).cloned().unwrap_or_default();
                            let session_count = agent_sessions.len();
                            let is_collapsed = collapsed_set.contains(&agent_id);
                            let is_active_agent = active_agent.as_deref() == Some(&agent_id);

                            // Auto-expand active agent's group
                            let show_sessions = !is_collapsed || is_active_agent;

                            let agent_id_toggle = agent_id.clone();
                            let agent_id_new = agent_id.clone();

                            view! {
                                <div class="mb-2">
                                    // Agent header
                                    <div class="flex items-center justify-between px-2 py-1.5">
                                        <button
                                            class="flex items-center gap-1 text-text-secondary text-[11px] font-semibold uppercase tracking-wider hover:text-text-primary transition-colors"
                                            on:click=move |_| {
                                                collapsed.update(|set| {
                                                    if set.contains(&agent_id_toggle) {
                                                        set.remove(&agent_id_toggle);
                                                    } else {
                                                        set.insert(agent_id_toggle.clone());
                                                    }
                                                });
                                            }
                                        >
                                            <span class="text-[10px]">
                                                {if show_sessions { "\u{25BC}" } else { "\u{25B6}" }}
                                            </span>
                                            <span>{&agent.name}</span>
                                            <span class="text-text-tertiary font-normal ml-1">
                                                {format!("({})", session_count)}
                                            </span>
                                        </button>
                                        // + New Chat button
                                        <button
                                            class="w-5 h-5 flex items-center justify-center rounded-full
                                                   bg-primary text-white text-[10px] font-bold
                                                   hover:bg-primary/80 transition-colors"
                                            title="New Chat"
                                            on:click=move |_| {
                                                chat.clear_session();
                                                chat.agent_id.set(Some(agent_id_new.clone()));
                                            }
                                        >
                                            "+"
                                        </button>
                                    </div>
                                    // Session list (collapsible)
                                    <Show when=move || show_sessions>
                                        <div class="pl-2">
                                            {agent_sessions.iter().map(|session| {
                                                let key = session.key.clone();
                                                let key_click = key.clone();
                                                let sid = session.agent_id.clone();
                                                let is_active = move || {
                                                    active_key.as_deref() == Some(&key)
                                                };
                                                let label = session.topic.clone()
                                                    .unwrap_or_else(|| "New Chat".to_string());
                                                let subtitle = format_session_subtitle(session);
                                                view! {
                                                    <button
                                                        class=move || format!(
                                                            "w-full text-left px-3 py-2.5 rounded-lg text-sm transition-colors {}",
                                                            if is_active() {
                                                                "bg-primary/10 text-primary font-medium"
                                                            } else {
                                                                "text-text-secondary hover:bg-surface-sunken hover:text-text-primary"
                                                            }
                                                        )
                                                        on:click=move |_| {
                                                            on_select_session(key_click.clone(), sid.clone());
                                                        }
                                                    >
                                                        <div class="truncate font-medium text-xs">{label}</div>
                                                        <div class="truncate text-[10px] text-text-tertiary mt-0.5">{subtitle}</div>
                                                    </button>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    </Show>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_any()
            }}
        </div>
    </div>
}
```

- [ ] **Step 5: Update `on_select_session` to use `clear_session()` and set `agent_id`**

```rust
let on_select_session = move |key: String, agent_id: String| {
    let dash = dashboard;
    let current = chat.session_key.get_untracked();
    if current.as_deref() == Some(&key) {
        return; // already selected
    }
    // Clear session state but keep agent_id flexible
    chat.clear_session();
    chat.agent_id.set(Some(agent_id));
    chat.session_key.set(Some(key.clone()));

    // Load history for selected session
    leptos::task::spawn_local(async move {
        match ChatApi::history(&dash, &key, Some(50)).await {
            Ok(history) => {
                let msgs: Vec<crate::views::chat::state::ChatMessage> = history
                    .into_iter()
                    .enumerate()
                    .map(|(i, m)| crate::views::chat::state::ChatMessage {
                        id: m.run_id.unwrap_or_else(|| format!("hist-{i}")),
                        role: m.role,
                        content: m.content,
                        tool_calls: vec![],
                        is_streaming: false,
                        error: None,
                    })
                    .collect();
                chat.messages.set(msgs);
            }
            Err(e) => {
                web_sys::console::error_1(&format!("Failed to load history: {e}").into());
            }
        }
    });
};
```

- [ ] **Step 6: Remove old `format_session_label` and update `format_session_subtitle`**

Delete `format_session_label` (no longer needed — we use `topic` directly).

Rewrite `format_session_subtitle` to handle `updated_at: Option<i64>` instead of `last_active_at: String`:

```rust
fn format_session_subtitle(session: &SessionEntry) -> String {
    let msg_count = session.message_count;
    match session.updated_at {
        Some(ts) => {
            // Format Unix epoch seconds as short date
            let date = chrono::DateTime::from_timestamp(ts, 0)
                .map(|dt| dt.format("%m-%d").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            format!("{msg_count} msgs - {date}")
        }
        None => format!("{msg_count} messages"),
    }
}
```

Note: check if `chrono` is available in the panel crate. If not, use `js_sys::Date` for timestamp formatting in WASM, or do simple integer division to extract date components.

- [ ] **Step 7: Compile check**

Run: `cd apps/panel && cargo check`
Expected: Compiles. Fix any Leptos-specific issues with closures, signal moves, or lifetime issues.

- [ ] **Step 8: Commit**

```bash
git add apps/panel/src/components/chat_sidebar.rs
git commit -m "panel: rewrite ChatSidebar with two-level agent/session hierarchy"
```

---

### Task 7: Build WASM and manual integration test

**Files:**
- No new files — build and verify

- [ ] **Step 1: Build the full stack**

Run: `just build` (or `just dev` for dev mode)
Expected: WASM compiles, server builds successfully.

- [ ] **Step 2: Start the server and verify sidebar**

```bash
# Kill any existing processes
pkill -f "target/release/aleph" 2>/dev/null
pkill -f "target/debug/aleph" 2>/dev/null
sleep 2

# Start server
cargo run --bin aleph -- start
```

Open panel in browser. Verify:
1. Sidebar shows agents as collapsible groups
2. Sessions appear under correct agent groups with topics (or "New Chat")
3. Clicking a session loads its history
4. Clicking "+" creates a new empty chat under that agent
5. Sending a first message generates a topic (appears after a few seconds via session_updated event)

- [ ] **Step 3: Commit build artifacts if needed**

```bash
git add apps/panel/dist/
git commit -m "panel: rebuild WASM with sidebar redesign"
```

---

### Task 8: Handle `/new` topic fallback

**Files:**
- Modify: `src/builtin_tools/sessions/new_tool.rs`

- [ ] **Step 1: Review current `/new` implementation**

The current `session_new` tool already accepts `topic` param and passes it to `close_session`. The spec says: if old session has no topic at close time, trigger fallback topic generation.

However, the `/new` tool doesn't have access to `ProviderRegistry` to generate a topic. It only has `SessionManager`. The simplest approach: if the user/LLM provides a topic via the tool arg, use it. If not, the session stays without a topic — but the async first-message topic generation from Task 3 should have already set one. So the fallback is rarely needed.

**Decision:** Keep `/new` as-is. The first-message topic generation (Task 3) covers the vast majority of cases. No code change needed.

- [ ] **Step 2: Verify no changes needed — mark complete**

Run: `cargo test -p alephcore --lib sessions::new_tool`
Expected: All existing tests pass unchanged.

- [ ] **Step 3: Commit (no-op — document decision)**

No code changes. The first-message topic generation handles the common case.
