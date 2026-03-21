//! Main Chat view — message list + input area.

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;
use crate::components::markdown::MarkdownRenderer;
use crate::context::DashboardState;
use crate::api::chat::{ChatApi, ChatAttachment};
use super::state::{ChatState, ChatPhase, ChatMessage};
use super::events::subscribe_run_events;

/// A file attachment pending upload.
#[derive(Clone, Debug)]
struct FileAttachment {
    name: String,
    mime_type: String,
    data_base64: String,
    size: u64,
}

/// Top-level Chat view component.
#[component]
pub fn ChatView() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = ChatState::new();
    provide_context(chat);

    // Subscribe to run.* events directly (no Effect — this is a one-shot mount action)
    let sub_id = subscribe_run_events(&dashboard, chat);

    // Tell the Gateway to start forwarding stream.* events
    // (backend publishes events with method "stream.run_accepted", "stream.response_chunk", etc.)
    // Wait until connected before subscribing, since ChatView may mount before WebSocket is ready.
    let dash_for_sub = dashboard;
    spawn_local(async move {
        // Poll until connected (max ~5s)
        for _ in 0..50 {
            if dash_for_sub.is_connected.get_untracked() { break; }
            gloo_timers::future::TimeoutFuture::new(100).await;
        }
        if let Err(e) = dash_for_sub.subscribe_topic("stream.*").await {
            web_sys::console::error_1(&format!("Failed to subscribe stream events: {e}").into());
        }
    });

    let dash_for_cleanup = dashboard;
    on_cleanup(move || {
        dash_for_cleanup.unsubscribe_events(sub_id);
        // Tell the Gateway to stop forwarding stream.* events
        let dash = dash_for_cleanup;
        spawn_local(async move {
            let _ = dash.unsubscribe_topic("stream.*").await;
        });
    });

    view! {
        <div class="flex flex-col h-full bg-surface">
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
    let chat = expect_context::<ChatState>();
    let scroll_ref = NodeRef::<leptos::html::Div>::new();

    // Auto-scroll to bottom when messages change or during streaming
    Effect::new(move || {
        let _msgs = chat.messages.get();
        let _phase = chat.phase.get();
        if let Some(el) = scroll_ref.get() {
            let el: &web_sys::HtmlElement = &el;
            el.set_scroll_top(el.scroll_height());
        }
    });

    view! {
        <div node_ref=scroll_ref class="flex-1 overflow-y-auto px-4 py-6 space-y-4">
            <For
                each=move || chat.messages.get()
                key=|msg| format!("{}:{}:{}:{}", msg.id, msg.content.len(), msg.is_streaming, msg.tool_calls.len())
                children=move |msg| {
                    view! { <MessageBubble message=msg /> }
                }
            />
            // Thinking indicator
            <Show when=move || chat.phase.get() == ChatPhase::Thinking>
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
fn MessageBubble(message: ChatMessage) -> impl IntoView {
    let is_user = message.role == "user";
    let has_error = message.error.is_some();
    let has_tools = !message.tool_calls.is_empty();

    let bubble_align = if is_user { "flex justify-end" } else { "flex justify-start" };
    let bubble_style = if is_user {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-primary text-white"
    } else if has_error {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-danger-subtle text-danger border border-danger/20"
    } else {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-surface-raised text-text-primary"
    };

    let tool_calls_view = if has_tools {
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
                    let status_icon = match tc.status.as_str() {
                        "running" => "\u{27F3}",
                        "completed" => "\u{2713}",
                        "failed" => "\u{2717}",
                        _ => "\u{00B7}",
                    };
                    let duration_text = tc.duration_ms
                        .map(|d| format!(" ({d}ms)"))
                        .unwrap_or_default();
                    view! {
                        <div class="flex items-center gap-2 text-xs font-mono">
                            <span class=status_color>
                                {status_icon}
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
    };

    let content = message.content.clone();
    let is_streaming = message.is_streaming;
    let error = message.error.clone();

    let streaming_cursor = if is_streaming {
        Some(view! {
            <span class="inline-block w-1.5 h-4 bg-primary/60 animate-pulse ml-0.5 align-text-bottom"></span>
        })
    } else {
        None
    };

    let error_view = error.map(|err| view! {
        <div class="mt-2 text-xs text-danger/80">{err}</div>
    });

    view! {
        <div class=bubble_align>
            <div class=bubble_style>
                {tool_calls_view}

                // Message content — Markdown for assistant, plain text for user
                {if is_user {
                    view! {
                        <div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
                            {content.clone()}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <MarkdownRenderer content=content.clone() />
                    }.into_any()
                }}

                // Streaming cursor
                {streaming_cursor}

                // Error message
                {error_view}
            </div>
        </div>
    }
}

/// Format file size as human-readable string.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// A single command entry from the Gateway (supports tree structure).
#[derive(Clone, Debug)]
struct CommandInfo {
    key: String,
    description: String,
    is_namespace: bool,
    param_hint: Option<String>,
    children: Vec<CommandInfo>,
}

/// Parse a single command from JSON (recursive for children).
fn parse_command_info(item: &serde_json::Value) -> Option<CommandInfo> {
    let name = item.get("name").and_then(|v| v.as_str())
        // Fallback: legacy flat format uses "key"
        .or_else(|| item.get("key").and_then(|v| v.as_str()))?;
    let hint = item.get("hint").and_then(|v| v.as_str())
        .or_else(|| item.get("description").and_then(|v| v.as_str()))
        .unwrap_or("");
    let is_namespace = item.get("is_namespace").and_then(|v| v.as_bool()).unwrap_or(false);
    let param_hint = item.get("param_hint").and_then(|v| v.as_str()).map(String::from);
    let children = item.get("children")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(parse_command_info).collect())
        .unwrap_or_default();
    Some(CommandInfo {
        key: name.to_string(),
        description: hint.to_string(),
        is_namespace,
        param_hint,
        children,
    })
}

/// A display entry in the palette (flattened for rendering).
#[derive(Clone, Debug)]
struct PaletteEntry {
    /// Display label (e.g. "session" or "new")
    label: String,
    /// Full slash command to insert (e.g. "/session new ")
    full_command: String,
    /// Description / hint text
    description: String,
    /// True if this is a namespace that can be drilled into
    is_namespace: bool,
    /// True if this is the "back" pseudo-entry
    is_back: bool,
}

/// Text input area with send button, file attachments, and slash command palette.
#[component]
fn InputArea() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let input_text = RwSignal::new(String::new());
    let is_sending = RwSignal::new(false);
    let attachments: RwSignal<Vec<FileAttachment>> = RwSignal::new(Vec::new());

    // Slash command palette state
    let all_commands: RwSignal<Vec<CommandInfo>> = RwSignal::new(Vec::new());
    let show_palette = RwSignal::new(false);
    let palette_entries: RwSignal<Vec<PaletteEntry>> = RwSignal::new(Vec::new());
    let selected_index = RwSignal::new(0usize);
    let commands_loaded = RwSignal::new(false);
    // Current namespace context for hierarchical browsing (None = root level)
    let current_namespace: RwSignal<Option<String>> = RwSignal::new(None);

    // NodeRef for the hidden file input
    let file_input_ref = NodeRef::<leptos::html::Input>::new();

    let send_message = move || {
        if is_sending.get_untracked() { return; }
        let text = input_text.get_untracked().trim().to_string();
        let files = attachments.get_untracked();
        if text.is_empty() && files.is_empty() { return; }

        is_sending.set(true);
        input_text.set(String::new());
        attachments.set(Vec::new());
        chat.push_user_message(&text);

        // Convert to API attachments
        let api_attachments: Vec<ChatAttachment> = files
            .into_iter()
            .map(|f| ChatAttachment {
                name: f.name,
                mime_type: f.mime_type,
                data_base64: f.data_base64,
                size: f.size,
            })
            .collect();

        let session_key = chat.session_key.get();
        let agent_id = chat.agent_id.get();
        let dash = dashboard;
        spawn_local(async move {
            let sk = session_key.as_deref();
            let aid = agent_id.as_deref();
            match ChatApi::send(&dash, &text, sk, api_attachments, aid).await {
                Ok(resp) => {
                    chat.session_key.set(Some(resp.session_key));
                }
                Err(e) => {
                    chat.error_message.set(Some(e.clone()));
                    chat.phase.set(ChatPhase::Error);
                }
            }
            is_sending.set(false);
        });
    };

    // Build palette entries from commands based on current namespace and query
    let build_palette_entries = move |cmds: &[CommandInfo], namespace: &Option<String>, query: &str| -> Vec<PaletteEntry> {
        let mut entries = Vec::new();
        if let Some(ns) = namespace {
            // Inside a namespace — show "back" + children
            entries.push(PaletteEntry {
                label: ".. back".to_string(),
                full_command: "/".to_string(),
                description: "Return to top level".to_string(),
                is_namespace: false,
                is_back: true,
            });
            if let Some(parent) = cmds.iter().find(|c| &c.key == ns) {
                for child in &parent.children {
                    let full_cmd = format!("/{} {} ", ns, child.key);
                    let desc = if let Some(ref ph) = child.param_hint {
                        format!("{} {}", child.description, ph)
                    } else {
                        child.description.clone()
                    };
                    if query.is_empty() || child.key.to_lowercase().contains(&query.to_lowercase())
                        || desc.to_lowercase().contains(&query.to_lowercase())
                    {
                        entries.push(PaletteEntry {
                            label: child.key.clone(),
                            full_command: full_cmd,
                            description: desc,
                            is_namespace: false,
                            is_back: false,
                        });
                    }
                }
            }
        } else {
            // Root level — show namespaces and independent commands
            let q = query.to_lowercase();
            for cmd in cmds {
                let desc = if let Some(ref ph) = cmd.param_hint {
                    format!("{} {}", cmd.description, ph)
                } else {
                    cmd.description.clone()
                };
                if !query.is_empty()
                    && !cmd.key.to_lowercase().contains(&q)
                    && !desc.to_lowercase().contains(&q)
                {
                    continue;
                }
                let indicator = if cmd.is_namespace { " \u{25B8}" } else { "" };
                entries.push(PaletteEntry {
                    label: cmd.key.clone(),
                    full_command: if cmd.is_namespace {
                        // Clicking a namespace drills into it (handled separately)
                        String::new()
                    } else {
                        format!("/{} ", cmd.key)
                    },
                    description: format!("{}{}", desc, indicator),
                    is_namespace: cmd.is_namespace,
                    is_back: false,
                });
            }
        }
        entries
    };

    // Fetch commands from Gateway (once), then refresh the palette
    let fetch_commands = move || {
        if commands_loaded.get_untracked() { return; }
        let dash = dashboard;
        spawn_local(async move {
            match dash.rpc_call("commands.list", serde_json::json!({})).await {
                Ok(result) => {
                    let mut cmds = Vec::new();
                    if let Some(arr) = result.get("commands").and_then(|v| v.as_array()) {
                        for item in arr {
                            if let Some(cmd) = parse_command_info(item) {
                                cmds.push(cmd);
                            }
                        }
                    }
                    all_commands.set(cmds.clone());
                    commands_loaded.set(true);
                    // Refresh palette with newly loaded commands
                    let text = input_text.get_untracked();
                    if text.starts_with('/') {
                        let query = &text[1..]; // safe: '/' is single-byte ASCII
                        let ns = current_namespace.get_untracked();
                        let entries = build_palette_entries(&cmds, &ns, query);
                        palette_entries.set(entries);
                        selected_index.set(0);
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to fetch commands: {e}").into());
                }
            }
        });
    };

    // Pre-load commands at mount so guided mode (/session Enter) works immediately
    fetch_commands();

    // Update filtered commands based on current input
    let update_palette = move |text: &str| {
        if text.starts_with('/') {
            let after_slash = &text[1..]; // safe: '/' is single-byte ASCII
            let cmds = all_commands.get_untracked();
            let ns = current_namespace.get_untracked();

            // Detect if user typed "/namespace " pattern → auto-enter namespace
            if ns.is_none() && !after_slash.is_empty() {
                let parts: Vec<&str> = after_slash.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    let maybe_ns = parts[0];
                    let sub_query = parts[1];
                    if let Some(parent) = cmds.iter().find(|c| c.key == maybe_ns && c.is_namespace) {
                        // User typed "/session ..." — auto-enter namespace context
                        current_namespace.set(Some(parent.key.clone()));
                        let entries = build_palette_entries(&cmds, &Some(parent.key.clone()), sub_query);
                        palette_entries.set(entries);
                        selected_index.set(0);
                        show_palette.set(true);
                        return;
                    }
                }
            }

            let query = if let Some(ref ns_key) = ns {
                // Inside namespace: query is after "/namespace "
                let prefix = format!("{} ", ns_key);
                if after_slash.starts_with(&prefix) {
                    &after_slash[prefix.len()..]
                } else if after_slash == ns_key.as_str() {
                    ""
                } else {
                    // User changed text away from namespace — exit namespace
                    current_namespace.set(None);
                    after_slash
                }
            } else {
                after_slash
            };

            let ns = current_namespace.get_untracked();
            let entries = build_palette_entries(&cmds, &ns, query);
            palette_entries.set(entries);
            selected_index.set(0);
            show_palette.set(true);
            // Fetch commands on first '/' if not yet loaded
            fetch_commands();
        } else {
            show_palette.set(false);
            current_namespace.set(None);
        }
    };

    // Handle palette entry selection
    let select_palette_entry = move |entry: &PaletteEntry| {
        if entry.is_back {
            // Go back to root level
            current_namespace.set(None);
            input_text.set("/".to_string());
            let cmds = all_commands.get_untracked();
            let entries = build_palette_entries(&cmds, &None, "");
            palette_entries.set(entries);
            selected_index.set(0);
            // Keep palette open
        } else if entry.is_namespace {
            // Drill into namespace
            current_namespace.set(Some(entry.label.clone()));
            input_text.set(format!("/{} ", entry.label));
            let cmds = all_commands.get_untracked();
            let ns = Some(entry.label.clone());
            let entries = build_palette_entries(&cmds, &ns, "");
            palette_entries.set(entries);
            selected_index.set(0);
            // Keep palette open to show children
        } else {
            // Leaf command — insert and close palette
            input_text.set(entry.full_command.clone());
            show_palette.set(false);
            current_namespace.set(None);
        }
    };

    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if show_palette.get_untracked() {
            let entries = palette_entries.get_untracked();
            let count = entries.len();
            match ev.key().as_str() {
                "ArrowDown" => {
                    ev.prevent_default();
                    if count > 0 {
                        selected_index.set((selected_index.get_untracked() + 1) % count);
                    }
                }
                "ArrowUp" => {
                    ev.prevent_default();
                    if count > 0 {
                        let cur = selected_index.get_untracked();
                        selected_index.set(if cur == 0 { count - 1 } else { cur - 1 });
                    }
                }
                "Tab" | "Enter" => {
                    ev.prevent_default();
                    let idx = selected_index.get_untracked();
                    if idx < count {
                        select_palette_entry(&entries[idx]);
                    }
                }
                "Escape" => {
                    ev.prevent_default();
                    // If inside namespace, go back instead of closing
                    if current_namespace.get_untracked().is_some() {
                        current_namespace.set(None);
                        input_text.set("/".to_string());
                        let cmds = all_commands.get_untracked();
                        let new_entries = build_palette_entries(&cmds, &None, "");
                        palette_entries.set(new_entries);
                        selected_index.set(0);
                    } else {
                        show_palette.set(false);
                    }
                }
                _ => {}
            }
            return;
        }
        // Guided mode: if user types "/namespace" (exact match) and presses Enter,
        // check if it matches a namespace and open its children instead of sending
        if ev.key() == "Enter" && !ev.shift_key() {
            let text = input_text.get_untracked();
            let trimmed = text.trim();
            if trimmed.starts_with('/') && !trimmed.contains(' ') {
                let maybe_ns = &trimmed[1..];
                let cmds = all_commands.get_untracked();
                if let Some(parent) = cmds.iter().find(|c| c.key == maybe_ns && c.is_namespace) {
                    ev.prevent_default();
                    current_namespace.set(Some(parent.key.clone()));
                    input_text.set(format!("/{} ", parent.key));
                    let ns = Some(parent.key.clone());
                    let entries = build_palette_entries(&cmds, &ns, "");
                    palette_entries.set(entries);
                    selected_index.set(0);
                    show_palette.set(true);
                    return;
                }
            }
            ev.prevent_default();
            send_message();
        }
    };

    let can_send = Memo::new(move |_| {
        (!input_text.get().trim().is_empty() || !attachments.get().is_empty()) && !is_sending.get()
    });

    // Click the hidden file input
    let on_attach_click = move |_: web_sys::MouseEvent| {
        if let Some(input) = file_input_ref.get() {
            let el: &web_sys::HtmlInputElement = &input;
            el.click();
        }
    };

    // Handle file selection
    let on_file_change = move |_ev: web_sys::Event| {
        let Some(input) = file_input_ref.get() else { return };
        let el: &web_sys::HtmlInputElement = &input;
        let Some(file_list) = el.files() else { return };

        for i in 0..file_list.length() {
            let Some(file) = file_list.get(i) else { continue };
            let name = file.name();
            let mime_type = file.type_();
            let size = file.size() as u64;

            let reader = match web_sys::FileReader::new() {
                Ok(r) => r,
                Err(_) => continue,
            };

            let reader_clone = reader.clone();
            let attachments_signal = attachments;
            let file_name = name.clone();
            let file_mime = if mime_type.is_empty() {
                "application/octet-stream".to_string()
            } else {
                mime_type
            };

            let onload = Closure::wrap(Box::new(move || {
                if let Ok(result) = reader_clone.result() {
                    if let Some(data_url) = result.as_string() {
                        // data URL format: "data:<mime>;base64,<data>"
                        let base64_data = data_url
                            .split(',')
                            .nth(1)
                            .unwrap_or("")
                            .to_string();

                        let attachment = FileAttachment {
                            name: file_name.clone(),
                            mime_type: file_mime.clone(),
                            data_base64: base64_data,
                            size,
                        };

                        attachments_signal.update(|list| list.push(attachment));
                    }
                }
            }) as Box<dyn Fn()>);

            reader.set_onload(Some(onload.as_ref().unchecked_ref()));
            onload.forget();

            let _ = reader.read_as_data_url(&file);
        }

        // Reset input so the same file can be re-selected
        el.set_value("");
    };

    // Remove attachment by index
    let remove_attachment = move |idx: usize| {
        attachments.update(|list| {
            if idx < list.len() {
                list.remove(idx);
            }
        });
    };

    // Abort button handler
    let on_abort = move |_: web_sys::MouseEvent| {
        if let Some(run_id) = chat.active_run_id.get() {
            let dash = dashboard;
            spawn_local(async move {
                let _ = ChatApi::abort(&dash, &run_id).await;
            });
        }
    };

    view! {
        <div class="border-t border-border px-4 py-3">
            // Attachment preview bar
            <Show when=move || !attachments.get().is_empty()>
                <div class="flex flex-wrap gap-2 mb-2">
                    <For
                        each=move || {
                            attachments.get().into_iter().enumerate().collect::<Vec<_>>()
                        }
                        key=|(idx, f)| format!("{}:{}", idx, f.name)
                        children=move |(idx, file)| {
                            let file_name = file.name.clone();
                            let file_size = format_size(file.size);
                            view! {
                                <div class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg
                                            bg-surface-raised border border-border text-xs text-text-secondary">
                                    // File icon
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-3.5 h-3.5 text-text-tertiary shrink-0"
                                         viewBox="0 0 20 20" fill="currentColor">
                                        <path fill-rule="evenodd"
                                              d="M4 4a2 2 0 0 1 2-2h4.586A2 2 0 0 1 12 2.586L15.414 6A2 2 0 0 1 16 7.414V16a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4Z"
                                              clip-rule="evenodd" />
                                    </svg>
                                    <span class="max-w-[120px] truncate">{file_name}</span>
                                    <span class="text-text-tertiary">{file_size}</span>
                                    // Delete button
                                    <button
                                        class="ml-0.5 p-0.5 rounded hover:bg-danger/10 hover:text-danger transition-colors"
                                        title="Remove"
                                        on:click=move |_| remove_attachment(idx)
                                    >
                                        <svg xmlns="http://www.w3.org/2000/svg" class="w-3 h-3" viewBox="0 0 20 20" fill="currentColor">
                                            <path d="M6.28 5.22a.75.75 0 0 0-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 1 0 1.06 1.06L10 11.06l3.72 3.72a.75.75 0 1 0 1.06-1.06L11.06 10l3.72-3.72a.75.75 0 0 0-1.06-1.06L10 8.94 6.28 5.22Z" />
                                        </svg>
                                    </button>
                                </div>
                            }
                        }
                    />
                </div>
            </Show>

            // Slash command palette (above the input)
            <Show when=move || show_palette.get() && !palette_entries.get().is_empty()>
                <div class="mb-1 rounded-xl border border-border bg-surface-raised shadow-lg
                            max-h-[200px] overflow-y-auto">
                    // Namespace breadcrumb header
                    <Show when=move || current_namespace.get().is_some()>
                        <div class="px-3 py-1.5 text-xs text-text-tertiary border-b border-border">
                            "/" {move || current_namespace.get().unwrap_or_default()} " >"
                        </div>
                    </Show>
                    <For
                        each=move || {
                            palette_entries.get().into_iter().enumerate().collect::<Vec<_>>()
                        }
                        key=|(_, entry)| format!("{}:{}", entry.label, entry.is_back)
                        children=move |(idx, entry)| {
                            let label = entry.label.clone();
                            let desc = entry.description.clone();
                            let entry_for_click = entry.clone();
                            let is_back = entry.is_back;
                            let is_ns = entry.is_namespace;
                            let ns_ctx = current_namespace.get_untracked();
                            view! {
                                <button
                                    class=move || {
                                        let base = "w-full text-left px-3 py-2 flex items-baseline gap-2 text-sm transition-colors";
                                        if selected_index.get() == idx {
                                            format!("{base} bg-primary/10 text-primary")
                                        } else {
                                            format!("{base} hover:bg-surface-sunken text-text-primary")
                                        }
                                    }
                                    on:mousedown=move |ev| {
                                        ev.prevent_default(); // keep textarea focus
                                        select_palette_entry(&entry_for_click);
                                    }
                                >
                                    {if is_back {
                                        view! {
                                            <span class="text-text-tertiary shrink-0">{label.clone()}</span>
                                        }.into_any()
                                    } else if ns_ctx.is_some() {
                                        // Inside namespace: show subcommand without leading /
                                        view! {
                                            <span class="font-medium shrink-0">{label.clone()}</span>
                                        }.into_any()
                                    } else {
                                        // Root level: show with leading /
                                        view! {
                                            <span class="font-medium shrink-0">"/" {label.clone()}</span>
                                        }.into_any()
                                    }}
                                    <span class="text-text-secondary text-xs truncate">{desc}</span>
                                    {if is_ns {
                                        Some(view! {
                                            <span class="ml-auto text-text-tertiary text-xs shrink-0">"\u{25B8}"</span>
                                        })
                                    } else {
                                        None
                                    }}
                                </button>
                            }
                        }
                    />
                </div>
            </Show>

            <div class="flex items-end gap-2">
                // Hidden file input
                <input
                    type="file"
                    multiple=true
                    class="hidden"
                    node_ref=file_input_ref
                    on:change=on_file_change
                />

                // Attachment button (paperclip)
                <button
                    class="p-2.5 rounded-xl text-text-secondary hover:text-text-primary
                           hover:bg-surface-raised transition-colors"
                    title="Attach files"
                    on:click=on_attach_click
                >
                    <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5" viewBox="0 0 20 20" fill="currentColor">
                        <path fill-rule="evenodd"
                              d="M15.621 4.379a3 3 0 0 0-4.242 0l-7 7a3 3 0 0 0 4.241 4.243h.001l.497-.5a.75.75 0 0 1 1.064 1.057l-.498.501-.002.002a4.5 4.5 0 0 1-6.364-6.364l7-7a4.5 4.5 0 0 1 6.368 6.36l-3.455 3.553A2.625 2.625 0 1 1 9.52 9.52l3.45-3.451a.75.75 0 1 1 1.061 1.06l-3.45 3.451a1.125 1.125 0 0 0 1.587 1.595l3.454-3.553a3 3 0 0 0 0-4.242Z"
                              clip-rule="evenodd" />
                    </svg>
                </button>

                <textarea
                    class="flex-1 resize-none rounded-xl border border-border bg-surface-sunken px-4 py-2.5
                           text-sm text-text-primary placeholder:text-text-tertiary
                           focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary
                           min-h-[40px] max-h-[120px]"
                    placeholder="Send a message..."
                    rows=1
                    prop:value=move || input_text.get()
                    on:input=move |ev| {
                        let val = event_target_value(&ev);
                        input_text.set(val.clone());
                        update_palette(&val);
                    }
                    on:keydown=on_keydown
                />

                // Abort button (when running)
                <Show when=move || chat.active_run_id.get().is_some()>
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
                <Show when=move || chat.active_run_id.get().is_none()>
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
