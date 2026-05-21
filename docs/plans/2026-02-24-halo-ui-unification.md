# Halo UI Unification Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace three separate Halo UI codebases (Swift, React, none-yet-in-Leptos) with a single Leptos `/halo` route served at `localhost:18790/halo`, then wire macOS and Tauri shells to load it.

**Architecture:** Build a new `/halo` route in the Leptos Control Plane that acts as a full chat interface — sending messages via `chat.send` RPC, receiving streaming events via WebSocket subscriptions (`run.*`), and rendering markdown responses. macOS `HaloWindow` switches from `NSHostingView<HaloViewV2>` to `WKWebView`. Tauri updates the halo window URL. Legacy Swift and React code is deleted.

**Tech Stack:** Leptos 0.8 (WASM), Tailwind CSS, WebSocket (JSON-RPC 2.0), WKWebView (macOS), Tauri WebView (Windows/Linux)

---

## Context for Implementer

### Gateway RPC Methods (Chat)

| Method | Params | Returns | Notes |
|--------|--------|---------|-------|
| `chat.send` | `{message, session_key?, channel?, stream?, thinking?}` | `{run_id, session_key, streaming}` | Starts agent run |
| `chat.abort` | `{run_id}` | `{}` | Cancels running agent |
| `chat.history` | `{session_key, limit?, before?}` | `{messages: [...]}` | Get conversation history |
| `chat.clear` | `{session_key}` | `{}` | Clear session |

### Streaming Events (subscribe to `run.*`)

Events arrive as `{method: "event", params: {topic: "run.*", data: StreamEvent}}`:

| Event Type | Key Fields | UI Action |
|------------|-----------|-----------|
| `run_accepted` | `run_id, session_key` | Show thinking spinner |
| `reasoning` | `run_id, content, is_complete` | Show thinking text |
| `tool_start` | `run_id, tool_name, tool_id` | Show tool badge |
| `tool_end` | `run_id, tool_id, result, duration_ms` | Update tool status |
| `response_chunk` | `run_id, content, is_final` | Append to response text |
| `run_complete` | `run_id, summary` | Show complete response |
| `run_error` | `run_id, error` | Show error state |
| `ask_user` | `run_id, question, options` | Show clarification UI |

### Existing Leptos Patterns

- **State management**: `DashboardState` in context (`expect_context::<DashboardState>()`)
- **RPC calls**: `state.rpc_call("method", params).await`
- **Event subscription**: `state.subscribe_topic("run.*")` + `state.subscribe_events(handler)`
- **Routing**: Add `<Route path=path!("/halo") view=HaloView />` in `app.rs`
- **Styling**: Tailwind CSS classes, design tokens from `tailwind.config.js`
- **Components**: Files in `core/ui/control_plane/src/components/`
- **Views**: Files in `core/ui/control_plane/src/views/`

### Key File Locations

| File | Purpose |
|------|---------|
| `core/ui/control_plane/src/app.rs` | Route definitions |
| `core/ui/control_plane/src/context.rs` | DashboardState (WS connection, RPC, events) |
| `core/ui/control_plane/src/api.rs` | API wrappers (AgentApi, ConfigApi, etc.) |
| `core/ui/control_plane/src/views/mod.rs` | View module exports |
| `core/ui/control_plane/src/components/mod.rs` | Component module exports |
| `apps/macos/Aleph/Sources/HaloWindow.swift` | macOS Halo window (NSWindow) |
| `apps/macos/Aleph/Sources/HaloState.swift` | Swift Halo state model |
| `apps/macos/Aleph/Sources/Components/HaloViewV2.swift` | Swift Halo main view (725 lines) |
| `apps/desktop/src-tauri/tauri.conf.json` | Tauri window definitions |
| `apps/desktop/src/windows/halo/` | React Halo components (20+ files) |

---

## Task 1: Chat API Module

**Files:**
- Create: `core/ui/control_plane/src/api/chat.rs`
- Modify: `core/ui/control_plane/src/api.rs` (add `pub mod chat;` at top)

**Step 1: Create chat API wrapper**

