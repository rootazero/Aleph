# Session Rename & Delete Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add session rename and delete functionality to the panel webchat sidebar, plus a `session_set_topic` builtin tool for natural language renaming.

**Architecture:** Three layers of change: (1) new `handle_set_topic_db` RPC handler in the gateway, (2) new `SessionSetTopicTool` builtin tool following the `session_new` pattern, (3) Leptos UI modifications in `chat_sidebar.rs` for hover menu, inline edit, and inline delete confirmation.

**Tech Stack:** Rust, Leptos/WASM, SQLite (SessionManager), AlephTool trait, JSON-RPC

**Spec:** `docs/superpowers/specs/2026-03-19-session-rename-delete-design.md`

---

### Task 1: Add `sessions.set_topic` RPC handler

**Files:**
- Modify: `src/gateway/handlers/session/db_handlers.rs` (append new handler)
- Modify: `src/gateway/handlers/session/mod.rs` (add re-export)
- Modify: `src/bin/aleph/commands/start/builder/handlers.rs:124-144` (register new method)

- [ ] **Step 1: Add `handle_set_topic_db` to db_handlers.rs**

Append before the `estimate_db_tokens` helper at the end of `db_handlers.rs` (line ~476). Follow the exact same pattern as `handle_delete_db` but call `manager.set_topic()` instead:

```rust
/// Handle sessions.set_topic RPC request with database backend
///
/// Params:
///   - session_key (required): session key string
///   - topic (required): new topic string (max 100 chars)
pub async fn handle_set_topic_db(
    request: JsonRpcRequest,
    manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let session_key_str = match params.get("session_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key");
        }
    };

    let topic = match params.get("topic").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing topic");
        }
    };

    // Validate topic length (P7: boundary validation)
    let topic = if topic.len() > 100 {
        &topic[..topic.char_indices().nth(100).map(|(i, _)| i).unwrap_or(topic.len())]
    } else {
        topic
    };

    let session_key = match SessionKey::from_key_string(session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    match manager.set_topic(&session_key, topic).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": session_key_str,
                "updated": true,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to set topic: {}", e),
        ),
    }
}
```

- [ ] **Step 2: Re-export from mod.rs**

In `src/gateway/handlers/session/mod.rs`, add `handle_set_topic_db` to the `pub use db_handlers::{...}` block:

```rust
pub use db_handlers::{
    handle_list_db, handle_history_db, handle_reset_db, handle_delete_db,
    handle_usage_db, handle_create_db, handle_new_session_db, handle_compact_db,
    handle_set_topic_db,
};
```

- [ ] **Step 3: Register in handler registration**

In `src/bin/aleph/commands/start/builder/handlers.rs`, inside `register_session_handlers()`, add after the `sessions.new` line (line 131):

```rust
register_handler!(server, "sessions.set_topic", session_handlers::handle_set_topic_db, session_manager);
```

And add to the print block:

```rust
println!("  - sessions.set_topic: Set session topic/title");
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore && cargo check -p aleph`
Expected: compiles without errors

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/session/db_handlers.rs \
       src/gateway/handlers/session/mod.rs \
       src/bin/aleph/commands/start/builder/handlers.rs
git commit -m "gateway: add sessions.set_topic RPC handler"
```

---

### Task 2: Add `session_set_topic` builtin tool

**Files:**
- Create: `src/builtin_tools/sessions/set_topic_tool.rs`
- Modify: `src/builtin_tools/sessions/mod.rs` (add module + re-exports)
- Modify: `src/executor/builtin_registry/definitions.rs` (~line 142, add definition)
- Modify: `src/executor/builtin_registry/registry.rs` (~line 72, add field + ~line 315, add execute_tool arm)
- Modify: `src/executor/builtin_registry/builder.rs` (~line 317, construct tool + ~line 516, register schema)
- Modify: `src/executor/builtin_registry/groups.rs` (~line 76, add to group)

- [ ] **Step 1: Create `set_topic_tool.rs`**

Create `src/builtin_tools/sessions/set_topic_tool.rs`. Follow the exact pattern of `new_tool.rs`:

```rust
//! Session set-topic tool — rename the current session's topic.
//!
//! Allows the LLM to rename a session topic via natural language,
//! complementing the panel UI's inline edit (R9: Everything is a Tool).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::router::SessionKey as LegacySessionKey;
use crate::gateway::SessionManager;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the session_set_topic tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionSetTopicArgs {
    /// The new topic/title for the session.
    pub topic: String,

    /// Injected by registry — serialized session key (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __session_key: String,
}

