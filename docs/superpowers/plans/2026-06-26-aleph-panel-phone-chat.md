# Phone Chat Screen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a native single-column phone Chat screen (session list → conversation → send/stream/stop/new-chat) that reuses the existing chat data layer, replacing the cramped desktop two-pane layout on `<640px`.

**Architecture:** A new `platform/phone/chat/` module renders the phone presentation only. `MainContent` swaps `ChatView` ↔ a new `PhoneChat` router by form factor, so exactly one mounts and owns the `run.*` streaming subscription. The phone screens reuse `ChatState`, `ChatApi`, `MessageList`, `sessions.list`, and `hydrate_session_history` verbatim; only the session-list landing and a minimal composer are new.

**Tech Stack:** Rust + Leptos 0.7 (reactive WASM), `aleph-panel` crate, iOS CSS classes in `styles/ios.css`.

## Global Constraints

- **Crate:** `aleph-panel` (lib `aleph_panel`), `interfaces/webchat/`. `rust-version = 1.92`.
- **No new dependencies** (the crate has no `[dev-dependencies]`; tests are plain native `#[test]`).
- **R4 (interface = pure I/O):** no business logic in phone screens — only render state + call existing `ChatApi`/RPCs.
- **Desktop layout must stay byte-identical** — no behavioural change to any `platform/wide/*` view. The only edits to existing files are: two `pub(super)`→`pub(crate)` visibility bumps, the `PhoneTabBar` active-state, and the `MainContent` Chat-branch swap.
- **Team/群聊 is out of scope and needs no filtering:** `sessions.list`'s server handler already excludes `task`/`ephemeral` session types (`src/gateway/handlers/session/db_handlers/query.rs:34`), and team chats are stored as `Task`-kind sessions — so they never appear in `sessions.list`. The phone list is single-agent by construction.
- **Cargo thrift (project rule "极度节制 cargo 调用"):** prefer rust-analyzer/LSP diagnostics per edit. Run `cargo check -p aleph-panel` only at the milestones noted (Task 1, after Task 6, Task 9). Run targeted `cargo test -p aleph-panel <name>` for the pure-logic tests in Task 2. Final visual verification is `just wasm` + iOS simulator (Task 9), not per-task builds.
- **Reused symbols (canonical paths):**
  - `crate::views::chat::ChatState` (re-exported), `crate::views::chat::state::{ChatPhase, ChatSendError}`
  - `crate::views::chat::messages::MessageList` (after Task 1)
  - `crate::views::chat::events::subscribe_run_events`
  - `crate::components::chat_sidebar::hydrate_session_history` (after Task 1)
  - `crate::api::chat::{ChatApi, ChatAttachment}`
  - `crate::context::DashboardState`, `crate::state::layout::WorkspaceState`
  - `crate::state::viewport::{FormFactor, FormFactorState}`
  - `crate::platform::phone::shell::{PhoneShell, PhoneTabBar}`
  - `crate::components::mode_sidebar::PanelMode`
- **`ChatApi::send` signature:** `async fn send(state: &DashboardState, message: &str, session_key: Option<&str>, attachments: Vec<ChatAttachment>, agent_id: Option<&str>, project_root: Option<&str>, model_override: Option<&crate::api::providers::ModelOverride>) -> Result<ChatSendResponse, String>` where `ChatSendResponse { run_id, session_key, streaming }`.
- **`ChatApi::abort(state, run_id) -> Result<(), String>`**; **`hydrate_session_history(dash: DashboardState, chat: ChatState, workspace: Option<WorkspaceState>, key: String)`** (async, loads `chat.messages` incl. trace replay).
- **`ChatState` methods:** `clear_session()` (clears messages/session_key/phase/team, keeps `agent_id`), `push_user_message(&str)`, `set_send_error(ChatSendError)`. Fields: `messages`, `phase: ChatPhase {Idle,Thinking,Streaming,Error}`, `active_run_id: RwSignal<Option<String>>`, `session_key: RwSignal<Option<String>>`, `agent_id: RwSignal<Option<String>>`, `active_project_root: RwSignal<Option<String>>`. `ChatSendError::classify(impl Into<String>) -> ChatSendError`.

