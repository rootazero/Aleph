# Chat Persistence & Memory Pipeline — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix chat session persistence so sidebar shows topics and records survive restarts, plus build auto-memorization pipeline from chat to LanceDB.

**Architecture:** Three independent fixes wired sequentially — (1) ensure_session before add_message, (2) SessionUpdated event + sidebar subscription, (3) async MemoryEntry write after run completion.

**Tech Stack:** Rust (tokio, serde, chrono), Leptos/WASM (leptos::prelude, spawn_local), LanceDB (SessionStore trait)

---

## Task 1: Add `ensure_session` to AgentInstance

**Files:**
- Modify: `core/src/gateway/agent_instance.rs:303` (before `add_message`)

**Step 1: Add `ensure_session` method**

Insert this method before the existing `add_message` method (line 303):

```rust
/// Ensure a session exists in both the in-memory cache and SQLite.
///
/// Must be called before `add_message()` for any new session key
/// to guarantee the in-memory HashMap has an entry.
pub async fn ensure_session(&self, key: &SessionKey) {
    let key_str = key.to_key_string();

    // Ensure in-memory entry exists
    {
        let mut sessions = self.sessions.write().await;
        sessions.entry(key_str.clone()).or_insert_with(|| {
            let now = chrono::Utc::now();
            SessionData {
                messages: Vec::new(),
                created_at: now,
                last_active_at: now,
            }
        });
    }

    // Ensure SQLite row exists
    if let Some(ref sm) = self.session_manager {
        if let Err(e) = sm.get_or_create(key).await {
            warn!("Failed to ensure session in SessionManager: {}", e);
        }
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: no new errors

**Step 3: Commit**

```bash
git add core/src/gateway/agent_instance.rs
git commit -m "gateway: add AgentInstance::ensure_session for in-memory + SQLite pre-creation"
```

---

## Task 2: Call `ensure_session` in ExecutionEngine before `add_message`

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs:169-172`

**Step 1: Insert ensure_session call**

Find line 169 (the comment `// Store user message in session`). Change:

```rust
        // Store user message in session
        agent
            .add_message(&request.session_key, MessageRole::User, &request.input)
            .await;
```

To:

```rust
        // Ensure session exists in memory + SQLite before adding messages
        agent.ensure_session(&request.session_key).await;

        // Store user message in session
        agent
            .add_message(&request.session_key, MessageRole::User, &request.input)
            .await;
```

**Step 2: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: no new errors

**Step 3: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs
git commit -m "engine: call ensure_session before add_message to fix persistence"
```

---

## Task 3: Add `SessionUpdated` variant to `StreamEvent`

**Files:**
- Modify: `core/src/gateway/event_emitter.rs:22-129` (StreamEvent enum)
- Modify: `core/src/gateway/event_emitter.rs:678-692` (event_method fn)

**Step 1: Add SessionUpdated variant**

In the `StreamEvent` enum (after `UncertaintySignal` variant, before the closing `}`), add:

```rust
    /// Session was updated (new messages added)
    ///
    /// Emitted after a run completes so that UI sidebars can refresh
    /// their session list without polling.
    SessionUpdated {
        session_key: String,
    },
```

**Step 2: Add event method mapping**

In the `event_method` function, add a new arm before the closing `}`:

```rust
        StreamEvent::SessionUpdated { .. } => "stream.session_updated",
```

**Step 3: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: no new errors (serde will auto-derive for the new variant)

**Step 4: Commit**

```bash
git add core/src/gateway/event_emitter.rs
git commit -m "gateway: add SessionUpdated stream event for sidebar refresh"
```

---

## Task 4: Emit `SessionUpdated` after run completion in ExecutionEngine

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs:226-246` (the `Ok(response)` arm)

**Step 1: Emit SessionUpdated after RunComplete**

Find the `Ok(response)` match arm inside the `execute()` method (around line 226). After the existing `emit RunComplete` block, add the SessionUpdated emission. Change:

```rust
                let _ = emitter
                    .emit(StreamEvent::RunComplete {
                        run_id: run_id.clone(),
                        seq: final_seq,
                        summary: RunSummary {
                            total_tokens: 0,
                            tool_calls: 0,
                            loops: steps_completed,
                            final_response: Some(response.clone()),
                        },
                        total_duration_ms: duration_ms,
                    })
                    .await;
                Ok(())
```

To:

```rust
                let _ = emitter
                    .emit(StreamEvent::RunComplete {
                        run_id: run_id.clone(),
                        seq: final_seq,
                        summary: RunSummary {
                            total_tokens: 0,
                            tool_calls: 0,
                            loops: steps_completed,
                            final_response: Some(response.clone()),
                        },
                        total_duration_ms: duration_ms,
                    })
                    .await;

                // Notify UI that the session was updated
                let _ = emitter
                    .emit(StreamEvent::SessionUpdated {
                        session_key: request.session_key.to_key_string(),
                    })
                    .await;

                Ok(())
```

**Step 2: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: no new errors

**Step 3: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs
git commit -m "engine: emit SessionUpdated event after successful run completion"
```

---

## Task 5: Add `memory_backend` field to ExecutionEngine

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs:33-66` (struct + new())

**Step 1: Add field to struct**

In the `ExecutionEngine` struct, after `workspace_manager`, add:

```rust
    /// Memory backend for auto-memorization of conversations
    memory_backend: Option<crate::memory::store::MemoryBackend>,
```

**Step 2: Update `new()` to accept memory_backend**

Change the `new()` signature and body:

```rust
    pub fn new(
        config: ExecutionEngineConfig,
        provider_registry: Arc<P>,
        tool_registry: Arc<R>,
        tools: Vec<UnifiedTool>,
        session_manager: Arc<crate::gateway::SessionManager>,
        memory_backend: Option<crate::memory::store::MemoryBackend>,
    ) -> Self {
        Self {
            config,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
            provider_registry,
            tool_registry,
            tools: Arc::new(tools),
            session_manager,
            workspace_manager: None,
            memory_backend,
        }
    }
```

**Step 3: Verify it compiles — expect call-site errors**

Run: `cargo check -p alephcore 2>&1 | head -30`
Expected: errors at call sites (start/mod.rs, server_init.rs) — we'll fix those in Task 7

**Step 4: Commit (WIP — call sites fixed in Task 7)**

```bash
git add core/src/gateway/execution_engine/engine.rs
git commit -m "engine: add memory_backend field for auto-memorization pipeline"
```

---

## Task 6: Write conversation memory after run completion

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs` (inside `execute()`, after SessionUpdated emission)

**Step 1: Add memory write helper function**

At the bottom of `engine.rs` (before any `#[cfg(test)]` block or at the end of the impl block), add this standalone async function:

```rust
/// Write a conversation turn to the memory system (Layer 1).
///
/// Runs in a background task — failures are logged but never block the caller.
async fn write_conversation_memory(
    memory_backend: crate::memory::store::MemoryBackend,
    session_key: String,
    user_input: String,
    ai_output: String,
) {
    use crate::memory::context::{ContextAnchor, MemoryEntry};

    let context = ContextAnchor::with_topic(
        "aleph.chat".to_string(),
        session_key.clone(),
        session_key,
    );
    let entry = MemoryEntry::new(
        uuid::Uuid::new_v4().to_string(),
        context,
        user_input,
        ai_output,
    );

    use crate::memory::store::SessionStore;
    if let Err(e) = memory_backend.insert_memory(&entry).await {
        warn!("Failed to write conversation memory: {}", e);
    } else {
        debug!("Conversation memory saved to Layer 1");
    }
}
```

**Step 2: Spawn memory write in execute()**

Inside the `Ok(response)` arm, after the `SessionUpdated` emission (added in Task 4), add:

```rust
                // Async write to memory system (Layer 1)
                if let Some(ref mb) = self.memory_backend {
                    let mb = mb.clone();
                    let sk = request.session_key.to_key_string();
                    let ui = request.input.clone();
                    let ao = response.clone();
                    tokio::spawn(async move {
                        write_conversation_memory(mb, sk, ui, ao).await;
                    });
                }
```

**Step 3: Verify it compiles (still expect call-site errors)**

Run: `cargo check -p alephcore 2>&1 | grep "error\[" | head -10`
Expected: only call-site errors from `ExecutionEngine::new()` arity change

**Step 4: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs
git commit -m "engine: async write conversation memory to LanceDB Layer 1 after run"
```

---

## Task 7: Fix call sites — pass `memory_backend` to ExecutionEngine::new()

**Files:**
- Modify: `core/src/bin/aleph_server/commands/start/mod.rs:346-352`
- Modify: `core/src/bin/aleph_server/server_init.rs` (if applicable)

**Step 1: Fix start/mod.rs**

Find the `ExecutionEngine::new()` call (around line 346). Change:

```rust
        let engine = Arc::new(ExecutionEngine::new(
            ExecutionEngineConfig::default(),
            provider_registry,
            tool_registry,
            tools,
            session_manager.clone(),
        ));