/// Output from session_set_topic tool
#[derive(Debug, Clone, Serialize)]
pub struct SessionSetTopicOutput {
    pub session_key: String,
    pub topic: String,
    pub message: String,
}

/// Tool that renames the current session's topic.
#[derive(Clone)]
pub struct SessionSetTopicTool {
    session_manager: Arc<SessionManager>,
}

impl SessionSetTopicTool {
    pub fn new(session_manager: Arc<SessionManager>) -> Self {
        Self { session_manager }
    }
}

#[async_trait]
impl AlephTool for SessionSetTopicTool {
    const NAME: &'static str = "session_set_topic";
    const DESCRIPTION: &'static str =
        "Rename the current session's topic/title. Use when the user \
         asks to change, rename, or set the conversation title or topic.";

    type Args = SessionSetTopicArgs;
    type Output = SessionSetTopicOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "session_set_topic(topic='项目架构讨论')".to_string(),
            "session_set_topic(topic='Debug WASM compilation issues')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let session_key_str = &args.__session_key;

        if session_key_str.is_empty() {
            return Err(crate::error::AlephError::tool(
                "session_set_topic: no session context available (session key not injected)",
            ));
        }

        let topic = args.topic.trim();
        if topic.is_empty() {
            return Err(crate::error::AlephError::tool(
                "session_set_topic: topic cannot be empty",
            ));
        }

        // Truncate to 100 chars (P7: boundary validation)
        let topic = if topic.len() > 100 {
            &topic[..topic.char_indices().nth(100).map(|(i, _)| i).unwrap_or(topic.len())]
        } else {
            topic
        };

        let legacy_key = LegacySessionKey::from_key_string(session_key_str).ok_or_else(|| {
            crate::error::AlephError::tool(format!(
                "session_set_topic: failed to parse session key '{}'",
                session_key_str
            ))
        })?;

        self.session_manager
            .set_topic(&legacy_key, topic)
            .await
            .map_err(|e| {
                crate::error::AlephError::tool(format!(
                    "session_set_topic: failed to set topic: {}",
                    e
                ))
            })?;

        info!(
            session = %session_key_str,
            topic = %topic,
            "Session topic updated via tool"
        );