---

### Task 1: Expose reused internals (visibility only)

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/messages.rs:104-106`
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:92`

**Interfaces:**
- Produces: `pub(crate) fn MessageList()` and `pub(crate) async fn hydrate_session_history(dash: DashboardState, chat: ChatState, workspace: Option<WorkspaceState>, key: String)` — both reachable from `platform/phone/chat/`.

- [ ] **Step 1: Bump `MessageList` visibility**

In `messages.rs`, change line 106 from:
```rust
pub(super) fn MessageList() -> impl IntoView {
```
to:
```rust
pub(crate) fn MessageList() -> impl IntoView {
```

- [ ] **Step 2: Bump `hydrate_session_history` visibility**

In `chat_sidebar.rs`, change line 92 from:
```rust
async fn hydrate_session_history(
```
to:
```rust
pub(crate) async fn hydrate_session_history(
```

- [ ] **Step 3: Verify it still compiles (no behaviour change)**

Run: `cargo check -p aleph-panel`
Expected: compiles with no new warnings/errors (a wider-than-needed `pub(crate)` on an item only used internally is fine; `hydrate_session_history` already has callers so no dead-code warning).

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/messages.rs interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: expose MessageList + hydrate_session_history as pub(crate) for phone reuse"
```

---

### Task 2: `SessionRow` DTO + sort helper (TDD)

**Files:**
- Create: `interfaces/webchat/src/platform/phone/chat/mod.rs`
- Create: `interfaces/webchat/src/platform/phone/chat/list.rs`
- Modify: `interfaces/webchat/src/platform/phone/mod.rs` (add `pub mod chat;`)

**Interfaces:**
- Produces: `pub(crate) struct SessionRow { key: String, agent_id: String, topic: Option<String>, message_count: u32, updated_at: Option<i64>, project_root: Option<String> }` (Deserialize) and `pub(crate) fn sort_sessions_desc(rows: Vec<SessionRow>) -> Vec<SessionRow>` (newest `updated_at` first, `None` last).

- [ ] **Step 1: Register the module**

In `interfaces/webchat/src/platform/phone/mod.rs`, add (next to the existing `pub mod settings;` / `pub mod shell;`):
```rust
pub mod chat;
```

Create `interfaces/webchat/src/platform/phone/chat/mod.rs` with just the submodule declarations for now:
```rust
//! Native iPhone Chat screens (single-agent). Mirrors the Settings phone
//! pattern: a session-list landing (`/`) drilling into a conversation
//! (`/chat`). Reuses ChatState / ChatApi / MessageList; only the list and a
//! minimal composer are phone-specific.

pub mod composer;
pub mod list;
pub mod thread;
```

(`composer.rs` and `thread.rs` are created in later tasks; create empty placeholder files now so the module compiles:)
```bash
printf '' > interfaces/webchat/src/platform/phone/chat/composer.rs
printf '' > interfaces/webchat/src/platform/phone/chat/thread.rs
```

- [ ] **Step 2: Write the failing tests**

Create `interfaces/webchat/src/platform/phone/chat/list.rs` with ONLY the type, helper stub, and tests:
```rust
//! Phone Chat landing — the session list.

use serde::Deserialize;

/// One row of `sessions.list`. Mirrors the server `SessionInfo` shape (only the
/// fields the phone list needs). Team chats never appear here — the server
/// filters out `task`/`ephemeral` session types.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct SessionRow {
    pub key: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub message_count: u32,
    /// Unix epoch seconds; `None` sorts last.
    #[serde(default)]
    pub updated_at: Option<i64>,
    #[serde(default)]
    pub project_root: Option<String>,
}