```rust
// core/ui/control_plane/src/api/chat.rs
//! Chat API — wraps chat.send / chat.abort / chat.history / chat.clear RPC methods.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

/// A single chat message (from history).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,        // "user" | "assistant" | "system"
    pub content: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

/// Response from chat.send
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSendResponse {
    pub run_id: String,
    pub session_key: String,
    pub streaming: bool,
}

pub struct ChatApi;

impl ChatApi {
    /// Send a message and start an agent run.
    pub async fn send(
        state: &DashboardState,
        message: &str,
        session_key: Option<&str>,
    ) -> Result<ChatSendResponse, String> {
        let params = serde_json::json!({
            "message": message,
            "session_key": session_key,
            "channel": "gui:halo",
            "stream": true,
        });
        let result = state.rpc_call("chat.send", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Abort a running agent.
    pub async fn abort(state: &DashboardState, run_id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "run_id": run_id });
        state.rpc_call("chat.abort", params).await?;
        Ok(())
    }

    /// Get chat history for a session.
    pub async fn history(
        state: &DashboardState,
        session_key: &str,
        limit: Option<usize>,
    ) -> Result<Vec<ChatMessage>, String> {
        let params = serde_json::json!({
            "session_key": session_key,
            "limit": limit,
        });
        let result = state.rpc_call("chat.history", params).await?;
        let messages = result.get("messages").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(messages).map_err(|e| e.to_string())
    }

    /// Clear chat history for a session.
    pub async fn clear(state: &DashboardState, session_key: &str) -> Result<(), String> {
        let params = serde_json::json!({ "session_key": session_key });
        state.rpc_call("chat.clear", params).await?;
        Ok(())
    }
}
```

**Step 2: Register module**

In `core/ui/control_plane/src/api.rs`, add at line 1 (before `use serde`):

```rust
pub mod chat;
```

**Step 3: Build to verify**

```bash
cd core/ui/control_plane && cargo build --lib --target wasm32-unknown-unknown 2>&1 | tail -5
```

Expected: compiles with 0 errors (warnings OK).

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/api/chat.rs core/ui/control_plane/src/api.rs
git commit -m "feat(halo): add ChatApi module for chat.send/abort/history/clear"
```

---

## Task 2: Halo State Signals

**Files:**
- Create: `core/ui/control_plane/src/views/halo/state.rs`
- Create: `core/ui/control_plane/src/views/halo/mod.rs`
- Modify: `core/ui/control_plane/src/views/mod.rs` (add `pub mod halo;`)

**Step 1: Create halo state module**

```rust
// core/ui/control_plane/src/views/halo/state.rs
//! Halo reactive state — signals for chat messages, streaming, and UI mode.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A rendered chat message (user or assistant).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HaloMessage {
    pub id: String,
    pub role: String,           // "user" | "assistant"
    pub content: String,        // final or accumulated text
    #[serde(default)]
    pub tool_calls: Vec<ToolCallEntry>,
    #[serde(default)]
    pub is_streaming: bool,     // true while response_chunks arrive
    #[serde(default)]
    pub error: Option<String>,
}

/// Minimal tool call record for display.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallEntry {
    pub tool_id: String,
    pub tool_name: String,
    pub status: String,    // "running" | "completed" | "failed"
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// Top-level Halo UI phase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HaloPhase {
    Idle,
    Thinking,
    Streaming,
    Error,
}

/// Reactive state container provided via Leptos context.
#[derive(Clone, Copy)]
pub struct HaloState {
    /// All messages in the current session.
    pub messages: RwSignal<Vec<HaloMessage>>,
    /// Current phase of the UI.
    pub phase: RwSignal<HaloPhase>,
    /// Active run_id (Some while agent is running).
    pub active_run_id: RwSignal<Option<String>>,
    /// Resolved session key from first chat.send response.
    pub session_key: RwSignal<Option<String>>,
    /// Accumulated reasoning text for the current run.
    pub reasoning_text: RwSignal<String>,
    /// Error message (set when run_error arrives).
    pub error_message: RwSignal<Option<String>>,
}

impl HaloState {
    pub fn new() -> Self {
        Self {
            messages: RwSignal::new(Vec::new()),
            phase: RwSignal::new(HaloPhase::Idle),
            active_run_id: RwSignal::new(None),
            session_key: RwSignal::new(None),
            reasoning_text: RwSignal::new(String::new()),
            error_message: RwSignal::new(None),
        }
    }

    /// Append a user message and reset error state.
    pub fn push_user_message(&self, text: &str) {
        let id = format!("user-{}", self.messages.with(|m| m.len()));
        self.messages.update(|msgs| {
            msgs.push(HaloMessage {
                id,
                role: "user".into(),
                content: text.to_string(),
                tool_calls: vec![],
                is_streaming: false,
                error: None,
            });
        });
        self.error_message.set(None);
    }

    /// Start a new assistant message placeholder (streaming).
    pub fn start_assistant_message(&self, run_id: &str) {
        let id = format!("assistant-{}", run_id);
        self.messages.update(|msgs| {
            msgs.push(HaloMessage {
                id,
                role: "assistant".into(),
                content: String::new(),
                tool_calls: vec![],
                is_streaming: true,
                error: None,
            });
        });
        self.active_run_id.set(Some(run_id.to_string()));
        self.phase.set(HaloPhase::Thinking);
        self.reasoning_text.set(String::new());
    }