        Ok(SessionSetTopicOutput {
            session_key: session_key_str.clone(),
            topic: topic.to_string(),
            message: format!("会话主题已更新为: {}", topic),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::SessionManagerConfig;
    use crate::tools::AlephTool;
    use tempfile::tempdir;

    fn test_session_manager() -> Arc<SessionManager> {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.into_path().join("test.db"),
            ..Default::default()
        };
        Arc::new(SessionManager::new(config).unwrap())
    }

    #[test]
    fn test_tool_definition() {
        let sm = test_session_manager();
        let tool = SessionSetTopicTool::new(sm);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "session_set_topic");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_empty_session_key_errors() {
        let sm = test_session_manager();
        let tool = SessionSetTopicTool::new(sm);

        let result = tool
            .call(SessionSetTopicArgs {
                topic: "test".into(),
                __session_key: String::new(),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_empty_topic_errors() {
        let sm = test_session_manager();
        let tool = SessionSetTopicTool::new(sm);

        let result = tool
            .call(SessionSetTopicArgs {
                topic: "   ".into(),
                __session_key: "agent:main:default".into(),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_set_topic_basic() {
        let sm = test_session_manager();

        // Create session first
        let key = LegacySessionKey::Main {
            agent_id: "main".into(),
            main_key: "default".into(),
        };
        sm.get_or_create(&key).await.unwrap();

        let tool = SessionSetTopicTool::new(sm);
        let result = tool
            .call(SessionSetTopicArgs {
                topic: "测试话题".into(),
                __session_key: "agent:main:default".into(),
            })
            .await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.topic, "测试话题");
        assert!(output.message.contains("测试话题"));
    }
}
```

- [ ] **Step 2: Export from sessions/mod.rs**

Add to `src/builtin_tools/sessions/mod.rs`:

```rust
pub mod set_topic_tool;
```

And add re-export:

```rust
pub use set_topic_tool::{SessionSetTopicArgs, SessionSetTopicOutput, SessionSetTopicTool};
```

- [ ] **Step 3: Add to definitions.rs**

In `src/executor/builtin_registry/definitions.rs`, add after the `session_new` entry (after line 142):

```rust
BuiltinToolDefinition {
    name: "session_set_topic",
    description: "Rename the current session's topic/title",
    requires_config: true, // Requires SessionManager (via gateway_context)
},
```

And in the `create_tool_boxed()` function's match statement (~line 310), add alongside `"session_new" => None`:

```rust
"session_set_topic" => None,
```

- [ ] **Step 4: Add field and execute arm to registry.rs**

In `src/executor/builtin_registry/registry.rs`:

1. Add field after `session_new_tool` (after line 72):
```rust
/// Session set-topic tool (optional - requires SessionManager)
pub(crate) session_set_topic_tool: Option<crate::builtin_tools::sessions::SessionSetTopicTool>,
```

2. Add execute arm after the `"session_new"` arm (after line 333). Copy the exact same `__session_key` injection pattern:
```rust
// Session set-topic tool — inject session key from session context
"session_set_topic" => {
    let arguments = {
        let mut args = arguments;
        if let Some(ref h) = self.session_context_handle {
            if let Ok(ctx) = h.try_read() {
                if let Some(obj) = args.as_object_mut() {
                    obj.insert("__session_key".into(), serde_json::Value::String(ctx.session_key_str.clone()));
                }
            }
        }
        args
    };
    Box::pin(async move {
        let tool = self.session_set_topic_tool.as_ref().ok_or_else(|| {
            AlephError::tool("session_set_topic not available: no SessionManager configured")
        })?;
        tool.call_json(arguments).await
    })
}
```

- [ ] **Step 5: Construct and register in builder.rs**

In `src/executor/builtin_registry/builder.rs`:

1. Add construction after `session_new_tool` (after line 319):
```rust
session_set_topic_tool: config.gateway_context.as_ref().map(|ctx| {
    crate::builtin_tools::sessions::SessionSetTopicTool::new(Arc::clone(ctx.session_manager()))
}),
```

2. Add schema registration after the `session_new` registration block (after line 521):
```rust
// Session set-topic tool (requires SessionManager from gateway_context)
if let Some(ref ctx) = config.gateway_context {
    use crate::builtin_tools::sessions::SessionSetTopicTool;
    let tmp_tool = SessionSetTopicTool::new(Arc::clone(ctx.session_manager()));
    let def = AlephTool::definition(&tmp_tool);
    reg(tools, "session_set_topic", SessionSetTopicTool::DESCRIPTION, def.parameters.clone());
    info!("Registered session_set_topic tool in BuiltinToolRegistry");
}
```

- [ ] **Step 6: Add to tool group in groups.rs**

In `src/executor/builtin_registry/groups.rs`, add `"session_set_topic"` after `"session_new"` (after line 76):

```rust
"session_set_topic",
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore --lib set_topic_tool`
Expected: all 4 tests pass

- [ ] **Step 8: Verify full compilation**

Run: `cargo check -p alephcore && cargo check -p aleph`
Expected: compiles without errors

- [ ] **Step 9: Commit**

```bash
git add src/builtin_tools/sessions/set_topic_tool.rs \
       src/builtin_tools/sessions/mod.rs \
       src/executor/builtin_registry/definitions.rs \
       src/executor/builtin_registry/registry.rs \
       src/executor/builtin_registry/builder.rs \
       src/executor/builtin_registry/groups.rs
git commit -m "tools: add session_set_topic builtin tool (R9)"
```

---

### Task 3: Panel UI — session rename and delete

**Files:**
- Modify: `apps/panel/src/components/chat_sidebar.rs`

**Reference:** Current `chat_sidebar.rs` is ~357 lines. After this task it will grow to ~500+ lines. This is acceptable for a single component file.

- [ ] **Step 1: Add state signals**

In `ChatSidebar()` component, after the existing signal declarations (after line 51), add:

```rust
// Session management UI state
let editing_key = RwSignal::new(Option::<String>::None);
let deleting_key = RwSignal::new(Option::<String>::None);
let edit_text = RwSignal::new(String::new());
let menu_open_key = RwSignal::new(Option::<String>::None);
let is_saving = RwSignal::new(false); // prevents double-submit during RPC
```

- [ ] **Step 2: Add helper closures for rename and delete**

After `on_new_chat` closure (after line 206), add:

```rust
// Clear all session action states (mutual exclusion)
let clear_action_states = move || {
    editing_key.set(None);
    deleting_key.set(None);
    menu_open_key.set(None);
    edit_text.set(String::new());
};

// Rename a session topic (with double-submit guard)
let do_rename = {
    let dashboard = dashboard;
    let reload_data = reload_data.clone();
    move |key: String, new_topic: String| {
        if is_saving.get_untracked() { return; }
        is_saving.set(true);
        let dash = dashboard;
        let reload = reload_data.clone();
        leptos::task::spawn_local(async move {
            let params = serde_json::json!({
                "session_key": key,
                "topic": new_topic,
            });
            match dash.rpc_call("sessions.set_topic", params).await {
                Ok(_) => reload(dash),
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Failed to set topic: {e}").into(),
                    );
                }
            }
            is_saving.set(false);
            editing_key.set(None);
            edit_text.set(String::new());
        });
    }
};

// Delete a session (with double-submit guard)
let do_delete = {
    let dashboard = dashboard;
    let reload_data = reload_data.clone();
    move |key: String| {
        if is_saving.get_untracked() { return; }
        is_saving.set(true);
        let dash = dashboard;
        let reload = reload_data.clone();
        leptos::task::spawn_local(async move {
            let params = serde_json::json!({ "session_key": key });
            match dash.rpc_call("sessions.delete", params).await {
                Ok(_) => {
                    // If deleting the active session, clear chat
                    if chat.session_key.get_untracked().as_deref() == Some(&key) {
                        chat.clear_session();
                    }
                    reload(dash);
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Failed to delete session: {e}").into(),
                    );
                }
            }
            is_saving.set(false);
            deleting_key.set(None);
        });
    }
};
```

- [ ] **Step 3: Replace session item rendering**

Replace the session item `view!` block (lines 311-333 approximately, the `view! { <button ...>` inside the `.map(|session|` closure) with the three-mode rendering. This is the largest change.

The new rendering for each session item:

```rust
view! {
    <div class="relative group">
        {move || {
            let key_ref = key.clone();
            let is_editing = editing_key.get().as_deref() == Some(&key_ref);
            let is_deleting = deleting_key.get().as_deref() == Some(&key_ref);
            let is_menu_open = menu_open_key.get().as_deref() == Some(&key_ref);

            if is_editing {
                // ── Edit mode ──
                let key_for_save = key.clone();
                let do_rename = do_rename.clone();
                view! {
                    <div class="px-3 py-2.5 rounded-lg bg-surface-sunken border border-primary/30">
                        <input
                            type="text"
                            class="w-full bg-transparent text-xs text-text-primary
                                   outline-none border-none font-medium"
                            prop:value=move || edit_text.get()
                            maxlength=100
                            on:input=move |ev| {
                                edit_text.set(event_target_value(&ev));
                            }
                            on:keydown={
                                let key_for_enter = key_for_save.clone();
                                let do_rename = do_rename.clone();
                                move |ev: web_sys::KeyboardEvent| {
                                    match ev.key().as_str() {
                                        "Enter" => {
                                            let text = edit_text.get_untracked().trim().to_string();
                                            if !text.is_empty() {
                                                do_rename(key_for_enter.clone(), text);
                                            } else {
                                                clear_action_states();
                                            }
                                        }
                                        "Escape" => clear_action_states(),
                                        _ => {}
                                    }
                                }
                            }
                            on:blur=move |_| clear_action_states()
                            node_ref=/* see step 4 for auto-focus */
                        />
                    </div>
                }.into_any()
            } else if is_deleting {
                // ── Delete-confirm mode ──
                let key_for_del = key.clone();
                let do_delete = do_delete.clone();
                view! {
                    <div class="px-3 py-2.5 rounded-lg bg-red-500/10 border border-red-500/30
                                flex items-center justify-between text-xs">
                        <span class="text-red-400 font-medium">"确认删除?"</span>
                        <div class="flex gap-1">
                            <button
                                class="px-2 py-0.5 rounded bg-red-500 text-white text-[10px]
                                       hover:bg-red-600 transition-colors"
                                on:click={
                                    let key = key_for_del.clone();
                                    let do_delete = do_delete.clone();
                                    move |ev: web_sys::MouseEvent| {
                                        ev.stop_propagation();
                                        do_delete(key.clone());
                                    }
                                }
                            >
                                "确认"
                            </button>
                            <button
                                class="px-2 py-0.5 rounded bg-surface-sunken text-text-secondary
                                       text-[10px] hover:bg-surface-raised transition-colors"
                                on:click=move |ev: web_sys::MouseEvent| {
                                    ev.stop_propagation();
                                    clear_action_states();
                                }
                            >
                                "取消"
                            </button>
                        </div>
                    </div>
                }.into_any()
            } else {
                // ── Normal mode ──
                let key_for_click = key.clone();
                let key_for_menu = key.clone();
                let key_for_rename = key.clone();
                let key_for_delete = key.clone();
                let on_select = on_select.clone();
                let session_agent_id = session_agent_id.clone();
                view! {
                    <button
                        class=move || format!(
                            "w-full text-left px-3 py-2.5 rounded-lg text-sm transition-colors \
                             flex items-center justify-between {}",
                            if is_active() {
                                "bg-primary/10 text-primary font-medium"
                            } else {
                                "text-text-secondary hover:bg-surface-sunken hover:text-text-primary"
                            }
                        )
                        on:click={
                            let key = key_for_click.clone();
                            let agent = session_agent_id.clone();
                            let on_select = on_select.clone();
                            move |_| {
                                clear_action_states();
                                on_select(key.clone(), agent.clone());
                            }
                        }
                    >
                        <div class="flex-1 min-w-0">
                            <div class="truncate font-medium text-xs">
                                {label.clone()}
                            </div>
                            <div class="truncate text-[10px] text-text-tertiary mt-0.5">
                                {subtitle.clone()}
                            </div>
                        </div>
                        // ⋯ button (visible on hover)
                        <div class="relative flex-shrink-0 ml-1">
                            <button
                                class="opacity-0 group-hover:opacity-100 transition-opacity
                                       px-1 py-0.5 rounded text-text-tertiary
                                       hover:text-text-primary hover:bg-surface-raised text-xs"
                                on:click={
                                    let key = key_for_menu.clone();
                                    move |ev: web_sys::MouseEvent| {
                                        ev.stop_propagation();
                                        let current = menu_open_key.get_untracked();
                                        if current.as_deref() == Some(&key) {
                                            menu_open_key.set(None);
                                        } else {
                                            editing_key.set(None);
                                            deleting_key.set(None);
                                            menu_open_key.set(Some(key.clone()));
                                        }
                                    }
                                }
                            >
                                "⋯"
                            </button>
                            // Dropdown menu
                            {move || {
                                if is_menu_open {
                                    let key_r = key_for_rename.clone();
                                    let key_d = key_for_delete.clone();
                                    let current_topic = label.clone();
                                    view! {
                                        <div class="absolute right-0 top-6 z-50 w-32
                                                    bg-surface-raised border border-border
                                                    rounded-lg shadow-lg py-1 text-xs">
                                            <button
                                                class="w-full text-left px-3 py-1.5
                                                       text-text-secondary hover:bg-surface-sunken
                                                       hover:text-text-primary transition-colors"
                                                on:click={
                                                    let key = key_r.clone();
                                                    let topic = current_topic.clone();
                                                    move |ev: web_sys::MouseEvent| {
                                                        ev.stop_propagation();
                                                        menu_open_key.set(None);
                                                        edit_text.set(topic.clone());
                                                        editing_key.set(Some(key.clone()));
                                                    }
                                                }
                                            >
                                                "重命名"
                                            </button>
                                            <button
                                                class="w-full text-left px-3 py-1.5
                                                       text-red-400 hover:bg-red-500/10
                                                       transition-colors"
                                                on:click={
                                                    let key = key_d.clone();
                                                    move |ev: web_sys::MouseEvent| {
                                                        ev.stop_propagation();
                                                        menu_open_key.set(None);
                                                        deleting_key.set(Some(key.clone()));
                                                    }
                                                }
                                            >
                                                "删除"
                                            </button>
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <span /> }.into_any()
                                }
                            }}
                        </div>
                    </button>
                }.into_any()
            }
        }}
    </div>
}
```

**Important integration notes:**
- The `label`, `subtitle`, `is_active`, `key`, `session_agent_id`, `on_select` variables come from the existing closure context — keep the existing variable bindings above the view.
- The outer `<div class="relative group">` replaces the existing `<button>` as the top-level element per session item.
- `do_rename` and `do_delete` closures need to be cloned into the map iterator.

- [ ] **Step 4: Add auto-focus for edit input**

At the top of the component, add a `NodeRef` for the edit input and an effect to auto-focus and select-all when entering edit mode:

```rust
use leptos::prelude::NodeRef;
use web_sys::HtmlInputElement;

// Inside the component:
let edit_input_ref = NodeRef::<leptos::html::Input>::new();

// Effect: auto-focus and select-all when editing starts
Effect::new(move || {
    if editing_key.get().is_some() {
        // Delay to let DOM update
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(10).await;
            if let Some(el) = edit_input_ref.get() {
                let _ = el.focus();
                let _ = el.select();
            }
        });
    }
});
```

Then apply `node_ref=edit_input_ref` to the `<input>` in edit mode.

**Note:** `gloo-timers` is already a dependency in `apps/panel/Cargo.toml`. The key point is to delay focus by one frame so the input is rendered first.

- [ ] **Step 5: Add delete-confirm auto-dismiss (5 seconds)**

When entering delete-confirm mode, start a 5-second timer that auto-clears `deleting_key`:

```rust
// Effect: auto-dismiss delete confirmation after 5 seconds
Effect::new(move || {
    if let Some(key) = deleting_key.get() {
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(5000).await;
            // Only dismiss if still confirming the same key
            if deleting_key.get_untracked().as_deref() == Some(&key) {
                deleting_key.set(None);
            }
        });
    }
});
```

- [ ] **Step 6: Add click-outside handler to close menu**

Use a transparent overlay approach (no leaked event listeners). When the menu is open, render a fixed overlay behind it that catches clicks:

Add this inside the session list container (before the `{filtered...}` block), as a sibling:

```rust
// Transparent overlay to close menu on outside click
{move || {
    if menu_open_key.get().is_some() {
        view! {
            <div
                class="fixed inset-0 z-40"
                on:click=move |_| menu_open_key.set(None)
            />
        }.into_any()
    } else {
        view! { <span /> }.into_any()
    }
}}
```

The dropdown menu already uses `z-50`, so it renders above the `z-40` overlay. Clicks on the overlay close the menu; clicks on menu items (which use `stop_propagation()`) still work.

- [ ] **Step 7: Add Esc handler for delete-confirm mode**

Add `tabindex="0"` and `on:keydown` directly to the delete-confirm `<div>` in Step 3's code. Update the delete-confirm div to:

```rust
<div
    tabindex=0
    class="px-3 py-2.5 rounded-lg bg-red-500/10 border border-red-500/30
                flex items-center justify-between text-xs"
    on:keydown=move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Escape" {
            clear_action_states();
        }
    }