/// Sort newest-first by `updated_at`; rows with no timestamp sink to the bottom.
pub(crate) fn sort_sessions_desc(mut rows: Vec<SessionRow>) -> Vec<SessionRow> {
    rows.sort_by(|a, b| b.updated_at.unwrap_or(i64::MIN).cmp(&a.updated_at.unwrap_or(i64::MIN)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_sessions_list_row() {
        let json = serde_json::json!({
            "key": "agent-main:default",
            "agent_id": "agent-main",
            "topic": "Build the phone chat",
            "message_count": 7,
            "updated_at": 1_750_000_000_i64,
            "project_root": null
        });
        let row: SessionRow = serde_json::from_value(json).unwrap();
        assert_eq!(row.key, "agent-main:default");
        assert_eq!(row.agent_id, "agent-main");
        assert_eq!(row.topic.as_deref(), Some("Build the phone chat"));
        assert_eq!(row.message_count, 7);
        assert_eq!(row.updated_at, Some(1_750_000_000));
    }

    #[test]
    fn deserializes_with_missing_optional_fields() {
        let json = serde_json::json!({ "key": "k" });
        let row: SessionRow = serde_json::from_value(json).unwrap();
        assert_eq!(row.key, "k");
        assert_eq!(row.agent_id, "");
        assert_eq!(row.topic, None);
        assert_eq!(row.message_count, 0);
        assert_eq!(row.updated_at, None);
    }

    #[test]
    fn sorts_newest_first_none_last() {
        let rows = vec![
            SessionRow { key: "old".into(), agent_id: String::new(), topic: None, message_count: 0, updated_at: Some(100), project_root: None },
            SessionRow { key: "none".into(), agent_id: String::new(), topic: None, message_count: 0, updated_at: None, project_root: None },
            SessionRow { key: "new".into(), agent_id: String::new(), topic: None, message_count: 0, updated_at: Some(200), project_root: None },
        ];
        let sorted = sort_sessions_desc(rows);
        let keys: Vec<&str> = sorted.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["new", "old", "none"]);
    }
}
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p aleph-panel --lib platform::phone::chat::list::tests`
Expected: 3 tests PASS. (The helper + type are already implemented above — this task is logic-first; the component is added in Task 3 within the same file.)

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/phone/mod.rs interfaces/webchat/src/platform/phone/chat/
git commit -m "panel(phone): add chat module scaffold + SessionRow DTO and sort helper with tests"
```

---

### Task 3: `PhoneChatList` component (session-list landing)

**Files:**
- Modify: `interfaces/webchat/src/platform/phone/chat/list.rs`

**Interfaces:**
- Consumes: `SessionRow`, `sort_sessions_desc` (Task 2); `ChatState`, `DashboardState`, `WorkspaceState`, `hydrate_session_history`, `PhoneShell`.
- Produces: `pub fn PhoneChatList() -> impl IntoView`.

- [ ] **Step 1: Add imports + the component**

Prepend these imports below the existing `use serde::Deserialize;` in `list.rs`:
```rust
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::components::chat_sidebar::hydrate_session_history;
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use crate::state::layout::WorkspaceState;
use crate::views::chat::ChatState;
```

Append the component (after `sort_sessions_desc`, before `#[cfg(test)]`):
```rust
/// Phone Chat landing: a "+ New chat" row plus the session list. Tapping a row
/// loads that session into the shared ChatState and drills into `/chat`.
#[component]
#[must_use]
pub fn PhoneChatList() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let workspace = expect_context::<WorkspaceState>();
    let navigate = use_navigate();

    // loading | loaded(rows) | error(msg)
    let rows = RwSignal::new(Vec::<SessionRow>::new());
    let loading = RwSignal::new(true);
    let load_error = RwSignal::new(Option::<String>::None);

    // Fetch sessions.list on mount.
    {
        let dash = dashboard;
        spawn_local(async move {
            match dash.rpc_call("sessions.list", serde_json::json!({})).await {
                Ok(result) => {
                    let parsed = result
                        .get("sessions")
                        .cloned()
                        .and_then(|v| serde_json::from_value::<Vec<SessionRow>>(v).ok())
                        .unwrap_or_default();
                    rows.set(sort_sessions_desc(parsed));
                    loading.set(false);
                }
                Err(e) => {
                    load_error.set(Some(e));
                    loading.set(false);
                }
            }
        });
    }

    // New chat: clear the current session (keeps agent) → the first send creates
    // a fresh session server-side. No RPC needed up front.
    let on_new = {
        let navigate = navigate.clone();
        move |_| {
            chat.clear_session();
            navigate("/chat", NavigateOptions::default());
        }
    };

    // Select a session: set ChatState, restore project root, load history, drill in.
    let on_select = move |row: SessionRow| {
        let navigate = navigate.clone();
        let dash = dashboard;
        if chat.session_key.get_untracked().as_deref() == Some(row.key.as_str()) {
            navigate("/chat", NavigateOptions::default());
            return;
        }
        chat.clear_session();
        chat.agent_id.set(Some(row.agent_id.clone()));
        chat.session_key.set(Some(row.key.clone()));
        chat.active_project_root.set(row.project_root.clone());
        spawn_local(hydrate_session_history(dash, chat, Some(workspace), row.key.clone()));
        navigate("/chat", NavigateOptions::default());
    };

    view! {
        <PhoneShell title="Chat">
            <div class="list">
                <div class="cell" on:click=on_new>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"></line><line x1="5" y1="12" x2="19" y2="12"></line></svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"New chat"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
            </div>

            {move || {
                if loading.get() {
                    return view! { <div class="list-header">"Loading…"</div> }.into_any();
                }
                if let Some(err) = load_error.get() {
                    return view! {
                        <div class="list">
                            <div class="cell"><div class="cell-body"><div class="cell-title">"Couldn't load conversations"</div><div class="cell-sub">{err}</div></div></div>
                        </div>
                    }.into_any();
                }
                let items = rows.get();
                if items.is_empty() {
                    return view! { <div class="list-header">"No conversations yet"</div> }.into_any();
                }
                view! {
                    <div class="list">
                        {items.into_iter().map(|row| {
                            let on_select = on_select.clone();
                            let title = row.topic.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "Untitled".to_string());
                            let sub = format!("{} messages", row.message_count);
                            let row_for_click = row.clone();
                            view! {
                                <div class="cell" on:click=move |_| on_select(row_for_click.clone())>
                                    <div class="cell-body">
                                        <div class="cell-title">{title}</div>
                                        <div class="cell-sub">{sub}</div>
                                    </div>
                                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
        </PhoneShell>
    }
}
```

- [ ] **Step 2: Verify it type-checks**

Run: LSP diagnostics on `list.rs` (or defer to the Task 6 milestone `cargo check`). No errors expected. (`on_select` is `Clone` because it only captures `Copy` handles + a cloneable `navigate`.)

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/platform/phone/chat/list.rs
git commit -m "panel(phone): PhoneChatList session-list landing (sessions.list + select + new-chat)"
```

---

### Task 4: `PhoneComposer` component (text + send + stop)

**Files:**
- Modify: `interfaces/webchat/src/platform/phone/chat/composer.rs`

**Interfaces:**
- Consumes: `ChatState`, `DashboardState`, `ChatApi`, `ChatSendError`, `ChatPhase`.
- Produces: `pub fn PhoneComposer() -> impl IntoView`.

- [ ] **Step 1: Write the composer**

Replace the empty `composer.rs` with:
```rust
//! Minimal phone composer: an auto-growing textarea + a send/stop button.
//! Faithful subset of the wide `InputArea::send_message` flow (no attachments,
//! slash-commands, @-mentions, team routing, or model override — server remains
//! the prompt-injection authority). Streaming deltas arrive via the run.* event
//! subscription owned by `PhoneChat`.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::chat::ChatApi;
use crate::context::DashboardState;
use crate::views::chat::state::{ChatPhase, ChatSendError};
use crate::views::chat::ChatState;

#[component]
#[must_use]
pub fn PhoneComposer() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();

    let input_text = RwSignal::new(String::new());
    let is_sending = RwSignal::new(false);

    // True while a run is in flight → button becomes Stop.
    let running = move || {
        matches!(chat.phase.get(), ChatPhase::Thinking | ChatPhase::Streaming)
            || chat.active_run_id.get().is_some()
    };

    let send = move || {
        if is_sending.get_untracked() {
            return;
        }
        let text = input_text.get_untracked().trim().to_string();
        if text.is_empty() {
            return;
        }
        is_sending.set(true);
        input_text.set(String::new());
        chat.push_user_message(&text);

        let session_key = chat.session_key.get_untracked();
        let agent_id = chat.agent_id.get_untracked();
        let project_root = chat.active_project_root.get_untracked();
        let dash = dashboard;
        spawn_local(async move {
            let res = ChatApi::send(
                &dash,
                &text,
                session_key.as_deref(),
                Vec::new(),
                agent_id.as_deref(),
                project_root.as_deref(),
                None,
            )
            .await;
            match res {
                Ok(resp) => chat.session_key.set(Some(resp.session_key)),
                Err(e) => chat.set_send_error(ChatSendError::classify(e)),
            }
            is_sending.set(false);
        });
    };

    let stop = move || {
        let Some(run_id) = chat.active_run_id.get_untracked() else { return };
        let dash = dashboard;
        spawn_local(async move {
            let _ = ChatApi::abort(&dash, &run_id).await;
        });
    };

    // Enter sends; Shift+Enter inserts a newline.
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            send();
        }
    };

    view! {
        <div style="flex:none; display:flex; align-items:flex-end; gap:8px; padding:8px 12px calc(8px + env(safe-area-inset-bottom)); border-top:1px solid var(--color-border-subtle); background:var(--color-surface);">
            <textarea
                prop:value=move || input_text.get()
                on:input=move |ev| input_text.set(event_target_value(&ev))
                on:keydown=on_keydown
                placeholder="Message…"
                rows="1"
                style="flex:1; resize:none; max-height:140px; min-height:38px; padding:9px 12px; border:1px solid var(--color-border); border-radius:var(--radius-xl); background:var(--color-surface-raised); color:var(--color-text-primary); font:inherit; font-size:15px; outline:none;"
            ></textarea>
            {move || if running() {
                view! {
                    <button
                        on:click=move |_| stop()
                        style="flex:none; width:38px; height:38px; border:0; border-radius:9999px; background:var(--color-danger); color:white; cursor:pointer; display:flex; align-items:center; justify-content:center;"
                        aria-label="Stop"
                    ><svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"></rect></svg></button>
                }.into_any()
            } else {
                view! {
                    <button
                        on:click=move |_| send()
                        style="flex:none; width:38px; height:38px; border:0; border-radius:9999px; background:var(--color-primary); color:white; cursor:pointer; display:flex; align-items:center; justify-content:center;"
                        aria-label="Send"
                    ><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="19" x2="12" y2="5"></line><polyline points="5 12 12 5 19 12"></polyline></svg></button>
                }.into_any()
            }}
        </div>
    }
}
```

- [ ] **Step 2: Verify it type-checks**

Run: LSP diagnostics on `composer.rs` (or defer to Task 6 milestone). Confirm `event_target_value` resolves (it's in `leptos::prelude`).

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/platform/phone/chat/composer.rs
git commit -m "panel(phone): PhoneComposer (text + send + stop, minimal ChatApi flow)"
```