```

To:

```rust
        let engine = Arc::new(ExecutionEngine::new(
            ExecutionEngineConfig::default(),
            provider_registry,
            tool_registry,
            tools,
            session_manager.clone(),
            Some(memory_db.clone()),
        ));
```

**Step 2: Fix server_init.rs (if it has another call site)**

Search `server_init.rs` for any `ExecutionEngine::new()` call. If found, add the `memory_backend` parameter. Based on the code read, `server_init.rs` uses `handle_run_with_engine` / `handle_chat_send_with_engine` which receive the engine as a parameter, so no change needed there.

**Step 3: Verify full compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: no errors

Run: `cargo check --bin aleph-server 2>&1 | head -20`
Expected: no errors

**Step 4: Commit**

```bash
git add core/src/bin/aleph_server/commands/start/mod.rs
git commit -m "server: pass memory_backend to ExecutionEngine for auto-memorization"
```

---

## Task 8: Update chat sidebar to subscribe to session events

**Files:**
- Modify: `core/ui/control_plane/src/components/chat_sidebar.rs`

**Step 1: Extract reload_sessions helper**

Replace the entire file content. The key changes are:
1. Extract session loading into a reusable `reload_sessions` closure
2. Subscribe to `stream.session_updated` events via `subscribe_events`
3. Re-fetch session list when event is received

```rust
// core/ui/control_plane/src/components/chat_sidebar.rs
//
// Chat mode sidebar — session list fetched from sessions.list RPC.
// Refreshes automatically via stream.session_updated events.
//
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use crate::context::DashboardState;
use crate::views::chat::state::ChatState;
use crate::api::chat::ChatApi;

/// A session entry returned by the backend.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionEntry {
    key: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    message_count: u32,
    #[serde(default)]
    last_active_at: String,
}