    /// Append a response text chunk to the current assistant message.
    pub fn append_chunk(&self, run_id: &str, content: &str) {
        let target_id = format!("assistant-{}", run_id);
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                msg.content.push_str(content);
            }
        });
        if self.phase.get() != HaloPhase::Streaming {
            self.phase.set(HaloPhase::Streaming);
        }
    }

    /// Record a tool call event.
    pub fn update_tool(&self, run_id: &str, tool_id: &str, tool_name: &str, status: &str, duration_ms: Option<u64>) {
        let target_id = format!("assistant-{}", run_id);
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                if let Some(tc) = msg.tool_calls.iter_mut().find(|t| t.tool_id == tool_id) {
                    tc.status = status.to_string();
                    tc.duration_ms = duration_ms;
                } else {
                    msg.tool_calls.push(ToolCallEntry {
                        tool_id: tool_id.to_string(),
                        tool_name: tool_name.to_string(),
                        status: status.to_string(),
                        duration_ms,
                    });
                }
            }
        });
    }

    /// Finalize current run (mark message as not streaming).
    pub fn complete_run(&self, run_id: &str) {
        let target_id = format!("assistant-{}", run_id);
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                msg.is_streaming = false;
            }
        });
        self.active_run_id.set(None);
        self.phase.set(HaloPhase::Idle);
    }

    /// Mark current run as errored.
    pub fn fail_run(&self, run_id: &str, error: &str) {
        let target_id = format!("assistant-{}", run_id);
        self.messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().rev().find(|m| m.id == target_id) {
                msg.is_streaming = false;
                msg.error = Some(error.to_string());
            }
        });
        self.active_run_id.set(None);
        self.phase.set(HaloPhase::Error);
        self.error_message.set(Some(error.to_string()));
    }

    /// Clear all messages and reset state.
    pub fn clear(&self) {
        self.messages.set(Vec::new());
        self.phase.set(HaloPhase::Idle);
        self.active_run_id.set(None);
        self.session_key.set(None);
        self.reasoning_text.set(String::new());
        self.error_message.set(None);
    }
}
```

**Step 2: Create halo view module**

```rust
// core/ui/control_plane/src/views/halo/mod.rs
pub mod state;

pub use state::HaloState;
```

**Step 3: Register in views/mod.rs**

Add this line to `core/ui/control_plane/src/views/mod.rs`:

```rust
pub mod halo;
```

**Step 4: Build to verify**

```bash
cd core/ui/control_plane && cargo build --lib --target wasm32-unknown-unknown 2>&1 | tail -5
```

**Step 5: Commit**

```bash
git add core/ui/control_plane/src/views/halo/
git add core/ui/control_plane/src/views/mod.rs
git commit -m "feat(halo): add HaloState reactive signals for chat state management"
```

---

## Task 3: Event Handler — Wire Streaming Events to HaloState

**Files:**
- Create: `core/ui/control_plane/src/views/halo/events.rs`
- Modify: `core/ui/control_plane/src/views/halo/mod.rs` (add `pub mod events;`)

**Step 1: Create event handler**

```rust
// core/ui/control_plane/src/views/halo/events.rs
//! Maps Gateway streaming events (run.*) to HaloState mutations.

use serde_json::Value;
use crate::context::{DashboardState, GatewayEvent};
use super::state::HaloState;