---

### Task 5: `PhoneChatThread` component (conversation)

**Files:**
- Modify: `interfaces/webchat/src/platform/phone/chat/thread.rs`

**Interfaces:**
- Consumes: `MessageList` (Task 1), `PhoneComposer` (Task 4), `PhoneTabBar`.
- Produces: `pub fn PhoneChatThread() -> impl IntoView`.

- [ ] **Step 1: Write the thread chrome**

Replace the empty `thread.rs` with (manual full-screen column: top bar with back → `/`, scrolling `MessageList`, pinned `PhoneComposer`, shared `PhoneTabBar`; the streaming subscription is owned by `PhoneChat`, Task 6):
```rust
//! Phone Chat conversation view. Manual iOS chrome (a dynamic title isn't
//! expressible through PhoneShell's `&'static str` title, and the body must be
//! flush so MessageList controls its own scroll) reusing PhoneTabBar. Renders
//! the shared `MessageList` + `PhoneComposer` against the app-root ChatState.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::platform::phone::chat::composer::PhoneComposer;
use crate::platform::phone::shell::PhoneTabBar;
use crate::views::chat::messages::MessageList;

#[component]
#[must_use]
pub fn PhoneChatThread() -> impl IntoView {
    let navigate = use_navigate();
    let back = move |_| navigate("/", NavigateOptions::default());

    view! {
        <div
            class="fixed inset-x-0 top-0 h-dvh z-[70] flex flex-col"
            style="background:var(--color-surface);"
        >
            <div
                class="glass"
                style="position:relative; flex:none; display:flex; align-items:center; gap:8px; min-height:50px; padding:calc(4px + env(safe-area-inset-top)) 14px 8px; z-index:4; background-color:color-mix(in oklch, var(--color-surface-overlay) 78%, transparent);"
            >
                <button
                    style="position:absolute; left:10px; top:50%; transform:translateY(-10%); display:flex; align-items:center; gap:2px; background:none; border:0; cursor:pointer; color:var(--color-primary); font:inherit; font-size:16px; padding:4px 6px 4px 0;"
                    on:click=back
                >
                    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 6 9 12 15 18"></polyline></svg>
                    "Chat"
                </button>
                <span style="width:100%; text-align:center; font-size:17px; font-weight:600; letter-spacing:-0.01em; color:var(--color-text-primary);">"Conversation"</span>
            </div>

            <div style="flex:1; min-height:0; display:flex; flex-direction:column;">
                <MessageList/>
            </div>

            <PhoneComposer/>
            <PhoneTabBar/>
        </div>
    }
}
```

- [ ] **Step 2: Verify it type-checks**

Run: LSP diagnostics on `thread.rs` (or defer to Task 6 milestone). Confirm `MessageList` is reachable (Task 1 made it `pub(crate)`).

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/platform/phone/chat/thread.rs
git commit -m "panel(phone): PhoneChatThread conversation chrome (MessageList + composer + tabbar)"
```

