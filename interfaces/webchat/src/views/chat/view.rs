//! Main Chat view — message list + input area.

use super::events::subscribe_run_events;
use super::project_menu::ProjectMenu;
use super::state::{
    ChatMessage, ChatPhase, ChatSendError, ChatSendErrorCode, ChatState, PendingAttachment,
};
use crate::api::chat::{ChatApi, ChatAttachment};
use crate::components::markdown::{MarkdownRenderer, StreamingRenderer};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;
use shared_ui_logic::safety::{check_prompt_injection, prompt_guard_message, PromptInjectionVerdict};
use wasm_bindgen::prelude::*;

/// Top-level Chat view component.
#[component]
pub fn ChatView() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    // ChatState is provided once at the app root so the chat sidebar
    // (left column) and this view share one session / agent selection.
    let chat = expect_context::<ChatState>();

    // Subscribe to run.* events directly (no Effect — this is a one-shot mount action)
    let sub_id = subscribe_run_events(&dashboard, chat);

    // Tell the Gateway to start forwarding stream.* events
    // (backend publishes events with method "stream.run_accepted", "stream.response_chunk", etc.)
    // Wait until connected before subscribing, since ChatView may mount before WebSocket is ready.
    let dash_for_sub = dashboard;
    spawn_local(async move {
        // Poll until connected (max ~5s)
        for _ in 0..50 {
            if dash_for_sub.is_connected.get_untracked() {
                break;
            }
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

    // ---- G5: chat-surface drop zone ----
    // Listening on the root div so a Finder/Explorer drop anywhere over
    // the chat (messages list, composer, anywhere in between) routes the
    // files through the same attachment pipeline as the paperclip input.
    let on_dragover = move |ev: web_sys::DragEvent| {
        // Only react to file drags — keeps text-selection drag inside
        // bubbles silent. `types` reports "Files" when a file is over us.
        if let Some(dt) = ev.data_transfer() {
            let types = dt.types();
            let mut has_files = false;
            for i in 0..types.length() {
                if let Some(s) = types.get(i).as_string() {
                    if s == "Files" {
                        has_files = true;
                        break;
                    }
                }
            }
            if has_files {
                ev.prevent_default(); // mandatory for drop to fire
                chat.is_dragging_files.set(true);
            }
        }
    };
    let on_dragleave = move |_: web_sys::DragEvent| {
        chat.is_dragging_files.set(false);
    };
    let on_drop = move |ev: web_sys::DragEvent| {
        ev.prevent_default();
        chat.is_dragging_files.set(false);
        let Some(dt) = ev.data_transfer() else { return };
        let Some(files) = dt.files() else { return };
        let attachments = chat.pending_attachments;
        for i in 0..files.length() {
            let Some(file) = files.get(i) else { continue };
            ingest_dropped_file(file, attachments);
        }
    };

    view! {
        // Transparent — the shell's light-field shows through.
        <div
            class="relative flex flex-col h-full"
            on:dragover=on_dragover
            on:dragleave=on_dragleave
            on:drop=on_drop
        >
            // Message list (scrollable) — or the welcome hero when empty
            <MessageList />
            // Input area (pinned to bottom)
            <InputArea />
            // Drop overlay — only visible while a file is hovering.
            <Show when=move || chat.is_dragging_files.get()>
                <div class="absolute inset-0 z-30 pointer-events-none
                            flex items-center justify-center
                            bg-primary/10 border-2 border-dashed border-primary/40 rounded-lg
                            backdrop-blur-[1px]">
                    <div class="flex flex-col items-center gap-2 text-primary font-medium">
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-10 h-10" viewBox="0 0 24 24"
                             fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>
                            <polyline points="17 8 12 3 7 8"/>
                            <line x1="12" y1="3" x2="12" y2="15"/>
                        </svg>
                        <span class="text-sm">"Drop to attach"</span>
                    </div>
                </div>
            </Show>
        </div>
    }
}

/// Read a single dropped `web_sys::File` into base64 and append it to the
/// shared pending-attachments list. Mirrors the per-file logic used by the
/// paperclip input — kept as a free fn so both the change handler and the
/// drop handler stay tight.
fn ingest_dropped_file(file: web_sys::File, attachments: RwSignal<Vec<PendingAttachment>>) {
    let name = file.name();
    let mime_type = file.type_();
    let size = file.size() as u64;
    let Ok(reader) = web_sys::FileReader::new() else {
        return;
    };
    let reader_clone = reader.clone();
    let file_name = name.clone();
    let file_mime = if mime_type.is_empty() {
        "application/octet-stream".to_string()
    } else {
        mime_type
    };
    let onload = Closure::wrap(Box::new(move || {
        if let Ok(result) = reader_clone.result() {
            if let Some(data_url) = result.as_string() {
                let base64_data = data_url.split(',').nth(1).unwrap_or("").to_string();
                let attachment = PendingAttachment {
                    name: file_name.clone(),
                    mime_type: file_mime.clone(),
                    data_base64: base64_data,
                    size,
                };
                attachments.update(|list| list.push(attachment));
            }
        }
    }) as Box<dyn Fn()>);
    reader.set_onload(Some(onload.as_ref().unchecked_ref()));
    onload.forget();
    let _ = reader.read_as_data_url(&file);
}

/// Welcome hero — shown in the message area while a conversation is empty.
/// A breathing ℵ orb above a shimmering greeting, with a staggered reveal.
#[component]
fn ChatHero() -> impl IntoView {
    view! {
        <div class="h-full flex flex-col items-center justify-center px-6 pb-10 text-center select-none">
            <div class="aleph-rise mb-7" style="animation-delay: 0s">
                <div class="aleph-hero-orb w-16 h-16 rounded-2xl flex items-center justify-center">
                    // Brand mark — Hebrew aleph, same glyph the app icon uses.
                    <svg viewBox="0 0 100 100" class="w-9 h-9" xmlns="http://www.w3.org/2000/svg">
                        <text x="50" y="78"
                              font-size="92"
                              font-family="'Frank Ruhl Libre','David Libre','Times New Roman','Times','Arial Hebrew',serif"
                              text-anchor="middle"
                              fill="white">"\u{05D0}"</text>
                    </svg>
                </div>
            </div>
            <div class="aleph-rise" style="animation-delay: 0.09s">
                <h2 class="aleph-hero-title text-[2rem] leading-tight font-semibold tracking-tight">
                    "我们从哪里开始？"
                </h2>
            </div>
            <p
                class="aleph-rise mt-3 text-sm text-text-tertiary max-w-sm leading-relaxed"
                style="animation-delay: 0.18s"
            >
                "直接输入消息，或输入 / 调用命令与技能。"
            </p>
        </div>
    }
}

/// Scrollable message list — stick-to-bottom variant.
///
/// Replaces the cycle-1 "always slam to scroll_height" behavior with the
/// pattern from openhuman's `useStickToBottom`: we only auto-scroll when
/// the user is already near the bottom (≤64px). If they've scrolled up to
/// read history, new content lands silently and a "↓ New messages" pill
/// appears so they can opt in.
#[component]
fn MessageList() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();
    let scroll_ref = NodeRef::<leptos::html::Div>::new();
    // True iff the viewport bottom is within 64px of scroll_height.
    let stuck_to_bottom = RwSignal::new(true);
    // Pulse incremented whenever new content lands while NOT stuck — drives
    // the "↓ New messages" pill so a user reading history sees it appear.
    let unseen_below = RwSignal::new(false);

    const STICK_THRESHOLD_PX: f64 = 64.0;

    // Scroll event handler updates stuck_to_bottom + clears unseen_below
    // when the user returns to the bottom on their own.
    let on_scroll = move |_ev: web_sys::Event| {
        if let Some(el) = scroll_ref.get() {
            let el: &web_sys::HtmlElement = &el;
            let distance =
                f64::from(el.scroll_height()) - f64::from(el.scroll_top()) - f64::from(el.client_height());
            let near_bottom = distance <= STICK_THRESHOLD_PX;
            stuck_to_bottom.set(near_bottom);
            if near_bottom {
                unseen_below.set(false);
            }
        }
    };

    // Reactive auto-scroll — only when the user is already at the bottom.
    Effect::new(move |_| {
        // Subscribe to message/phase changes (read both untracked-style for re-runs)
        let _msgs = chat.messages.get();
        let _phase = chat.phase.get();
        if let Some(el) = scroll_ref.get() {
            if stuck_to_bottom.get_untracked() {
                let el: &web_sys::HtmlElement = &el;
                el.set_scroll_top(el.scroll_height());
            } else {
                unseen_below.set(true);
            }
        }
    });

    // Jump-to-bottom button handler (also re-arms stickiness).
    let on_jump = move |_: web_sys::MouseEvent| {
        if let Some(el) = scroll_ref.get() {
            let el: &web_sys::HtmlElement = &el;
            el.set_scroll_top(el.scroll_height());
            stuck_to_bottom.set(true);
            unseen_below.set(false);
        }
    };

    view! {
        <div class="relative flex-1 min-h-0">
            <div node_ref=scroll_ref class="absolute inset-0 overflow-y-auto" on:scroll=on_scroll>
                <Show
                    when=move || chat.messages.get().is_empty()
                    fallback=move || view! {
                        <div class="max-w-3xl mx-auto px-4 py-6 space-y-4">
                            // Inline send-error banner (G2) — shown when the last
                            // outbound send failed; colour-coded by error code.
                            <SendErrorBanner />
                            <For
                                each=move || chat.messages.get()
                                key=|msg| format!("{}:{}:{}:{}:{}:{}", msg.id, msg.content.len(), msg.is_streaming, msg.is_intermediate, msg.tool_calls.len(), msg.model_info.is_some())
                                children=move |msg| {
                                    view! { <MessageBubble message=msg /> }
                                }
                            />
                            // Thinking indicator
                            <Show when=move || chat.phase.get() == ChatPhase::Thinking>
                                <div class="flex items-center gap-2 text-text-secondary text-sm px-3 py-2">
                                    <span class="inline-block w-2 h-2 rounded-full bg-primary animate-pulse"></span>
                                    {move || t_string!(i18n, chat.thinking).to_string()}
                                </div>
                            </Show>
                        </div>
                    }
                >
                    <ChatHero />
                </Show>
            </div>
            // "↓ New messages" pill — only when user scrolled up AND new
            // content has landed since they last looked at the bottom.
            <Show when=move || unseen_below.get() && !stuck_to_bottom.get()>
                <button
                    class="absolute left-1/2 -translate-x-1/2 bottom-3 z-10
                           px-3 py-1.5 rounded-full text-xs font-medium
                           bg-primary text-white shadow-md hover:bg-primary-hover
                           transition-all flex items-center gap-1"
                    on:click=on_jump
                >
                    <span>"\u{2193}"</span>
                    <span>"New messages"</span>
                </button>
            </Show>
        </div>
    }
}

/// Inline banner for the most recent `ChatSendError`. Empty when none.
#[component]
fn SendErrorBanner() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    view! {
        <Show when=move || chat.send_error.get().is_some()>
            {move || {
                let err = chat.send_error.get();
                err.map(|e| {
                    let is_warning = matches!(e.code, ChatSendErrorCode::PromptReview);
                    let class_str = if is_warning {
                        "px-3 py-2 rounded-lg border text-sm bg-warning-subtle border-warning/30 text-warning"
                    } else {
                        "px-3 py-2 rounded-lg border text-sm bg-danger-subtle border-danger/30 text-danger"
                    };
                    view! {
                        <div class=class_str role="alert">
                            <div class="flex items-start gap-2">
                                <span class="font-mono text-[10px] uppercase tracking-wider opacity-70 shrink-0 pt-0.5">
                                    {format!("{:?}", e.code).to_lowercase()}
                                </span>
                                <span class="flex-1">{e.message}</span>
                                <button
                                    class="opacity-60 hover:opacity-100 shrink-0"
                                    title="Dismiss"
                                    on:click=move |_| {
                                        chat.send_error.set(None);
                                        chat.error_message.set(None);
                                    }
                                >
                                    "\u{2715}"
                                </button>
                            </div>
                        </div>
                    }
                })
            }}
        </Show>
    }
}