/// Subscribe to `run.*` events and dispatch to HaloState.
/// Returns the subscription ID for cleanup.
pub fn subscribe_run_events(dashboard: &DashboardState, halo: HaloState) -> usize {
    dashboard.subscribe_events(move |event: GatewayEvent| {
        if !event.topic.starts_with("run.") {
            return;
        }

        let data = &event.data;

        // Extract event type from the "type" field
        let event_type = data.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let run_id = data.get("run_id").and_then(|r| r.as_str()).unwrap_or("");

        match event_type {
            "run_accepted" => {
                if let Some(sk) = data.get("session_key").and_then(|s| s.as_str()) {
                    halo.session_key.set(Some(sk.to_string()));
                }
                halo.start_assistant_message(run_id);
            }
            "reasoning" => {
                if let Some(content) = data.get("content").and_then(|c| c.as_str()) {
                    halo.reasoning_text.update(|t| t.push_str(content));
                }
            }
            "tool_start" => {
                let name = data.get("tool_name").and_then(|n| n.as_str()).unwrap_or("tool");
                let tool_id = data.get("tool_id").and_then(|t| t.as_str()).unwrap_or("");
                halo.update_tool(run_id, tool_id, name, "running", None);
            }
            "tool_end" => {
                let tool_id = data.get("tool_id").and_then(|t| t.as_str()).unwrap_or("");
                let status = data.get("result")
                    .and_then(|r| r.get("success"))
                    .and_then(|s| s.as_bool())
                    .map(|ok| if ok { "completed" } else { "failed" })
                    .unwrap_or("completed");
                let duration = data.get("duration_ms").and_then(|d| d.as_u64());
                halo.update_tool(run_id, tool_id, "", status, duration);
            }
            "response_chunk" => {
                if let Some(content) = data.get("content").and_then(|c| c.as_str()) {
                    halo.append_chunk(run_id, content);
                }
            }
            "run_complete" => {
                halo.complete_run(run_id);
            }
            "run_error" => {
                let error = data.get("error").and_then(|e| e.as_str()).unwrap_or("Unknown error");
                halo.fail_run(run_id, error);
            }
            _ => {} // Ignore unknown event types
        }
    })
}
```

**Step 2: Register module**

In `core/ui/control_plane/src/views/halo/mod.rs`, add:

```rust
pub mod events;
```

**Step 3: Build to verify**

```bash
cd core/ui/control_plane && cargo build --lib --target wasm32-unknown-unknown 2>&1 | tail -5
```

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/views/halo/events.rs
git add core/ui/control_plane/src/views/halo/mod.rs
git commit -m "feat(halo): add event handler to wire run.* streaming events to HaloState"
```

---

## Task 4: Halo View Component — Main Chat UI

**Files:**
- Create: `core/ui/control_plane/src/views/halo/view.rs`
- Modify: `core/ui/control_plane/src/views/halo/mod.rs` (add module + re-export)
- Modify: `core/ui/control_plane/src/app.rs` (add `/halo` route)

**Step 1: Create main Halo view**