---

### Task 6: `PhoneChat` router + streaming subscription

**Files:**
- Modify: `interfaces/webchat/src/platform/phone/chat/mod.rs`

**Interfaces:**
- Consumes: `PhoneChatList`, `PhoneChatThread`, `subscribe_run_events`, `DashboardState`, `ChatState`, `WorkspaceState`.
- Produces: `pub fn PhoneChat() -> impl IntoView` (the form-factor swap target wired in Task 7).

- [ ] **Step 1: Add the router component to `mod.rs`**

Append to `interfaces/webchat/src/platform/phone/chat/mod.rs` (below the `pub mod` lines):
```rust
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::context::DashboardState;
use crate::state::layout::WorkspaceState;
use crate::views::chat::events::subscribe_run_events;
use crate::views::chat::ChatState;

use self::list::PhoneChatList;
use self::thread::PhoneChatThread;

/// Phone Chat router. Owns the `run.*` streaming subscription (mirrors the wide
/// `ChatView`); exactly one of {ChatView, PhoneChat} mounts per form factor, so
/// there is no double-subscribe. Renders the list at `/` and the thread at
/// `/chat`.
#[component]
#[must_use]
pub fn PhoneChat() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let workspace = expect_context::<WorkspaceState>();

    // Drive ChatState from run.* events (single-agent stream only — no team).
    let sub_id = subscribe_run_events(&dashboard, chat, workspace);

    // Ask the Gateway to forward stream.* once connected (poll up to ~5s).
    {
        let dash = dashboard;
        spawn_local(async move {
            for _ in 0..50 {
                if dash.is_connected.get_untracked() {
                    break;
                }
                gloo_timers::future::TimeoutFuture::new(100).await;
            }
            if let Err(e) = dash.subscribe_topic("stream.*").await {
                web_sys::console::error_1(&format!("phone chat stream sub failed: {e}").into());
            }
        });
    }

    on_cleanup(move || {
        dashboard.unsubscribe_events(sub_id);
        let dash = dashboard;
        spawn_local(async move {
            let _ = dash.unsubscribe_topic("stream.*").await;
        });
    });

    let location = use_location();
    move || {
        if location.pathname.get() == "/chat" {
            view! { <PhoneChatThread/> }.into_any()
        } else {
            view! { <PhoneChatList/> }.into_any()
        }
    }
}
```