#[component]
pub fn ChatSidebar() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();

    let sessions = RwSignal::new(Vec::<SessionEntry>::new());
    let is_loading = RwSignal::new(false);

    // ---- Reusable session loader ----
    let dash_reload = dashboard;
    let reload_sessions = move || {
        let dash = dash_reload;
        spawn_local(async move {
            match dash.rpc_call("sessions.list", serde_json::json!({})).await {
                Ok(result) => {
                    if let Some(arr) = result.get("sessions") {
                        if let Ok(list) = serde_json::from_value::<Vec<SessionEntry>>(arr.clone()) {
                            sessions.set(list);
                        }
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to list sessions: {e}").into());
                }
            }
            is_loading.set(false);
        });
    };

    // ---- Initial load on connect ----
    let reload_for_connect = reload_sessions.clone();
    let dash_connect = dashboard;
    Effect::new(move || {
        if dash_connect.is_connected.get() {
            is_loading.set(true);
            reload_for_connect();
        }
    });

    // ---- Subscribe to session_updated events for auto-refresh ----
    let reload_for_events = reload_sessions.clone();
    let sub_id = dashboard.subscribe_events(move |event| {
        // stream.session_updated arrives as topic "run.session_updated"
        if event.topic.contains("session_updated") {
            reload_for_events();
        }
    });

    // Subscribe to stream.session_updated on the Gateway
    let dash_for_sub = dashboard;
    spawn_local(async move {
        // Wait until connected
        for _ in 0..50 {
            if dash_for_sub.is_connected.get_untracked() { break; }
            gloo_timers::future::TimeoutFuture::new(100).await;
        }
        if let Err(e) = dash_for_sub.subscribe_topic("stream.session_updated").await {
            web_sys::console::error_1(&format!("Failed to subscribe session events: {e}").into());
        }
    });

    // Cleanup subscription on unmount
    let dash_cleanup = dashboard;
    on_cleanup(move || {
        dash_cleanup.unsubscribe_events(sub_id);
    });

    let on_select_session = move |key: String| {
        let dash = dashboard;
        let current = chat.session_key.get_untracked();
        if current.as_deref() == Some(&key) {
            return; // already selected
        }
        // Clear current messages and set new session key
        chat.clear();
        chat.session_key.set(Some(key.clone()));

        // Load history for selected session
        spawn_local(async move {
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

    view! {
        <div class="flex flex-col h-full">
            // Search
            <div class="p-3">
                <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-sunken border border-border text-sm">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-text-tertiary flex-shrink-0">
                        <circle cx="11" cy="11" r="8" />
                        <line x1="21" y1="21" x2="16.65" y2="16.65" />
                    </svg>
                    <span class="text-text-tertiary">"Search chats..."</span>
                </div>
            </div>

            // Session list
            <div class="flex-1 overflow-y-auto px-3 py-2 space-y-1">
                {move || {
                    let list = sessions.get();
                    let active_key = chat.session_key.get();
                    if is_loading.get() {
                        view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                "Loading sessions..."
                            </p>
                        }.into_any()
                    } else if list.is_empty() {
                        view! {
                            <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                                "Start a new conversation"
                            </p>
                        }.into_any()
                    } else {
                        view! {
                            <For
                                each=move || sessions.get()
                                key=|s| s.key.clone()
                                children=move |session| {
                                    let key = session.key.clone();
                                    let key_for_click = key.clone();
                                    let is_active = move || {
                                        chat.session_key.get().as_deref() == Some(&key)
                                    };
                                    let on_select = on_select_session.clone();
                                    let label = format_session_label(&session);
                                    let subtitle = format_session_subtitle(&session);
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
                                            on:click=move |_| on_select(key_for_click.clone())
                                        >
                                            <div class="truncate font-medium text-xs">{label}</div>
                                            <div class="truncate text-[10px] text-text-tertiary mt-0.5">{subtitle}</div>
                                        </button>
                                    }
                                }
                            />
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

fn format_session_label(session: &SessionEntry) -> String {
    // Extract meaningful part from session key like "agent:main:main" -> "main"
    let parts: Vec<&str> = session.key.split(':').collect();
    if parts.len() >= 3 {
        format!("{} ({})", parts[1], parts[2])
    } else {
        session.key.clone()
    }
}

fn format_session_subtitle(session: &SessionEntry) -> String {
    let msg_count = session.message_count;
    if session.last_active_at.is_empty() {
        format!("{msg_count} messages")
    } else {
        // Show short date from ISO 8601
        let date = session.last_active_at
            .split('T')
            .next()
            .unwrap_or(&session.last_active_at);
        format!("{msg_count} msgs - {date}")
    }
}
```

**Step 2: Verify WASM compiles**

This is a Leptos/WASM component. The full WASM build requires `wasm-pack` or `trunk`. For now verify that the Rust syntax is valid:

Run: `cargo check -p alephcore --features control-plane 2>&1 | head -20`

If the control-plane UI is a separate crate, check the appropriate target. If it doesn't compile standalone, verify syntax by reading.

**Step 3: Commit**

```bash
git add core/ui/control_plane/src/components/chat_sidebar.rs
git commit -m "ui: sidebar auto-refreshes via session_updated event subscription"
```

---

## Task 9: Run existing tests to verify no regressions

**Step 1: Run core tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: existing tests pass (pre-existing `markdown_skill::loader` failures are known — ignore)

**Step 2: Run execution engine tests specifically**

Run: `cargo test -p alephcore --lib execution_engine 2>&1 | tail -20`
Expected: PASS (or no tests — the engine tests may use `CollectingEventEmitter` which already handles unknown variants)

**Step 3: Run event_emitter tests**

Run: `cargo test -p alephcore --lib event_emitter 2>&1 | tail -20`
Expected: PASS

---

## Task 10: Full build verification and final commit

**Step 1: Full check**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: no errors

Run: `cargo check --bin aleph-server 2>&1 | tail -5`
Expected: no errors

**Step 2: Rebuild WASM if applicable**

If the control-plane WASM builds separately:
Run: `cd core/ui/control_plane && wasm-pack build --target web 2>&1 | tail -10`

**Step 3: Final integration commit (if needed)**

```bash
git add -A
git status
# Only commit if there are remaining unstaged changes
git commit -m "chore: fix remaining compilation issues from persistence + memory pipeline"
```

---

## Verification Checklist

After implementation, manually verify:

1. **Start server**: `cargo run --bin aleph-server --features control-plane`
2. **Open ControlPlane**: `http://localhost:8081`
3. **Send 2-3 chat messages** via the Chat view
4. **Check sidebar**: Should show the session with message count updating in real-time
5. **Close browser tab, reopen**: Session should still appear in sidebar with correct message count
6. **Check Memory Dashboard**: Should show the conversation entries (user_input + ai_output)
7. **Check server logs**: Should see "Conversation memory saved to Layer 1" debug messages