```rust
// core/ui/control_plane/src/views/halo/view.rs
//! Main Halo chat view — message list + input area.

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::chat::ChatApi;
use super::state::{HaloState, HaloPhase, HaloMessage};
use super::events::subscribe_run_events;

/// Top-level Halo view component.
#[component]
pub fn HaloView() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let halo = HaloState::new();
    provide_context(halo);

    // Subscribe to run.* events on mount
    let cleanup_id = StoredValue::new(None::<usize>);
    Effect::new(move || {
        let dashboard = expect_context::<DashboardState>();
        let halo = expect_context::<HaloState>();

        spawn_local(async move {
            if let Err(e) = dashboard.subscribe_topic("run.*").await {
                web_sys::console::error_1(&format!("Failed to subscribe run events: {e}").into());
            }
        });

        let id = subscribe_run_events(&dashboard, halo);
        cleanup_id.set_value(Some(id));
    });

    on_cleanup(move || {
        if let Some(id) = cleanup_id.get_value() {
            dashboard.unsubscribe_events(id);
        }
    });

    view! {
        <div class="flex flex-col h-screen bg-surface">
            // Message list (scrollable)
            <MessageList />
            // Input area (pinned to bottom)
            <InputArea />
        </div>
    }
}

/// Scrollable message list.
#[component]
fn MessageList() -> impl IntoView {
    let halo = expect_context::<HaloState>();

    view! {
        <div class="flex-1 overflow-y-auto px-4 py-6 space-y-4">
            <For
                each=move || halo.messages.get()
                key=|msg| msg.id.clone()
                children=move |msg| {
                    view! { <MessageBubble message=msg /> }
                }
            />
            // Thinking indicator
            <Show when=move || halo.phase.get() == HaloPhase::Thinking>
                <div class="flex items-center gap-2 text-text-secondary text-sm px-3 py-2">
                    <span class="inline-block w-2 h-2 rounded-full bg-primary animate-pulse"></span>
                    "Thinking..."
                </div>
            </Show>
        </div>
    }
}

/// Single message bubble.
#[component]
fn MessageBubble(message: HaloMessage) -> impl IntoView {
    let is_user = message.role == "user";
    let has_error = message.error.is_some();
    let has_tools = !message.tool_calls.is_empty();

    view! {
        <div class=move || {
            if is_user {
                "flex justify-end"
            } else {
                "flex justify-start"
            }
        }>
            <div class=move || {
                let base = if is_user {
                    "max-w-[80%] rounded-2xl px-4 py-3 bg-primary text-white"
                } else if has_error {
                    "max-w-[80%] rounded-2xl px-4 py-3 bg-danger-subtle text-danger border border-danger/20"
                } else {
                    "max-w-[80%] rounded-2xl px-4 py-3 bg-surface-raised text-text-primary"
                };
                base.to_string()
            }>
                // Tool calls (show above content)
                {if has_tools {
                    let tools = message.tool_calls.clone();
                    Some(view! {
                        <div class="mb-2 space-y-1">
                            {tools.into_iter().map(|tc| {
                                let status_color = match tc.status.as_str() {
                                    "running" => "text-warning",
                                    "completed" => "text-success",
                                    "failed" => "text-danger",
                                    _ => "text-text-secondary",
                                };
                                let duration_text = tc.duration_ms
                                    .map(|d| format!(" ({d}ms)"))
                                    .unwrap_or_default();
                                view! {
                                    <div class="flex items-center gap-2 text-xs font-mono">
                                        <span class=status_color>
                                            {match tc.status.as_str() {
                                                "running" => "⟳",
                                                "completed" => "✓",
                                                "failed" => "✗",
                                                _ => "·",
                                            }}
                                        </span>
                                        <span class="text-text-secondary">{tc.tool_name.clone()}</span>
                                        <span class="text-text-tertiary">{duration_text}</span>
                                    </div>
                                }
                            }).collect::<Vec<_>>()}
                        </div>
                    })
                } else {
                    None
                }}

                // Message content
                <div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
                    {message.content.clone()}
                </div>

                // Streaming cursor
                {if message.is_streaming {
                    Some(view! {
                        <span class="inline-block w-1.5 h-4 bg-primary/60 animate-pulse ml-0.5 align-text-bottom"></span>
                    })
                } else {
                    None
                }}

                // Error message
                {message.error.clone().map(|err| view! {
                    <div class="mt-2 text-xs text-danger/80">{err}</div>
                })}
            </div>
        </div>
    }
}

/// Text input area with send button.
#[component]
fn InputArea() -> impl IntoView {
    let halo = expect_context::<HaloState>();
    let dashboard = expect_context::<DashboardState>();
    let input_text = RwSignal::new(String::new());
    let is_sending = RwSignal::new(false);

    let send_message = move || {
        let text = input_text.get().trim().to_string();
        if text.is_empty() { return; }

        is_sending.set(true);
        input_text.set(String::new());
        halo.push_user_message(&text);

        let session_key = halo.session_key.get();
        spawn_local(async move {
            let sk = session_key.as_deref();
            match ChatApi::send(&dashboard, &text, sk).await {
                Ok(resp) => {
                    halo.session_key.set(Some(resp.session_key));
                    // run_accepted event will trigger start_assistant_message
                }
                Err(e) => {
                    halo.error_message.set(Some(e.clone()));
                    halo.phase.set(HaloPhase::Error);
                }
            }
            is_sending.set(false);
        });
    };

    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            send_message();
        }
    };

    let can_send = Memo::new(move |_| {
        !input_text.get().trim().is_empty() && !is_sending.get()
    });

    // Abort button (visible when agent is running)
    let on_abort = move |_| {
        if let Some(run_id) = halo.active_run_id.get() {
            let dashboard = dashboard;
            spawn_local(async move {
                let _ = ChatApi::abort(&dashboard, &run_id).await;
            });
        }
    };

    view! {
        <div class="border-t border-border px-4 py-3">
            <div class="flex items-end gap-2">
                <textarea
                    class="flex-1 resize-none rounded-xl border border-border bg-surface-sunken px-4 py-2.5
                           text-sm text-text-primary placeholder:text-text-tertiary
                           focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary
                           min-h-[40px] max-h-[120px]"
                    placeholder="Send a message..."
                    rows=1
                    prop:value=move || input_text.get()
                    on:input=move |ev| {
                        input_text.set(event_target_value(&ev));
                    }
                    on:keydown=on_keydown
                />

                // Abort button (when running)
                <Show when=move || halo.active_run_id.get().is_some()>
                    <button
                        class="p-2.5 rounded-xl bg-danger/10 text-danger hover:bg-danger/20 transition-colors"
                        title="Stop"
                        on:click=on_abort
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5" viewBox="0 0 20 20" fill="currentColor">
                            <rect x="4" y="4" width="12" height="12" rx="2" />
                        </svg>
                    </button>
                </Show>

                // Send button (when idle)
                <Show when=move || halo.active_run_id.get().is_none()>
                    <button
                        class="p-2.5 rounded-xl bg-primary text-white hover:bg-primary/90
                               disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                        disabled=move || !can_send.get()
                        on:click=move |_| send_message()
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5" viewBox="0 0 20 20" fill="currentColor">
                            <path d="M3.105 2.288a.75.75 0 0 0-.826.95l1.414 4.926A1.5 1.5 0 0 0 5.135 9.25h6.115a.75.75 0 0 1 0 1.5H5.135a1.5 1.5 0 0 0-1.442 1.086l-1.414 4.926a.75.75 0 0 0 .826.95l14.095-5.61a.75.75 0 0 0 0-1.394L3.105 2.288Z" />
                        </svg>
                    </button>
                </Show>
            </div>
        </div>
    }
}
```