- [ ] **Step 2: Milestone compile check (all phone components)**

Run: `cargo check -p aleph-panel`
Expected: compiles clean. Fix any path/visibility errors surfaced here before wiring. (`gloo_timers` and `web_sys` are already crate deps used by `view.rs`.)

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/platform/phone/chat/mod.rs
git commit -m "panel(phone): PhoneChat router owning the run.* streaming subscription"
```

---

### Task 7: Wire into `MainContent` (form-factor swap)

**Files:**
- Modify: `interfaces/webchat/src/app.rs:36-45` (imports), `:381-389` (Chat branch)

**Interfaces:**
- Consumes: `PhoneChat` (Task 6), `FormFactorState`/`FormFactor` (already imported at `app.rs:45`).

- [ ] **Step 1: Import `PhoneChat`**

In `app.rs`, next to the other phone imports (around line 38), add:
```rust
use crate::platform::phone::chat::PhoneChat;
```

- [ ] **Step 2: Read form factor at the top of `MainContent` and swap the Chat branch**

In `MainContent` (app.rs:382), after `let mode = Memo::new(...)`, add:
```rust
    let form_factor = expect_context::<FormFactorState>();
```

Then replace the Chat container (currently `app.rs:387-389`):
```rust
        <div style:display=move || if mode.get() == PanelMode::Chat { "contents" } else { "none" }>
            <ChatView />
        </div>