/// Single message bubble.
#[component]
fn MessageBubble(message: ChatMessage) -> impl IntoView {
    let is_user = message.role == "user";
    let has_error = message.error.is_some();
    let has_tools = !message.tool_calls.is_empty();

    let bubble_align = if is_user {
        "flex justify-end"
    } else {
        "flex justify-start"
    };
    let bubble_style = if is_user {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-primary text-white"
    } else if has_error {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-danger-subtle text-danger border border-danger/20"
    } else if message.is_intermediate {
        "max-w-[80%] rounded-2xl px-3 py-2 bg-surface-raised/60 text-text-secondary text-sm italic"
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
    let model_info = message.model_info.clone();

    let streaming_cursor = if is_streaming {
        Some(view! {
            <span class="inline-block w-1.5 h-4 bg-primary/60 animate-pulse ml-0.5 align-text-bottom"></span>
        })
    } else {
        None
    };

    let error_view = error.map(|err| {
        view! {
            <div class="mt-2 text-xs text-danger/80">{err}</div>
        }
    });

    // Model info indicator (shows fallback when applicable)
    let model_view = if !is_user {
        model_info.map(|info| {
            if info.is_fallback {
                let original = info.original_model.unwrap_or_default();
                view! {
                    <div class="mt-1.5 text-[10px] leading-tight font-mono flex items-center gap-1">
                        <span style="text-decoration: line-through; opacity: 0.4;">{original}</span>
                        <span class="text-text-tertiary">{"\u{2192}"}</span>
                        <span style="color: #fde047;">{info.model}</span>
                    </div>
                }
                .into_any()
            } else {
                view! {
                    <div class="mt-1.5 text-[10px] leading-tight font-mono text-text-tertiary">
                        {info.model}
                    </div>
                }
                .into_any()
            }
        })
    } else {
        None
    };

    // ---- G4: per-bubble hover actions (Copy + Retry) ----
    // Reach for ChatState so the retry button can pulse the composer
    // without prop-drilling a callback through MessageList → MessageBubble.
    let chat = expect_context::<ChatState>();
    let copy_text = content.clone();
    let on_copy = move |_: web_sys::MouseEvent| {
        let win = match web_sys::window() {
            Some(w) => w,
            None => return,
        };
        // Modern API — navigator.clipboard.writeText(text)
        let clipboard = win.navigator().clipboard();
        let _promise = clipboard.write_text(&copy_text);
    };
    let on_retry = move |_: web_sys::MouseEvent| {
        chat.request_retry();
    };
    // Retry only makes sense on a finalized assistant turn — streaming /
    // intermediate / error messages are noise.
    let show_retry = !is_user && !is_streaming && !has_error && !message.is_intermediate;
    let actions_align = if is_user { "right-2" } else { "left-2" };
    let action_class = format!(
        "absolute -bottom-3 {actions_align} flex items-center gap-1 \
         opacity-0 group-hover:opacity-100 focus-within:opacity-100 \
         transition-opacity"
    );

    view! {
        <div class=format!("{bubble_align} group relative")>
            <div class=bubble_style>
                {tool_calls_view}

                // Message content — Markdown for assistant, plain text for user
                {if is_user {
                    view! {
                        <div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
                            {content.clone()}
                        </div>
                    }.into_any()
                } else if is_streaming {
                    view! {
                        <StreamingRenderer content=content.clone() />
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

                // Model info (with fallback indicator)
                {model_view}
            </div>
            // Hover action bar — Copy (all bubbles) + Retry (final assistant).
            <div class=action_class>
                <button
                    class="px-1.5 h-6 rounded-md bg-surface-raised border border-border
                           text-[11px] text-text-tertiary hover:text-text-primary hover:bg-surface-sunken
                           shadow-sm transition-colors flex items-center gap-1"
                    title="Copy message"
                    on:click=on_copy
                >
                    <svg xmlns="http://www.w3.org/2000/svg" class="w-3 h-3" viewBox="0 0 24 24"
                         fill="none" stroke="currentColor" stroke-width="2"
                         stroke-linecap="round" stroke-linejoin="round">
                        <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                        <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                    </svg>
                    <span>"Copy"</span>
                </button>
                {if show_retry {
                    Some(view! {
                        <button
                            class="px-1.5 h-6 rounded-md bg-surface-raised border border-border
                                   text-[11px] text-text-tertiary hover:text-text-primary hover:bg-surface-sunken
                                   shadow-sm transition-colors flex items-center gap-1"
                            title="Retry last prompt"
                            on:click=on_retry
                        >
                            <svg xmlns="http://www.w3.org/2000/svg" class="w-3 h-3" viewBox="0 0 24 24"
                                 fill="none" stroke="currentColor" stroke-width="2"
                                 stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="23 4 23 10 17 10"></polyline>
                                <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path>
                            </svg>
                            <span>"Retry"</span>
                        </button>
                    })
                } else {
                    None
                }}
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
    let name = item
        .get("name")
        .and_then(|v| v.as_str())
        // Fallback: legacy flat format uses "key"
        .or_else(|| item.get("key").and_then(|v| v.as_str()))?;
    let hint = item
        .get("hint")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("description").and_then(|v| v.as_str()))
        .unwrap_or("");
    let is_namespace = item
        .get("is_namespace")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let param_hint = item
        .get("param_hint")
        .and_then(|v| v.as_str())
        .map(String::from);
    let children = item
        .get("children")
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
    let i18n = use_i18n();
    let input_text = RwSignal::new(String::new());
    let is_sending = RwSignal::new(false);
    // Attachments live on ChatState so the chat-surface drop zone (G5) can
    // share the same list as the paperclip input.
    let attachments = chat.pending_attachments;

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
        if is_sending.get_untracked() {
            return;
        }
        let text = input_text.get_untracked().trim().to_string();
        let files = attachments.get_untracked();
        if text.is_empty() && files.is_empty() {
            return;
        }

        // ---- G1: client-side prompt-injection guard ----
        // Hard-blocks land here so a wasted round-trip is avoided; reviews
        // are surfaced live by the banner below but still allowed through
        // (final authority is server-side security).
        if !text.is_empty() {
            let check = check_prompt_injection(&text);
            if check.verdict == PromptInjectionVerdict::Block {
                chat.set_send_error(ChatSendError::new(
                    ChatSendErrorCode::PromptBlocked,
                    prompt_guard_message(&check),
                ));
                return;
            }
        }

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
        let project_root = chat.active_project_root.get();
        let dash = dashboard;
        spawn_local(async move {
            let sk = session_key.as_deref();
            let aid = agent_id.as_deref();
            let pr = project_root.as_deref();
            match ChatApi::send(&dash, &text, sk, api_attachments, aid, pr).await {
                Ok(resp) => {
                    chat.session_key.set(Some(resp.session_key));
                }
                Err(e) => {
                    // Promote to structured error so the banner can colour-
                    // code (warn vs error) and analytics can branch on code.
                    chat.set_send_error(ChatSendError::classify(e));
                }
            }
            is_sending.set(false);
        });
    };

    // ---- G4 plumbing: composer listens for retry pulses ----
    // MessageBubble's Retry button bumps `chat.retry_pulse` — we re-take
    // the most recent user message and route it through the normal send
    // pipeline (so prompt-guard + idempotency + error classification all
    // apply identically to retry vs first send).
    {
        let chat = chat;
        let send_message = send_message.clone();
        Effect::new(move |prev_pulse: Option<u32>| {
            let pulse = chat.retry_pulse.get();
            // Skip the initial run (Effect fires once on subscription).
            if prev_pulse.is_some() && Some(pulse) != prev_pulse {
                if let Some(last) = chat.last_user_text() {
                    input_text.set(last);
                    send_message();
                }
            }
            pulse
        });
    }

    // Build palette entries from commands based on current namespace and query
    let build_palette_entries =
        move |cmds: &[CommandInfo], namespace: &Option<String>, query: &str| -> Vec<PaletteEntry> {
            let mut entries = Vec::new();
            if let Some(ns) = namespace {
                // Inside a namespace — show "back" + children
                entries.push(PaletteEntry {
                    label: t_string!(i18n, chat.back).to_string(),
                    full_command: "/".to_string(),
                    description: t_string!(i18n, chat.back_desc).to_string(),
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
                        if query.is_empty()
                            || child.key.to_lowercase().contains(&query.to_lowercase())
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
        if commands_loaded.get_untracked() {
            return;
        }
        let dash = dashboard;
        spawn_local(async move {
            // Wait until connected before fetching to avoid "Not connected" errors.
            for _ in 0..50 {
                if dash.is_connected.get_untracked() {
                    break;
                }
                gloo_timers::future::TimeoutFuture::new(100).await;
            }

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
                    if let Some(query) = text.strip_prefix('/') {
                        // safe: '/' is single-byte ASCII
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
        if let Some(after_slash) = text.strip_prefix('/') {
            // safe: '/' is single-byte ASCII
            let cmds = all_commands.get_untracked();
            let ns = current_namespace.get_untracked();

            // Detect if user typed "/namespace " pattern → auto-enter namespace
            if ns.is_none() && !after_slash.is_empty() {
                let parts: Vec<&str> = after_slash.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    let maybe_ns = parts[0];
                    let sub_query = parts[1];
                    if let Some(parent) = cmds.iter().find(|c| c.key == maybe_ns && c.is_namespace)
                    {
                        // User typed "/session ..." — auto-enter namespace context
                        current_namespace.set(Some(parent.key.clone()));
                        let entries =
                            build_palette_entries(&cmds, &Some(parent.key.clone()), sub_query);
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
        let Some(input) = file_input_ref.get() else {
            return;
        };
        let el: &web_sys::HtmlInputElement = &input;
        let Some(file_list) = el.files() else { return };

        for i in 0..file_list.length() {
            let Some(file) = file_list.get(i) else {
                continue;
            };
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
                        let base64_data = data_url.split(',').nth(1).unwrap_or("").to_string();

                        let attachment = PendingAttachment {
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
        <div class="px-4 pb-5 pt-2">
            <div class="max-w-3xl mx-auto">
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
                                        title=move || t_string!(i18n, chat.remove).to_string()
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
                <div class="mb-2 rounded-2xl border border-border bg-surface-raised shadow-lg
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

            // Live prompt-injection guard banner (G1) — pure UI heuristic
            // that hints to the user when their draft looks like an
            // override / exfiltration / jailbreak attempt. Final authority
            // is server-side; this lives here so they can rephrase before
            // sending and wasting a model call.
            {move || {
                let text = input_text.get();
                if text.trim().is_empty() {
                    return None;
                }
                let check = check_prompt_injection(&text);
                if check.verdict == PromptInjectionVerdict::Allow {
                    return None;
                }
                let is_block = check.verdict == PromptInjectionVerdict::Block;
                let cls = if is_block {
                    "mx-1 mb-1 px-2.5 py-1.5 rounded-md text-xs border bg-danger-subtle border-danger/30 text-danger"
                } else {
                    "mx-1 mb-1 px-2.5 py-1.5 rounded-md text-xs border bg-warning-subtle border-warning/30 text-warning"
                };
                Some(view! {
                    <div class=cls role="status">
                        {prompt_guard_message(&check)}
                    </div>
                })
            }}

            // Project picker — sits directly above the composer so the
            // dropdown can flip upward (composer lives at the bottom of the
            // viewport; downward menu would clip).
            <div class="aleph-project-row px-1 pb-1">
                <ProjectMenu />
            </div>

            // Compact single-row composer — paperclip | textarea | send,
            // all on one 32 px baseline. Textarea starts at 32 px (matches
            // the side buttons so the caret aligns with both icons) and
            // grows up to 140 px when multi-line input is typed
            // (Shift+Enter); items-end keeps the side buttons pinned to the
            // composer's bottom edge as the textarea grows.
            <div class="aleph-composer flex items-end gap-2 px-3 py-1.5">
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
                    class="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary
                           hover:bg-surface-sunken transition-colors flex-shrink-0"
                    title=move || t_string!(i18n, chat.attach).to_string()
                    on:click=on_attach_click
                >
                    <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5" viewBox="0 0 20 20" fill="currentColor">
                        <path fill-rule="evenodd"
                              d="M15.621 4.379a3 3 0 0 0-4.242 0l-7 7a3 3 0 0 0 4.241 4.243h.001l.497-.5a.75.75 0 0 1 1.064 1.057l-.498.501-.002.002a4.5 4.5 0 0 1-6.364-6.364l7-7a4.5 4.5 0 0 1 6.368 6.36l-3.455 3.553A2.625 2.625 0 1 1 9.52 9.52l3.45-3.451a.75.75 0 1 1 1.061 1.06l-3.45 3.451a1.125 1.125 0 0 0 1.587 1.595l3.454-3.553a3 3 0 0 0 0-4.242Z"
                              clip-rule="evenodd" />
                    </svg>
                </button>

                <textarea
                    class="flex-1 min-w-0 resize-none bg-transparent px-1 py-[6px] text-sm leading-snug
                           text-text-primary placeholder:text-text-tertiary
                           focus:outline-none min-h-[32px] max-h-[140px]"
                    placeholder=move || t_string!(i18n, chat.send_placeholder).to_string()
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
                        class="w-8 h-8 rounded-full bg-danger/15 text-danger flex items-center
                               justify-center hover:bg-danger/25 transition-colors flex-shrink-0"
                        title=move || t_string!(i18n, chat.stop).to_string()
                        on:click=on_abort
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-3.5 h-3.5" viewBox="0 0 20 20" fill="currentColor">
                            <rect x="4" y="4" width="12" height="12" rx="2" />
                        </svg>
                    </button>
                </Show>

                // Send button (when idle)
                <Show when=move || chat.active_run_id.get().is_none()>
                    <button
                        class="w-8 h-8 rounded-full bg-primary text-white flex items-center
                               justify-center shadow-sm hover:bg-primary-hover
                               disabled:opacity-35 disabled:cursor-not-allowed
                               disabled:shadow-none transition-all flex-shrink-0"
                        disabled=move || !can_send.get()
                        on:click=move |_| send_message()
                    >
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"
                             stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"
                             class="w-4 h-4">
                            <path d="M12 19V5" />
                            <path d="M5 12l7-7 7 7" />
                        </svg>
                    </button>
                </Show>
            </div>

            </div>
        </div>
    }
}