**Step 2: Update halo/mod.rs**

```rust
// core/ui/control_plane/src/views/halo/mod.rs
pub mod state;
pub mod events;
pub mod view;

pub use state::HaloState;
pub use view::HaloView;
```

**Step 3: Add `/halo` route to app.rs**

In `core/ui/control_plane/src/app.rs`:

Add import at top:
```rust
use crate::views::halo::HaloView;
```

Add route inside `<Routes>` block (after the `/memory` route, before settings routes):
```rust
<Route path=path!("/halo") view=HaloView />
```

**Step 4: Build to verify**

```bash
cd core/ui/control_plane && cargo build --lib --target wasm32-unknown-unknown 2>&1 | tail -10
```

**Step 5: Commit**

```bash
git add core/ui/control_plane/src/views/halo/view.rs
git add core/ui/control_plane/src/views/halo/mod.rs
git add core/ui/control_plane/src/app.rs
git commit -m "feat(halo): add /halo route with chat UI, message list, and input area"
```

---

## Task 5: Build WASM + Verify in Browser

**Files:**
- No new files — build pipeline verification

**Step 1: Build WASM library**

```bash
cd core/ui/control_plane && \
cargo build --lib --target wasm32-unknown-unknown --release 2>&1 | tail -5
```

**Step 2: Generate JS bindings**

```bash
wasm-bindgen --target web \
  --out-dir core/ui/control_plane/dist \
  --out-name aleph-dashboard \
  /Volumes/TBU4/Workspace/Aleph/target/wasm32-unknown-unknown/release/aleph_dashboard.wasm
```

**Step 3: Build Tailwind CSS**

```bash
cd core/ui/control_plane && npm run build:css
```

**Step 4: Rebuild server with embedded UI**

```bash
cargo build --bin aleph-server --features control-plane 2>&1 | tail -5
```

**Step 5: Commit updated dist artifacts**

```bash
git add core/ui/control_plane/dist/
git commit -m "build(halo): rebuild WASM with /halo route"
```

---

## Task 6: macOS — HaloWindow Switches to WKWebView

**Files:**
- Create: `apps/macos/Aleph/Sources/Components/HaloWebView.swift`
- Modify: `apps/macos/Aleph/Sources/HaloWindow.swift`

**Step 1: Create HaloWebView (WKWebView wrapper)**

```swift
// apps/macos/Aleph/Sources/Components/HaloWebView.swift
//
// WKWebView wrapper that loads the Leptos /halo route.
// Handles transparent background and server-unavailable detection.

import AppKit
import WebKit

/// WKWebView that hosts the Leptos Halo chat UI.
final class HaloWebView: WKWebView {

    private static let haloURL = URL(string: "http://127.0.0.1:18790/halo")!

    init() {
        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "drawsBackground")

        #if DEBUG
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")
        #endif

        super.init(frame: .zero, configuration: config)

        // Transparent background so NSWindow transparency works
        setValue(false, forKey: "drawsBackground")

        #if DEBUG
        isInspectable = true
        #endif

        navigationDelegate = self
        load(URLRequest(url: Self.haloURL))
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// Reload the Halo page (e.g., after server restart).
    func reload() {
        load(URLRequest(url: Self.haloURL))
    }
}

extension HaloWebView: WKNavigationDelegate {
    func webView(
        _ webView: WKWebView,
        didFailProvisionalNavigation navigation: WKNavigation!,
        withError error: Error
    ) {
        let nsError = error as NSError
        if nsError.domain == NSURLErrorDomain &&
           (nsError.code == NSURLErrorCannotConnectToHost ||
            nsError.code == NSURLErrorTimedOut) {
            print("[HaloWebView] Server unreachable — will retry on next show()")
        }
    }
}
```

**Step 2: Modify HaloWindow to use WKWebView**

Replace the `hostingView` property and `setupHostingView()` method in `HaloWindow.swift`.

Change the `hostingView` property from:
```swift
private var hostingView: NSHostingView<HaloViewV2>?
```
to:
```swift
private var webView: HaloWebView?
```

Replace `setupHostingView()` with:
```swift
private func setupWebView() {
    let haloWeb = HaloWebView()
    haloWeb.frame = contentView?.bounds ?? .zero
    haloWeb.autoresizingMask = [.width, .height]
    contentView = haloWeb
    self.webView = haloWeb
}
```