```
with:
```rust
        <div style:display=move || if mode.get() == PanelMode::Chat { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneChat /> }.into_any()
            } else {
                view! { <ChatView /> }.into_any()
            }}
        </div>
```

(`/chat` already maps to `PanelMode::Chat` via the `from_path` catch-all, so the thread route stays in this branch on both form factors — no `from_path` change needed.)

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p aleph-panel`
Expected: compiles clean.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/app.rs
git commit -m "panel: swap ChatView <-> PhoneChat by form factor in MainContent"
```

---

### Task 8: `PhoneTabBar` dynamic active state

**Files:**
- Modify: `interfaces/webchat/src/platform/phone/shell.rs:14-40`

**Interfaces:**
- Consumes: `PanelMode` (`crate::components::mode_sidebar::PanelMode`).

- [ ] **Step 1: Derive the active tab from the route**

In `shell.rs`, add imports at the top:
```rust
use crate::components::mode_sidebar::PanelMode;
use leptos_router::hooks::use_location;
```

In `PhoneTabBar`, after `let go = ...`, add:
```rust
    let location = use_location();
    let mode = move || PanelMode::from_path(&location.pathname.get());
```

- [ ] **Step 2: Replace each tab's static class with a reactive `class:tabitem-active`**

For each of the four `<button class="tabitem" ...>` (and the Settings one currently `class="tabitem tabitem-active"`), set the base class to `"tabitem"` and add a reactive toggle. Chat:
```rust
            <button class="tabitem" class:tabitem-active=move || mode() == PanelMode::Chat on:click=go("/")>
```
Memory:
```rust
            <button class="tabitem" class:tabitem-active=move || mode() == PanelMode::Memory on:click=go("/memory")>
```
Agents:
```rust
            <button class="tabitem" class:tabitem-active=move || mode() == PanelMode::Agents on:click=go("/agents")>
```
Settings (drop the hardcoded `tabitem-active`):
```rust
            <button class="tabitem" class:tabitem-active=move || mode() == PanelMode::Settings on:click=go("/settings")>
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p aleph-panel`
Expected: compiles clean. (The Settings phone screens previously relied on the hardcoded active class; now it's route-derived — `/settings*` → `PanelMode::Settings`, so Settings stays highlighted there, and Chat highlights on `/` and `/chat`.)

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/phone/shell.rs
git commit -m "panel(phone): PhoneTabBar active state follows the route (was hardcoded Settings)"
```

