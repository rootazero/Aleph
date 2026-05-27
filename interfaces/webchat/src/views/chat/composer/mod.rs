//! Chat composer — textarea, attachments, slash-command palette,
//! send / abort. The orchestrator only; render plane lives in the
//! `palette` and `attachments` submodules so this file owns only the
//! glue (signals, closures, top-level view layout).
//!
//! Originally a single 815 LOC `composer.rs`. Split per CLAUDE.md P2
//! (high cohesion, low coupling) — each submodule is independently
//! testable and the orchestrator now fits inside the 400 LOC ceiling.

mod attachments;
mod palette;

use attachments::{read_file_list_into, AttachmentPreviewBar};
use palette::{
    build_palette_entries, parse_command_info, CommandInfo, PaletteEntry, PaletteLabels,
    SlashPaletteView,
};

use super::project_menu::ProjectMenu;
use super::state::{ChatSendError, ChatSendErrorCode, ChatState};
use crate::api::chat::{ChatApi, ChatAttachment};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;
use shared_ui_logic::safety::{check_prompt_injection, prompt_guard_message, PromptInjectionVerdict};

/// Textarea + side buttons + palette popup + injection-guard banner.
/// Mounted by [`super::view::ChatView`] at the viewport bottom.
#[component]
pub(super) fn InputArea() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    let input_text = RwSignal::new(String::new());
    let is_sending = RwSignal::new(false);
    // Attachments live on ChatState so the chat-surface drop zone (G5)
    // can share the same list as the paperclip input.
    let attachments = chat.pending_attachments;

    // Palette state — populated lazily on first `/`.
    let all_commands: RwSignal<Vec<CommandInfo>> = RwSignal::new(Vec::new());
    let show_palette = RwSignal::new(false);
    let palette_entries: RwSignal<Vec<PaletteEntry>> = RwSignal::new(Vec::new());
    let selected_index = RwSignal::new(0usize);
    let commands_loaded = RwSignal::new(false);
    let current_namespace: RwSignal<Option<String>> = RwSignal::new(None);

    let file_input_ref = NodeRef::<leptos::html::Input>::new();

    // i18n labels threaded into the pure palette builder so palette.rs
    // stays free of i18n macros (and unit-testable).
    let palette_labels = move || PaletteLabels {
        back: t_string!(i18n, chat.back).to_string(),
        back_desc: t_string!(i18n, chat.back_desc).to_string(),
    };

    let send_message = move || {
        if is_sending.get_untracked() {
            return;
        }
        let text = input_text.get_untracked().trim().to_string();
        let files = attachments.get_untracked();
        if text.is_empty() && files.is_empty() {
            return;
        }

        // G1 client-side prompt-injection guard — hard-blocks save a
        // wasted round-trip; reviews are surfaced live by the banner
        // below but still permitted through (server is final authority).
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
        // Per-turn model override stamped on ChatState → daemon's run
        // loop short-circuits its provider-fallback chain.
        let model_override = chat.selected_model.get();
        let dash = dashboard;
        spawn_local(async move {
            let sk = session_key.as_deref();
            let aid = agent_id.as_deref();
            let pr = project_root.as_deref();
            let mo = model_override.as_ref();
            match ChatApi::send(&dash, &text, sk, api_attachments, aid, pr, mo).await {
                Ok(resp) => {
                    chat.session_key.set(Some(resp.session_key));
                }
                Err(e) => {
                    // Structured error → banner colour-codes (warn vs
                    // error); analytics can branch on the code.
                    chat.set_send_error(ChatSendError::classify(e));
                }
            }
            is_sending.set(false);
        });
    };

    // G4 retry plumbing — MessageBubble's Retry button bumps
    // `chat.retry_pulse`; we re-take the most recent user message and
    // route it through the normal send pipeline so prompt-guard +
    // idempotency + error classification apply identically.
    {
        let send_message = send_message.clone();
        Effect::new(move |prev_pulse: Option<u32>| {
            let pulse = chat.retry_pulse.get();
            if prev_pulse.is_some() && Some(pulse) != prev_pulse {
                if let Some(last) = chat.last_user_text() {
                    input_text.set(last);
                    send_message();
                }
            }
            pulse
        });
    }

    // Fetch the catalogue once, then refresh whatever the user already
    // typed. Idempotent — guarded by `commands_loaded`.
    let fetch_commands = move || {
        if commands_loaded.get_untracked() {
            return;
        }
        let dash = dashboard;
        let labels = palette_labels();
        spawn_local(async move {
            // Wait until connected to avoid the "Not connected" error.
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
                    // Refresh palette in case the user already typed `/`
                    // while we were waiting for the catalogue.
                    let text = input_text.get_untracked();
                    if let Some(query) = text.strip_prefix('/') {
                        let ns = current_namespace.get_untracked();
                        let entries = build_palette_entries(&cmds, &ns, query, &labels);
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
    fetch_commands();

    // Recompute palette entries from the current draft text. Handles
    // guided drilldown ("/session ...") and auto-exit when the user
    // breaks the prefix.
    let update_palette = move |text: &str| {
        let labels = palette_labels();
        if let Some(after_slash) = text.strip_prefix('/') {
            let cmds = all_commands.get_untracked();
            let ns = current_namespace.get_untracked();

            // Auto-enter "/namespace " when seen for the first time.
            if ns.is_none() && !after_slash.is_empty() {
                let parts: Vec<&str> = after_slash.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    let maybe_ns = parts[0];
                    let sub_query = parts[1];
                    if let Some(parent) =
                        cmds.iter().find(|c| c.key == maybe_ns && c.is_namespace)
                    {
                        current_namespace.set(Some(parent.key.clone()));
                        let entries = build_palette_entries(
                            &cmds,
                            &Some(parent.key.clone()),
                            sub_query,
                            &labels,
                        );
                        palette_entries.set(entries);
                        selected_index.set(0);
                        show_palette.set(true);
                        return;
                    }
                }
            }

            let query = if let Some(ref ns_key) = ns {
                let prefix = format!("{} ", ns_key);
                if after_slash.starts_with(&prefix) {
                    &after_slash[prefix.len()..]
                } else if after_slash == ns_key.as_str() {
                    ""
                } else {
                    // User text diverged from "/namespace " — exit.
                    current_namespace.set(None);
                    after_slash
                }
            } else {
                after_slash
            };

            let ns = current_namespace.get_untracked();
            let entries = build_palette_entries(&cmds, &ns, query, &labels);
            palette_entries.set(entries);
            selected_index.set(0);
            show_palette.set(true);
            // Lazy first-use trigger for the catalogue fetch.
            fetch_commands();
        } else {
            show_palette.set(false);
            current_namespace.set(None);
        }
    };

    // Palette row selected (mousedown or Tab/Enter).
    let select_palette_entry = move |entry: PaletteEntry| {
        let labels = palette_labels();
        if entry.is_back {
            current_namespace.set(None);
            input_text.set("/".to_string());
            let cmds = all_commands.get_untracked();
            let entries = build_palette_entries(&cmds, &None, "", &labels);
            palette_entries.set(entries);
            selected_index.set(0);
        } else if entry.is_namespace {
            current_namespace.set(Some(entry.label.clone()));
            input_text.set(format!("/{} ", entry.label));
            let cmds = all_commands.get_untracked();
            let ns = Some(entry.label.clone());
            let entries = build_palette_entries(&cmds, &ns, "", &labels);
            palette_entries.set(entries);
            selected_index.set(0);
        } else {
            input_text.set(entry.full_command.clone());
            show_palette.set(false);
            current_namespace.set(None);
        }
    };

    let on_keydown = {
        let send_message = send_message.clone();
        let select_palette_entry = select_palette_entry.clone();
        move |ev: web_sys::KeyboardEvent| {
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
                            select_palette_entry(entries[idx].clone());
                        }
                    }
                    "Escape" => {
                        ev.prevent_default();
                        if current_namespace.get_untracked().is_some() {
                            current_namespace.set(None);
                            input_text.set("/".to_string());
                            let labels = palette_labels();
                            let cmds = all_commands.get_untracked();
                            let new_entries =
                                build_palette_entries(&cmds, &None, "", &labels);
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
            // Guided mode — "/namespace" + Enter → drill into the children
            // instead of sending.
            if ev.key() == "Enter" && !ev.shift_key() {
                let text = input_text.get_untracked();
                let trimmed = text.trim();
                if trimmed.starts_with('/') && !trimmed.contains(' ') {
                    let maybe_ns = &trimmed[1..];
                    let cmds = all_commands.get_untracked();
                    if let Some(parent) = cmds.iter().find(|c| c.key == maybe_ns && c.is_namespace)
                    {
                        ev.prevent_default();
                        current_namespace.set(Some(parent.key.clone()));
                        input_text.set(format!("/{} ", parent.key));
                        let labels = palette_labels();
                        let ns = Some(parent.key.clone());
                        let entries = build_palette_entries(&cmds, &ns, "", &labels);
                        palette_entries.set(entries);
                        selected_index.set(0);
                        show_palette.set(true);
                        return;
                    }
                }
                ev.prevent_default();
                send_message();
            }
        }
    };

    let can_send = Memo::new(move |_| {
        (!input_text.get().trim().is_empty() || !attachments.get().is_empty()) && !is_sending.get()
    });

    let on_attach_click = move |_: web_sys::MouseEvent| {
        if let Some(input) = file_input_ref.get() {
            let el: &web_sys::HtmlInputElement = &input;
            el.click();
        }
    };
    let on_file_change = move |_ev: web_sys::Event| {
        let Some(input) = file_input_ref.get() else {
            return;
        };
        let el: &web_sys::HtmlInputElement = &input;
        if let Some(file_list) = el.files() {
            read_file_list_into(&file_list, attachments);
        }
        el.set_value("");
    };

    let on_abort = move |_: web_sys::MouseEvent| {
        if let Some(run_id) = chat.active_run_id.get() {
            let dash = dashboard;
            spawn_local(async move {
                let _ = ChatApi::abort(&dash, &run_id).await;
            });
        }
    };

    let select_for_callback = select_palette_entry.clone();
    let on_palette_select: Callback<PaletteEntry> =
        Callback::new(move |entry: PaletteEntry| select_for_callback(entry));

    view! {
        <div class="px-4 pb-5 pt-2">
            <div class="max-w-3xl mx-auto">
                <AttachmentPreviewBar attachments=attachments />

                <SlashPaletteView
                    show=show_palette
                    palette_entries=palette_entries
                    selected_index=selected_index
                    current_namespace=current_namespace
                    on_select=on_palette_select
                />

                // Live prompt-injection guard banner (G1). Server-side
                // remains the final authority; this is just a hint so
                // the user can rephrase before wasting a model call.
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

                // Project + model row — both pickers sit directly above
                // the composer so their dropdowns flip upward. The
                // trailing LayoutToggle opens / closes the right pane.
                <div class="aleph-project-row flex items-center gap-2 px-1 pb-1">
                    <ProjectMenu />
                    <crate::components::model_picker::ModelPicker />
                    <div class="ml-auto">
                        <crate::components::layout_toggle::LayoutToggle />
                    </div>
                </div>

                // Compact single-row composer — paperclip | textarea |
                // [clear] | [abort | send]. Textarea grows up to 140px;
                // items-end keeps the side buttons pinned to the bottom.
                <div class="aleph-composer flex items-end gap-2 px-3 py-1.5">
                    // Hidden file input. `accept` is a *hint* — the OS
                    // picker defaults to images, common video, plain
                    // text / markdown / pdf / json. Users can still
                    // switch to "All files" for niche types.
                    <input
                        type="file"
                        multiple=true
                        class="hidden"
                        accept="image/*,video/mp4,video/webm,video/quicktime,text/*,application/pdf,application/json,.md,.csv"
                        node_ref=file_input_ref
                        on:change=on_file_change
                    />

                    <button
                        class="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary
                               hover:bg-surface-sunken transition-colors flex-shrink-0"
                        title=move || t_string!(i18n, chat.attach).to_string()
                        on:click=on_attach_click
                    >
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5"
                             viewBox="0 0 20 20" fill="currentColor">
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

                    // Clear-draft ✕ — visible only when text exists.
                    // Wipes text + closes palette + exits namespace in
                    // one click. Attachments are left alone (own ✕).
                    <Show when=move || !input_text.get().trim().is_empty()>
                        <button
                            class="w-8 h-8 rounded-full text-text-tertiary hover:text-text-primary
                                   hover:bg-surface-sunken flex items-center justify-center
                                   transition-colors flex-shrink-0"
                            title=move || t_string!(i18n, chat.clear).to_string()
                            on:click=move |_| {
                                input_text.set(String::new());
                                show_palette.set(false);
                                current_namespace.set(None);
                            }
                        >
                            <svg xmlns="http://www.w3.org/2000/svg" class="w-3.5 h-3.5"
                                 viewBox="0 0 20 20" fill="currentColor">
                                <path d="M6.28 5.22a.75.75 0 0 0-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 1 0 1.06 1.06L10 11.06l3.72 3.72a.75.75 0 1 0 1.06-1.06L11.06 10l3.72-3.72a.75.75 0 0 0-1.06-1.06L10 8.94 6.28 5.22Z" />
                            </svg>
                        </button>
                    </Show>

                    <Show when=move || chat.active_run_id.get().is_some()>
                        <button
                            class="w-8 h-8 rounded-full bg-danger/15 text-danger flex items-center
                                   justify-center hover:bg-danger/25 transition-colors flex-shrink-0"
                            title=move || t_string!(i18n, chat.stop).to_string()
                            on:click=on_abort
                        >
                            <svg xmlns="http://www.w3.org/2000/svg" class="w-3.5 h-3.5"
                                 viewBox="0 0 20 20" fill="currentColor">
                                <rect x="4" y="4" width="12" height="12" rx="2" />
                            </svg>
                        </button>
                    </Show>

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
