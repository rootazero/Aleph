// apps/panel/src/views/chat/view.rs
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

/// A single command entry from the Gateway.
#[derive(Clone, Debug)]
struct CommandInfo {
    key: String,
    description: String,
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
    let filtered_commands: RwSignal<Vec<CommandInfo>> = RwSignal::new(Vec::new());
    let selected_index = RwSignal::new(0usize);
    let commands_loaded = RwSignal::new(false);

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
        let dash = dashboard;
        spawn_local(async move {
            let sk = session_key.as_deref();
            match ChatApi::send(&dash, &text, sk, api_attachments).await {
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
                            if let (Some(key), Some(desc)) = (
                                item.get("key").and_then(|v| v.as_str()),
                                item.get("description").and_then(|v| v.as_str()),
                            ) {
                                cmds.push(CommandInfo {
                                    key: key.to_string(),
                                    description: desc.to_string(),
                                });
                            }
                        }
                    }
                    all_commands.set(cmds.clone());
                    commands_loaded.set(true);
                    // Refresh palette with newly loaded commands
                    let text = input_text.get_untracked();
                    if text.starts_with('/') {
                        let query = &text[1..];
                        let matches: Vec<CommandInfo> = if query.is_empty() {
                            cmds
                        } else {
                            let q = query.to_lowercase();
                            cmds.into_iter()
                                .filter(|c| c.key.to_lowercase().contains(&q) || c.description.to_lowercase().contains(&q))
                                .collect()
                        };
                        filtered_commands.set(matches);
                        selected_index.set(0);
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to fetch commands: {e}").into());
                }
            }
        });
    };

    // Update filtered commands based on current input
    let update_palette = move |text: &str| {
        if text.starts_with('/') {
            let query = &text[1..]; // safe: '/' is single-byte ASCII
            let cmds = all_commands.get_untracked();
            let matches: Vec<CommandInfo> = if query.is_empty() {
                cmds
            } else {
                let q = query.to_lowercase();
                cmds.into_iter()
                    .filter(|c| c.key.to_lowercase().contains(&q) || c.description.to_lowercase().contains(&q))
                    .collect()
            };
            filtered_commands.set(matches);
            selected_index.set(0);
            show_palette.set(true);
            // Fetch commands on first '/' if not yet loaded
            fetch_commands();
        } else {
            show_palette.set(false);
        }
    };

    // Insert selected command into input
    let insert_command = move |cmd: &CommandInfo| {
        input_text.set(format!("/{} ", cmd.key));
        show_palette.set(false);
    };

    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if show_palette.get_untracked() {
            let cmds = filtered_commands.get_untracked();
            let count = cmds.len();
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
                        insert_command(&cmds[idx]);
                    }
                }
                "Escape" => {
                    ev.prevent_default();
                    show_palette.set(false);
                }
                _ => {}
            }
            return;
        }
        if ev.key() == "Enter" && !ev.shift_key() {
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
            <Show when=move || show_palette.get() && !filtered_commands.get().is_empty()>
                <div class="mb-1 rounded-xl border border-border bg-surface-raised shadow-lg
                            max-h-[200px] overflow-y-auto">
                    <For
                        each=move || {
                            filtered_commands.get().into_iter().enumerate().collect::<Vec<_>>()
                        }
                        key=|(_, cmd)| cmd.key.clone()
                        children=move |(idx, cmd)| {
                            let key = cmd.key.clone();
                            let desc = cmd.description.clone();
                            let cmd_for_click = cmd.clone();
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
                                        insert_command(&cmd_for_click);
                                    }
                                >
                                    <span class="font-medium shrink-0">"/" {key}</span>
                                    <span class="text-text-secondary text-xs truncate">{desc}</span>
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