---

### Task 9: Build + visual verification in the simulator

**Files:** none (build + manual verification; commit any fixes found).

- [ ] **Step 1: Rebuild the WASM the dev server serves**

Run: `just wasm`
Expected: Tailwind + `wasm-bindgen` succeed; `interfaces/webchat/dist/aleph_panel_bg.wasm` updated. (The running dev daemon serves `dist/` from disk — no server rebuild needed. If verifying against the standalone `target/debug/aleph-server`, it embeds `dist/` at compile time, so rebuild it instead.)

- [ ] **Step 2: Launch the phone shell against the local core and screenshot**

Ensure the local core is up (`:18790`) and the iOS app points at it, then:
```bash
~/AlephPaneliOS/launch-local.sh /          # land on the Chat tab (session list)
```
Run: `xcrun simctl io <booted-udid> screenshot /tmp/phone-chat-list.png`
Expected: an iOS-native session list under a "Chat" title with a "+ New chat" row and the bottom TabBar (Chat tab highlighted).

- [ ] **Step 3: Exercise the flow**

Verify by tapping in the simulator (screenshot each):
1. Tap a session → drills to `/chat`, shows the conversation (reused `MessageList`) + composer + back button.
2. Type a message + Send → user bubble appears, assistant streams in (proves the `run.*` subscription works on phone), button shows Stop while running.
3. Tap Stop mid-stream → run aborts.
4. Back → returns to the list (Chat tab still highlighted).
5. "+ New chat" → empty thread; sending creates a fresh session.
6. Switch to Settings tab and back → Chat still works (subscription persists; `MainContent` keeps `PhoneChat` mounted).

- [ ] **Step 4: Confirm desktop is unaffected**

Resize the same panel ≥1024px (or load in a desktop window): the wide `ChatView` renders unchanged.

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "panel(phone): polish phone chat after simulator verification"
```

(If no fixes were needed, skip this commit.)

---

## Self-Review

**Spec coverage:**
- §2 scope (list → thread → send/stream/stop/new-chat) → Tasks 3,4,5,6,9. ✅
- §3 reuse table (ChatState/ChatApi/MessageList/sessions.list/subscribe_run_events reused; new list + composer) → Tasks 1,3,4,6. ✅
- §3 surgical edits (MessageList visibility, PhoneTabBar active, PhoneShell footer) → Task 1, Task 8; **PhoneShell footer dropped** — the thread builds its own chrome (Task 5) because PhoneShell's `&'static str` title can't be dynamic and its body padding fights a message list. Noted as a deliberate revision. ✅
- §4 files → Tasks 2–6. ✅
- §5 navigation (`/` list ↔ `/chat` thread, back, Chat tab highlight, tap-select, new-chat) → Tasks 3,5,6,7,8. ✅
- §6 streaming subscription on the phone side → Task 6. ✅
- §9 team detection → resolved (server already excludes team `task` sessions from `sessions.list`); no client filter. ✅
- §11 testing (pure helpers) → Task 2. ✅

**Placeholder scan:** no "TBD/TODO"; every code step shows complete code; the empty `composer.rs`/`thread.rs` files in Task 2 are explicitly created empty and filled in Tasks 4/5. ✅

**Type consistency:** `SessionRow`/`sort_sessions_desc` (Task 2) used in Task 3; `PhoneChatList`/`PhoneChatThread`/`PhoneComposer`/`PhoneChat` names consistent across Tasks 3–7; `hydrate_session_history`/`MessageList` visibility (Task 1) consumed in Tasks 3/5; `ChatApi::send` arg order matches the Global Constraints signature; `ChatSendError::classify`, `clear_session`, `push_user_message`, `set_send_error` match the extracted signatures. ✅

**Revision vs spec:** the streaming subscription was moved from the thread (spec §6) to the `PhoneChat` router (Task 6) so it mounts once and mirrors `ChatView` exactly — strictly better; recorded here.