In `init()`, change `setupHostingView()` to `setupWebView()`.

Remove the `viewModel` property (no longer needed — state lives in Leptos now).

Remove `import SwiftUI` (no longer needed).

Remove or simplify all the legacy state bridge methods (`showProcessingWithAI`, `showProcessing`, `showSuccess`, etc.) — the WebView handles everything. Keep `show(at:)`, `showCentered()`, `hide()`, `forceHide()` as they manage window visibility.

Remove `canBecomeKey` override that references `viewModel.state.isInteractive` — always return `true` since WebView needs key focus.

Update `updateState()` and other methods that reference `viewModel` — remove them.

**Step 3: Verify Swift syntax**

```bash
~/.uv/python3/bin/python Scripts/verify_swift_syntax.py apps/macos/Aleph/Sources/Components/HaloWebView.swift
~/.uv/python3/bin/python Scripts/verify_swift_syntax.py apps/macos/Aleph/Sources/HaloWindow.swift
```

**Step 4: Commit**

```bash
git add apps/macos/Aleph/Sources/Components/HaloWebView.swift
git add apps/macos/Aleph/Sources/HaloWindow.swift
git commit -m "feat(halo): switch macOS HaloWindow from SwiftUI to WKWebView"
```

---

## Task 7: macOS — Delete Legacy Swift Halo Files

**Files:**
- Delete: `apps/macos/Aleph/Sources/Components/HaloViewV2.swift`
- Delete: `apps/macos/Aleph/Sources/Components/HaloStreamingView.swift`
- Delete: `apps/macos/Aleph/Sources/Components/HaloStreamingTypes.swift`
- Delete: `apps/macos/Aleph/Sources/Components/HaloHistoryListView.swift`
- Delete: `apps/macos/Aleph/Sources/Components/HaloResultView.swift`
- Delete: `apps/macos/Aleph/Sources/Components/HaloCommandListView.swift`
- Delete: `apps/macos/Aleph/Sources/Components/HaloLegacyTypes.swift`
- Delete: `apps/macos/Aleph/Sources/Components/HaloResultDetailPopover.swift`
- Delete: `apps/macos/Aleph/Sources/Components/HaloToastView.swift`
- Delete: `apps/macos/Aleph/Sources/Components/HaloListeningView.swift`
- Delete: `apps/macos/Aleph/Sources/HaloState.swift`
- Modify: Any files that import/reference deleted types (fix compilation)

**Step 1: Delete Halo SwiftUI view files**

```bash
cd /Volumes/TBU4/Workspace/Aleph/.claude/worktrees/ui-unification-phase1
rm -f apps/macos/Aleph/Sources/Components/HaloViewV2.swift
rm -f apps/macos/Aleph/Sources/Components/HaloStreamingView.swift
rm -f apps/macos/Aleph/Sources/Components/HaloStreamingTypes.swift
rm -f apps/macos/Aleph/Sources/Components/HaloHistoryListView.swift
rm -f apps/macos/Aleph/Sources/Components/HaloResultView.swift
rm -f apps/macos/Aleph/Sources/Components/HaloCommandListView.swift
rm -f apps/macos/Aleph/Sources/Components/HaloLegacyTypes.swift
rm -f apps/macos/Aleph/Sources/Components/HaloResultDetailPopover.swift
rm -f apps/macos/Aleph/Sources/Components/HaloToastView.swift
rm -f apps/macos/Aleph/Sources/Components/HaloListeningView.swift
rm -f apps/macos/Aleph/Sources/HaloState.swift
```

**Step 2: Fix references in remaining files**

Search for references to deleted types (`HaloViewModelV2`, `HaloViewV2`, `HaloState`, `StreamingContext`, `ConfirmationContext`, `ResultContext`, `ErrorContext`, etc.) in:
- `HaloWindow.swift` (should already be cleaned in Task 6)
- `EventHandler.swift` or similar coordinator files
- `AppDelegate.swift`

For any file that calls `haloWindow.updateState(...)` or `haloWindow.showConfirmation(...)`, these calls need to be removed or replaced with WebView communication (Phase 4+ if needed). For now, simply remove the calls that won't compile.

**Step 3: Verify with syntax checker**

```bash
~/.uv/python3/bin/python Scripts/verify_swift_syntax.py apps/macos/Aleph/Sources/HaloWindow.swift
```

**Step 4: Commit**

```bash
git add -A apps/macos/Aleph/Sources/Components/Halo*.swift
git add apps/macos/Aleph/Sources/HaloState.swift
git commit -m "refactor(halo): delete legacy Swift Halo views (~3000 lines)"
```