>
```

The delete-confirm auto-focus is handled by Step 5's timer pattern: when `deleting_key` changes, spawn a delayed focus call on the delete-confirm div (similar to edit input auto-focus in Step 4). Alternatively, the implementer can skip auto-focus for delete-confirm and only support Esc if the user explicitly clicks/tabs into the row.

- [ ] **Step 8: Build WASM and verify**

Run: `just build-wasm` or the project's WASM build command.
Expected: compiles without errors, panel loads in browser.

- [ ] **Step 9: Manual test**

1. Open the panel in browser
2. Hover a session → verify ⋯ button appears
3. Click ⋯ → verify dropdown shows "重命名" and "删除"
4. Click "重命名" → verify inline input with current topic
5. Type new name, press Enter → verify topic updates in sidebar
6. Press Esc → verify edit cancelled
7. Click ⋯ → "删除" → verify red confirmation row
8. Click "取消" → verify returns to normal
9. Click ⋯ → "删除" → "确认" → verify session removed
10. Delete active session → verify chat area cleared

- [ ] **Step 10: Commit**

```bash
git add apps/panel/src/components/chat_sidebar.rs
git commit -m "panel: add session rename and delete to sidebar"
```

---

### Task 4: Build and verify everything together

- [ ] **Step 1: Run core tests**

Run: `cargo test -p alephcore --lib`
Expected: all tests pass (pre-existing failures in `tools::markdown_skill::loader::tests` are known)

- [ ] **Step 2: Full build**

Run: `just build` (or `cargo build --release -p aleph`)
Expected: compiles without errors

- [ ] **Step 3: Integration test**

1. Start the server: `target/release/aleph start`
2. Open panel, test rename flow end-to-end
3. Test delete flow end-to-end
4. In chat, type "把这次对话改名为测试话题" → verify LLM calls `session_set_topic` tool
5. Verify sidebar reflects the change

- [ ] **Step 4: Final commit (if any fixups needed)**

```bash
git add -A
git commit -m "session rename/delete: fixups after integration testing"
```