---

## Task 8: Tauri — Update Halo Window URL

**Files:**
- Modify: `apps/desktop/src-tauri/tauri.conf.json`

**Step 1: Update halo window URL**

In `apps/desktop/src-tauri/tauri.conf.json`, change the halo window's `url` from:
```json
"url": "/halo.html"
```
to:
```json
"url": "http://127.0.0.1:18790/halo"
```

Also update CSP to allow the server connection (if not already done — check for `connect-src`).

**Step 2: Commit**

```bash
git add apps/desktop/src-tauri/tauri.conf.json
git commit -m "feat(halo): point Tauri halo window to Leptos /halo route"
```

---

## Task 9: Tauri — Delete React Source

**Files:**
- Delete: `apps/desktop/src/` (entire React source directory)
- Delete: `apps/desktop/halo.html`
- Delete: `apps/desktop/settings.html`
- Delete: `apps/desktop/index.html`
- Delete: `apps/desktop/vite.config.ts`
- Delete: `apps/desktop/tailwind.config.ts`
- Delete: `apps/desktop/postcss.config.js`
- Delete: `apps/desktop/tsconfig.json`
- Delete: `apps/desktop/tsconfig.node.json`
- Keep: `apps/desktop/src-tauri/` (Rust backend)
- Keep: `apps/desktop/package.json` (but strip React dependencies)

**Step 1: Delete React source and build configs**

```bash
cd /Volumes/TBU4/Workspace/Aleph/.claude/worktrees/ui-unification-phase1
rm -rf apps/desktop/src/
rm -f apps/desktop/halo.html
rm -f apps/desktop/settings.html
rm -f apps/desktop/index.html
rm -f apps/desktop/vite.config.ts
rm -f apps/desktop/tailwind.config.ts
rm -f apps/desktop/postcss.config.js
rm -f apps/desktop/tsconfig.json
rm -f apps/desktop/tsconfig.node.json
```

**Step 2: Simplify package.json**

Strip all React/Vite/Tailwind dependencies from `apps/desktop/package.json`. Keep only:
- `@tauri-apps/api` (needed by Rust side for IPC types)
- `@tauri-apps/cli` (needed for `pnpm tauri dev`)

Remove: `react`, `react-dom`, `@radix-ui/*`, `tailwindcss`, `vite`, `zustand`, `framer-motion`, `i18next`, etc.

Remove all scripts except `tauri` (which invokes `cargo-tauri`).

**Step 3: Commit**

```bash
git add -A apps/desktop/
git commit -m "refactor(halo): delete React source from Tauri app (~56 files)"
```

---

## Task 10: Final Verification & Cleanup

**Files:**
- No new files — verification pass

**Step 1: Verify Leptos WASM builds**

```bash
cd core/ui/control_plane && cargo build --lib --target wasm32-unknown-unknown 2>&1 | tail -5
```

**Step 2: Verify Rust core builds**

```bash
cargo check --quiet 2>&1 | tail -5
```

**Step 3: Run core tests**

```bash
cargo test -p alephcore --lib 2>&1 | grep "test result"
```

Expected: `test result: ok. N passed; 0 failed`

**Step 4: Verify macOS Swift syntax**

```bash
for f in apps/macos/Aleph/Sources/HaloWindow.swift apps/macos/Aleph/Sources/Components/HaloWebView.swift; do
    echo "=== $f ==="
    ~/.uv/python3/bin/python Scripts/verify_swift_syntax.py "$f"
done
```

**Step 5: Final commit if any cleanup needed**

```bash
git add -A && git status
# Only commit if there are changes
git diff --cached --quiet || git commit -m "chore(halo): final cleanup after UI unification"
```

---

## Summary

| Task | Description | New Files | Deleted Files | Lines (est.) |
|------|-------------|-----------|---------------|-------------|
| 1 | Chat API module | 1 | 0 | +70 |
| 2 | Halo state signals | 2 | 0 | +170 |
| 3 | Event handler (run.* → HaloState) | 1 | 0 | +70 |
| 4 | Main Halo view component | 1 (+2 modified) | 0 | +250 |
| 5 | Build WASM + verify | 0 | 0 | 0 |
| 6 | macOS HaloWindow → WKWebView | 1 (+1 modified) | 0 | +50, -200 |
| 7 | Delete legacy Swift Halo | 0 | 11 | -3000 |
| 8 | Tauri halo URL update | 0 (+1 modified) | 0 | +1 |
| 9 | Delete React source | 0 | ~60 | -8000 |
| 10 | Final verification | 0 | 0 | 0 |

**Net effect:** ~+560 lines Leptos, ~-11,000 lines Swift+React deleted.
