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
mod voice;

use super::mention_palette::{update_mention_palette, MentionPaletteView};
use super::project_menu::ProjectMenu;
use super::state::{
    merge_draft, ChatPhase, ChatSendError, ChatSendErrorCode, ChatState, QueuedPrompt,
    TeamMemberView,
};
use super::TodoPanel;
use crate::api::chat::{ChatApi, ChatAttachment};
use crate::components::team_task_strip::TeamTaskStrip;
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::state::sessions::SessionMap;
use attachments::{read_file_list_into, AttachmentPreviewBar};
use leptos::prelude::*;
use leptos::task::spawn_local;
use palette::{
    build_palette_entries, doctor_command_info, expand_doctor_command, parse_command_info,
    CommandInfo, PaletteEntry, PaletteLabels, SlashPaletteView,
};
use shared_ui_logic::safety::{
    check_prompt_injection, prompt_guard_message, PromptInjectionVerdict,
};
use shared_ui_logic::state::{should_auto_drain_on_settle, should_flush_on_turn_boundary};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

/// Instruction seeded by the `f` hotkey (G1). The doctor tool only diagnoses
/// and applies safe mechanical repairs; the *reasoning* repair lives entirely
/// in this prompt (R7/R9), not in any deterministic branch. It tells the LLM
/// to read the structured findings and route each one: mechanical issues via
/// `doctor(fix=true)`, everything else through the tool named in the finding's
/// `fix_hint` (`self_config` / `vault_store` / …), then re-verify.
const DOCTOR_REPAIR_PROMPT: &str = "运行 doctor 工具诊断系统健康状况。\
对可机械修复的问题（repairable=true）调用 doctor(fix=true) 修复；\
对不可机械修复的问题，按其 fix_hint 用 self_config / vault_store 等对应工具修复；\
全部处理后再次运行 doctor 验证，并简要报告修复结果。";

/// Textarea + side buttons + palette popup + injection-guard banner.
/// Mounted by [`super::view::ChatView`] at the viewport bottom.
#[component]
#[must_use]
pub(super) fn InputArea() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let sessions = expect_context::<SessionMap>();
    let i18n = use_i18n();

    let input_text = RwSignal::new(String::new());
    let is_sending = RwSignal::new(false);
    // One-shot flag: set when the user presses Stop, consumed by the queue
    // drain Effect so an explicit interrupt suppresses exactly one auto-drain
    // (mirrors hermes-agent's `shouldAutoDrainOnSettle`).
    let user_interrupted = RwSignal::new(false);
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

    // @-mention palette state — team mode only.
    let show_mention = RwSignal::new(false);
    let mention_members: RwSignal<Vec<TeamMemberView>> = RwSignal::new(Vec::new());
    let mention_selected = RwSignal::new(0usize);
    // Byte offset of the active `@` in `input_text` (None when palette hidden).
    let mention_at: RwSignal<Option<usize>> = RwSignal::new(None);

    let file_input_ref = NodeRef::<leptos::html::Input>::new();
    let stack_ref = NodeRef::<leptos::html::Div>::new();
    let textarea_ref = NodeRef::<leptos::html::Textarea>::new();

    // Composer height → `--composer-clearance` on <html>, so the scroll
    // content + jump pill always clear the floating bar (queue bar /
    // attachments / multiline growth included). Mirrors the ResizeObserver
    // pattern in the galaxy canvas gl engine; the chat view is kept alive
    // by MainContent, so the leaked closure is one-per-app, not per-visit.
    Effect::new(move |_| {
        let Some(el) = stack_ref.get() else { return };
        let cb: Closure<dyn FnMut(js_sys::Array)> = Closure::new(move |entries: js_sys::Array| {
            if let Ok(entry) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() {
                let h = entry.content_rect().height();
                if let Some(root) = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.document_element())
                    .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = root
                        .style()
                        .set_property("--composer-clearance", &format!("{}px", h + 40.0));
                }
            }
        });
        if let Ok(observer) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
            observer.observe(&el);
        }
        cb.forget();
    });

    // Auto-grow the composer textarea to fit its content. We track the
    // `input_text` signal (not just the DOM `input` event) so every
    // programmatic rewrite — send-clear, retry refill, draft seed, slash/@
    // completion, clear button, queue replay — resizes too. Set height to
    // `auto` first so the box can shrink, then to `scroll_height`; CSS
    // `max-h-[140px]` caps it and `overflow-y-auto` scrolls beyond the cap.
    Effect::new(move |_| {
        let _ = input_text.get();
        if let Some(ta) = textarea_ref.get() {
            // Cast to HtmlElement so `.style()` resolves to the web-sys
            // inherent method, not Leptos's `ElementExt::style` (which is in
            // scope and would otherwise shadow it on HtmlTextAreaElement).
            let el: web_sys::HtmlElement = ta.unchecked_into();
            let _ = el.style().set_property("height", "auto");
            let _ = el
                .style()
                .set_property("height", &format!("{}px", el.scroll_height()));
        }
    });

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
        let raw = input_text.get_untracked().trim().to_string();
        // `/doctor` → seed the read-only detection prompt and route it through
        // the normal LLM pipeline, mirroring the `f`-hotkey repair flow. Done
        // before send so the literal slash command never reaches the gateway
        // fast path (which would run the tool deterministically, no LLM).
        let text = expand_doctor_command(&raw).unwrap_or(raw);
        let files = attachments.get_untracked();
        if text.is_empty() && files.is_empty() {
            return;
        }

        // `teams.chat.send` carries `{team_id, message}` only — there is no
        // attachment leg on the group transcript. The team branch below used to
        // run AFTER the tray was cleared, so a user who attached a spec and hit
        // Enter watched it vanish with no reply and no error. Refuse the send
        // instead, and leave the tray intact so nothing is lost.
        if chat.team_id.get_untracked().is_some() && !files.is_empty() {
            chat.set_send_error(ChatSendError::new(
                ChatSendErrorCode::Unsupported,
                t_string!(i18n, chat.team_attachments_unsupported).to_string(),
            ));
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

        // Team chat mode: route the requirement to the leader orchestration RPC
        // instead of the single-agent ChatApi::send. Early return skips the
        // single-agent path below.
        if let Some(team_id) = chat.team_id.get_untracked() {
            let dash = dashboard;
            let team_text = text.clone();
            // Optimistic busy state: the authoritative signal is the
            // `team.<id>.fanout` `started` event, but that is a round-trip away
            // and the first member reply can be a minute out. Without this the
            // group chat sits visually idle right after Enter, exactly the
            // stretch where the user most needs to see something happening.
            chat.phase.set(ChatPhase::Thinking);
            spawn_local(async move {
                if let Err(e) =
                    crate::api::team_chat::TeamChatApi::send(&dash, &team_id, &team_text).await
                {
                    // The fan-out never started, so no `settled` event is coming
                    // — drop back to idle or the composer hangs on "thinking".
                    chat.phase.set(ChatPhase::Idle);
                    chat.set_send_error(ChatSendError::classify(e));
                }
                is_sending.set(false);
            });
            return;
        }

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
        // The composer's exec tier. Carried on every send, because on the FIRST
        // send there is no session row to have written it to — and that is the
        // turn the picker was armed for. The server stamps it onto the session.
        let tier = chat.session_exec_tier.get();
        // The composer's usage mode. Unlike the tier it is carried ONLY on
        // the first send (no session row exists yet to have been patched):
        // once a session exists the store is authoritative, and re-carrying
        // the pill's cached value would out-rank and silently revert a
        // `session_set_mode` switch the model made mid-conversation.
        let mode = if session_key.is_some() {
            None
        } else {
            chat.session_mode.get()
        };
        // Capture the conversation active at *send* time. Binding the run to
        // this (rather than to whichever tab is focused when `run_accepted`
        // arrives) is what lets the user send in A, switch to B, and still have
        // A's reply stream into A (I1).
        let send_conv = sessions.active_conv();
        let dash = dashboard;
        spawn_local(async move {
            let sk = session_key.as_deref();
            let aid = agent_id.as_deref();
            let pr = project_root.as_deref();
            let mo = model_override.as_ref();
            match ChatApi::send(
                &dash,
                &text,
                sk,
                api_attachments,
                aid,
                pr,
                mo,
                tier.as_deref(),
                mode.as_deref(),
                false,
            )
            .await
            {
                Ok(resp) => {
                    if let Some(conv) = send_conv {
                        sessions.bind_run(&resp.run_id, conv, Some(&resp.session_key));
                    }
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

    // The question this conversation is parked on, if any.
    let pending_ask = Memo::new(move |_| {
        crate::state::notifications::pending_ask_for_session(
            &dashboard.pending_clarifications.get(),
            chat.session_key.get().as_deref(),
        )
        .cloned()
    });

    // Queue a follow-up while a run is active instead of sending it now.
    // Runs the same client-side injection guard as `send_message` so a
    // blocked prompt never enters the queue, then stashes the draft on
    // `ChatState` and clears the composer for the next line of input.
    let enqueue_message = move || {
        let raw = input_text.get_untracked().trim().to_string();
        let text = expand_doctor_command(&raw).unwrap_or(raw);
        let files = attachments.get_untracked();
        if text.is_empty() && files.is_empty() {
            return;
        }
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
        chat.enqueue_prompt(QueuedPrompt {
            text,
            attachments: files,
        });
        input_text.set(String::new());
        attachments.set(Vec::new());
        show_palette.set(false);
        current_namespace.set(None);
    };

    // Answer a pending `ask_user` with the current draft. Returns `true` when a
    // question was pending — the caller must then NOT fall through to the send /
    // queue path.
    //
    // This is the composer's half of the rule every channel already enforces in
    // `inbound_router::try_intercept_hitl`: while a clarification is pending,
    // whatever the user sends IS the answer. Without it the draft would be
    // queued behind a turn that can never reach its next boundary — the parked
    // tool would sit there until it timed out, and the user would have no way to
    // reach it except aborting the run.
    //
    // A stale question is the trap: core answers `resolved: false` when nobody
    // was unblocked (expired / superseded / run cancelled). The reply is then
    // still an unsent chat message — restore the draft and route it through the
    // normal path instead of swallowing it.
    let answer_pending_ask = move || -> bool {
        let Some(ask) = pending_ask.get_untracked() else {
            return false;
        };
        let reply = input_text.get_untracked().trim().to_string();
        // A question IS pending, so consume the keystroke either way — an empty
        // draft simply isn't an answer.
        if reply.is_empty() {
            return true;
        }
        input_text.set(String::new());
        let dash = dashboard;
        spawn_local(async move {
            match crate::api::ClarificationApi::resolve(&dash, &ask.session_key, &reply).await {
                Ok(true) => {
                    chat.push_user_message(&reply);
                    dash.pending_clarifications
                        .update(|l| l.retain(|p| p.session_key != ask.session_key));
                }
                Ok(false) => {
                    // The question is dead: drop it so it stops owning Enter,
                    // put the draft back, and send it as the ordinary message it
                    // turned out to be.
                    dash.pending_clarifications
                        .update(|l| l.retain(|p| p.session_key != ask.session_key));
                    input_text.set(reply);
                    if chat.active_run_id.get_untracked().is_some() {
                        enqueue_message();
                    } else {
                        send_message();
                    }
                }
                Err(e) => {
                    // The answer never left the client — keep the draft.
                    input_text.set(reply);
                    chat.set_send_error(ChatSendError::classify(e));
                }
            }
        });
        true
    };

    // Flush the entire prompt queue into the live run in one batch. Each prompt
    // rides the normal ChatApi::send path: while a run is active the gateway
    // Steer-injects it into the live session (picked up at the next turn
    // boundary); when idle the first send starts a fresh run and the rest steer
    // into it. Sends are awaited sequentially so the backend coalesces them in
    // order. The returned run_id of a steered send is intentionally ignored —
    // `active_run_id` is owned by the `run_accepted` event, and a steered send
    // emits none (execute.rs returns Ok before the RunAccepted emit).
    let flush_queue = move || {
        // Single-agent path: in team chat `active_run_id` is never set, so the
        // queue/flush is gated off entirely (team runs route via TeamChatApi).
        let batch = chat.drain_all_queued();
        if batch.is_empty() {
            return;
        }
        let session_key = chat.session_key.get_untracked();
        let agent_id = chat.agent_id.get_untracked();
        let project_root = chat.active_project_root.get_untracked();
        let model_override = chat.selected_model.get_untracked();
        let tier = chat.session_exec_tier.get_untracked();
        // First-send-only carriage — see the typed-send path above. Queue
        // flush always has a live session, so this is None in practice.
        let mode = if session_key.is_some() {
            None
        } else {
            chat.session_mode.get_untracked()
        };
        let dash = dashboard;
        spawn_local(async move {
            for entry in batch {
                chat.push_user_message(&entry.text);
                let api_attachments: Vec<ChatAttachment> = entry
                    .attachments
                    .into_iter()
                    .map(|f| ChatAttachment {
                        name: f.name,
                        mime_type: f.mime_type,
                        data_base64: f.data_base64,
                        size: f.size,
                    })
                    .collect();
                match ChatApi::send(
                    &dash,
                    &entry.text,
                    session_key.as_deref(),
                    api_attachments,
                    agent_id.as_deref(),
                    project_root.as_deref(),
                    model_override.as_ref(),
                    tier.as_deref(),
                    mode.as_deref(),
                    false,
                )
                .await
                {
                    Ok(resp) => chat.session_key.set(Some(resp.session_key)),
                    Err(e) => chat.set_send_error(ChatSendError::classify(e)),
                }
            }
        });
    };

    // Force-insert (B7): the user won't wait for the next turn boundary. Fold
    // the current draft into the queue, then interrupt the running task WITHOUT
    // setting `user_interrupted` — so the resulting busy→idle settle runs the
    // normal auto-drain (Task 3), flushing the whole queue as a fresh run. With
    // no active run it degrades to a normal send (B10).
    let force_insert = move || {
        if chat.active_run_id.get_untracked().is_none() {
            send_message();
            return;
        }
        enqueue_message(); // no-op when the draft is empty
        user_interrupted.set(false); // ensure the upcoming settle is NOT suppressed
        if let Some(run_id) = chat.active_run_id.get_untracked() {
            let dash = dashboard;
            spawn_local(async move {
                let _ = ChatApi::abort(&dash, &run_id).await;
            });
        }
    };

    // G4 retry plumbing — MessageBubble's Retry button bumps
    // `chat.retry_pulse`; we re-take the most recent user message and
    // route it through the normal send pipeline so prompt-guard +
    // idempotency + error classification apply identically.
    {
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

    // G1 `f`-hotkey repair flow — the global keydown listener bumps
    // `repair_pulse` (it can't reach the send pipeline directly). We seed the
    // doctor-and-fix instruction and route it through the same send pipeline as
    // a typed message, so prompt-guard + error classification apply identically.
    // While a run is active we queue it instead, matching the Enter-key
    // semantics. Mirrors the retry plumbing above.
    {
        Effect::new(move |prev_pulse: Option<u32>| {
            let pulse = chat.repair_pulse.get();
            if prev_pulse.is_some() && Some(pulse) != prev_pulse {
                input_text.set(DOCTOR_REPAIR_PROMPT.to_string());
                if chat.active_run_id.get_untracked().is_some() {
                    enqueue_message();
                } else {
                    send_message();
                }
            }
            pulse
        });
    }

    // Empty-state suggestion chips, the queued-ghost "click to edit", and the
    // composer's own ↑ retraction all drop a starter string on
    // `chat.draft_seed`; drain it into this local `input_text` here so none of
    // them needs a handle on the composer. Same shape as the retry plumbing
    // above. Writing the signal back to `None` inside the Effect that reads it
    // is safe — the re-run sees `None` and stops.
    //
    // The seed is MERGED with whatever is already typed, never substituted for
    // it: clicking a queued ghost to edit it used to wipe a half-written draft
    // with no way back. Chips only fire from the empty state, so for them the
    // merge is the identity.
    {
        Effect::new(move |_| {
            if let Some(seed) = chat.draft_seed.get() {
                input_text.set(merge_draft(&seed, &input_text.get_untracked()));
                chat.draft_seed.set(None);
            }
        });
    }

    // ↑ retraction — take the most recently queued prompt back into the
    // composer for editing (codex `edit_queued_message`). Restores the whole
    // prompt: the text merges above the current draft, the files rejoin the
    // pending list. Shared by the plain-↑ and Alt+↑ key paths below so both
    // behave identically. Returns whether anything was actually taken back, so
    // the caller only swallows the keystroke when it did something.
    //
    // Writes `input_text` directly rather than routing through `draft_seed`:
    // the seed is a ONE-SHOT slot drained by an Effect, so two ↑ presses inside
    // one frame would overwrite the first prompt before it ever reached the
    // textarea — silently eating a message the user asked to get back.
    let retract_latest_queued = move || -> bool {
        let Some(entry) = chat.retract_latest_queued() else {
            return false;
        };
        chat.add_pending_attachments(entry.attachments);
        input_text.set(merge_draft(&entry.text, &input_text.get_untracked()));
        true
    };

    // Queue auto-drain — when a run settles naturally (busy → idle), replay
    // the head of the queue through the normal send pipeline. An explicit
    // Stop sets `user_interrupted`, which suppresses exactly one drain so
    // cancelling a turn doesn't immediately re-fire the queued prompt. The
    // decision itself is the pure `should_auto_drain_on_settle` (host-tested
    // in `shared_ui_logic::state`); this Effect only owns the side effects.
    {
        Effect::new(move |prev_busy: Option<bool>| {
            let is_busy = chat.active_run_id.get().is_some();
            let was_busy = prev_busy.unwrap_or(false);
            let queue_len = chat.prompt_queue.get_untracked().len();
            if should_auto_drain_on_settle(
                was_busy,
                is_busy,
                queue_len,
                user_interrupted.get_untracked(),
            ) {
                flush_queue();
            }
            // Reset the one-shot interrupt flag once we've crossed the edge.
            if was_busy && !is_busy {
                user_interrupted.set(false);
            }
            is_busy
        });
    }

    // Turn-boundary flush — `events.rs` bumps `flush_pulse` when the agent
    // crosses into a new Think iteration with prompts still queued. Steer the
    // whole batch into the live run now (the pure decision is host-tested in
    // `shared_ui_logic::state::should_flush_on_turn_boundary`).
    {
        Effect::new(move |prev: Option<u32>| {
            let pulse = chat.flush_pulse.get();
            if prev.is_some() && Some(pulse) != prev {
                let is_busy = chat.active_run_id.get_untracked().is_some();
                let queue_len = chat.prompt_queue.get_untracked().len();
                if should_flush_on_turn_boundary(queue_len, is_busy) {
                    flush_queue();
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
                    if !cmds.iter().any(|c| c.key == "doctor") {
                        cmds.push(doctor_command_info());
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
                    if let Some(parent) = cmds.iter().find(|c| c.key == maybe_ns && c.is_namespace)
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
                let prefix = format!("{ns_key} ");
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
            let ns = Some(entry.label);
            let entries = build_palette_entries(&cmds, &ns, "", &labels);
            palette_entries.set(entries);
            selected_index.set(0);
        } else {
            input_text.set(entry.full_command);
            show_palette.set(false);
            current_namespace.set(None);
        }
    };

    // Commit a mention selection: splice the `@<query>` span with `token`.
    // Captures signals by copy — safe to call from multiple closures.
    let do_commit_mention = move |token: String| {
        let text = input_text.get_untracked();
        if let Some(at) = mention_at.get_untracked() {
            let before = text.get(..at).unwrap_or("");
            let query_start = at + 1;
            let query_end = text[query_start..]
                .find(|c: char| c.is_whitespace())
                .map(|rel| query_start + rel)
                .unwrap_or(text.len());
            let after = text.get(query_end..).unwrap_or("");
            input_text.set(format!("{before}{token}{after}"));
        } else {
            input_text.set(format!("{text}{token}"));
        }
        show_mention.set(false);
        mention_at.set(None);
        mention_members.set(Vec::new());
        mention_selected.set(0);
    };

    let on_keydown = {
        move |ev: web_sys::KeyboardEvent| {
            // @-mention palette takes priority over slash palette.
            if show_mention.get_untracked() {
                let count = mention_members.get_untracked().len() + 1; // +1 for "@all"
                match ev.key().as_str() {
                    "ArrowDown" => {
                        ev.prevent_default();
                        if count > 0 {
                            mention_selected.set((mention_selected.get_untracked() + 1) % count);
                        }
                    }
                    "ArrowUp" => {
                        ev.prevent_default();
                        if count > 0 {
                            let cur = mention_selected.get_untracked();
                            mention_selected.set(if cur == 0 { count - 1 } else { cur - 1 });
                        }
                    }
                    "Tab" | "Enter" => {
                        ev.prevent_default();
                        let idx = mention_selected.get_untracked();
                        let token = if idx == 0 {
                            "@all ".to_string()
                        } else {
                            mention_members
                                .get_untracked()
                                .get(idx - 1)
                                .map(|m| format!("@{} ", m.agent_id))
                                .unwrap_or_default()
                        };
                        do_commit_mention(token);
                    }
                    "Escape" => {
                        ev.prevent_default();
                        show_mention.set(false);
                        mention_at.set(None);
                    }
                    _ => {}
                }
                return;
            }

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
                            let new_entries = build_palette_entries(&cmds, &None, "", &labels);
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
            // ↑ = take the most recently queued prompt back for editing.
            //
            // The queued ghosts were mouse-only: a user who lined up three
            // follow-ups and then wanted the last one back had to leave the
            // keyboard. Both reference harnesses bind this (codex
            // `edit_queued_message` = Alt+↑ / Shift+←, pi `app.message.dequeue`
            // = Alt+↑), so Alt+↑ is the portable chord — and plain ↑ is offered
            // too, but ONLY on an empty draft, where the key has no caret work
            // to do. With text in the box plain ↑ must stay caret movement:
            // stealing it there would break editing a multi-line prompt.
            //
            // Ctrl/Meta are left alone (word-jump / document-start on the two
            // platforms). Palette and mention navigation already returned above,
            // so this cannot shadow them. `is_composing` keeps ↑ away from an
            // open IME candidate window — the draft reads as empty while a CJK
            // candidate is being picked, which is exactly when ↑ means "previous
            // candidate" and stealing it would be maddening.
            if ev.key() == "ArrowUp"
                && !ev.ctrl_key()
                && !ev.meta_key()
                && !ev.is_composing()
                && (ev.alt_key() || input_text.get_untracked().trim().is_empty())
            {
                // Only swallow the key when there was actually something to take
                // back; an empty queue leaves ↑ its default behaviour.
                if retract_latest_queued() {
                    ev.prevent_default();
                    return;
                }
            }
            // Esc while a run is active = force-insert: interrupt now and flush
            // the queue (+ the current draft) as a fresh run (B7). Palette/
            // mention Esc is handled in the branch above (it returns early), so
            // this only fires in the normal composing context.
            if ev.key() == "Escape" && chat.active_run_id.get_untracked().is_some() {
                ev.prevent_default();
                force_insert();
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
                // A pending question owns the send key — the turn is blocked on
                // the answer, so queueing the draft would strand it.
                if answer_pending_ask() {
                    return;
                }
                // While a run is active, Enter queues the follow-up instead
                // of sending (there is no live send slot until it settles).
                if chat.active_run_id.get_untracked().is_some() {
                    enqueue_message();
                } else {
                    send_message();
                }
            }
        }
    };

    let can_send = Memo::new(move |_| {
        (!input_text.get().trim().is_empty() || !attachments.get().is_empty()) && !is_sending.get()
    });

    // Draft is non-empty (text or attachments) — gates the queue button while
    // a run is active. Independent of `is_sending` (that's the brief outbound
    // HTTP window, not the whole run).
    let has_draft =
        Memo::new(move |_| !input_text.get().trim().is_empty() || !attachments.get().is_empty());

    // Force-insert is available while a run is active and there's *something*
    // to insert — queued ghosts or the current draft.
    let can_force = Memo::new(move |_| {
        chat.active_run_id.get().is_some()
            && (!chat.prompt_queue.get().is_empty()
                || !input_text.get().trim().is_empty()
                || !attachments.get().is_empty())
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
        // Suppress exactly one auto-drain: an explicit Stop must not let the
        // queue immediately re-fire its head (the "Stop does nothing" trap).
        user_interrupted.set(true);
        let Some(run_id) = chat.active_run_id.get() else {
            return;
        };
        // In team chat the id is a fan-out TREE, not an engine run: `chat.abort`
        // looks it up in `active_runs`, misses, and the group keeps talking.
        // `teams.chat.cancel` poisons the tree and walks its member runs.
        let is_team = chat.team_id.get_untracked().is_some();
        let dash = dashboard;
        spawn_local(async move {
            if is_team {
                let _ = crate::api::team_chat::TeamChatApi::cancel(&dash, &run_id).await;
                // The tree is gone, so no `settled` event will arrive to clear
                // the slot — release it here or the composer stays stuck on Stop.
                chat.active_run_id.set(None);
                chat.phase.set(ChatPhase::Idle);
            } else {
                let _ = ChatApi::abort(&dash, &run_id).await;
            }
        });
    };

    let select_for_callback = select_palette_entry;
    let on_palette_select: Callback<PaletteEntry> =
        Callback::new(move |entry: PaletteEntry| select_for_callback(entry));

    // @-mention selection callback — splices the token into `input_text`.
    // A second independent closure capturing the same Copy signals.
    let on_mention_select: Callback<String> = Callback::new(move |token: String| {
        let text = input_text.get_untracked();
        if let Some(at) = mention_at.get_untracked() {
            let before = text.get(..at).unwrap_or("");
            let query_start = at + 1;
            let query_end = text[query_start..]
                .find(|c: char| c.is_whitespace())
                .map(|rel| query_start + rel)
                .unwrap_or(text.len());
            let after = text.get(query_end..).unwrap_or("");
            input_text.set(format!("{before}{token}{after}"));
        } else {
            input_text.set(format!("{text}{token}"));
        }
        show_mention.set(false);
        mention_at.set(None);
        mention_members.set(Vec::new());
        mention_selected.set(0);
    });

    view! {
        <div class="absolute inset-x-0 bottom-0 z-10 px-4 pb-4 pt-2 pointer-events-none">
            <div class="w-full min-w-0 max-w-5xl mx-auto pointer-events-auto" node_ref=stack_ref>
                // Single-chat sticky Todo panel — top of the bottom input
                // stack (below the message flow, above the input box).
                // Hidden when no active plan. Living inside `stack_ref` lets
                // the existing ResizeObserver reserve `--composer-clearance`
                // for its height, so messages never hide behind it.
                <TodoPanel />
                <AttachmentPreviewBar attachments=attachments />

                // Team chat: most-salient task pill, above the input box.
                <TeamTaskStrip />

                // Floating-overlay anchor. The palettes below are positioned
                // `absolute bottom-full` against this `relative` wrapper, so
                // they float above the input cluster instead of sitting in
                // flow. Critical: an in-flow palette would grow the
                // ResizeObserver-tracked `stack_ref`, inflate
                // `--composer-clearance`, and push chat content up.
                <div class="relative">
                <SlashPaletteView
                    show=show_palette
                    palette_entries=palette_entries
                    selected_index=selected_index
                    current_namespace=current_namespace
                    on_select=on_palette_select
                />

                // @-mention palette — visible only in team mode when the user
                // types `@` followed by a valid mention token prefix.
                <Show when=move || chat.team_id.get().is_some()>
                    <MentionPaletteView
                        show=show_mention
                        members=mention_members
                        selected_index=mention_selected
                        on_select=on_mention_select
                    />
                </Show>

                // Send-error banner (G2) — the last outbound send failed or was
                // refused before it left. It lives in the composer stack, not in
                // the transcript: every writer of `send_error` is a composer
                // path, and a banner parked above the scrollback is invisible in
                // any conversation long enough to scroll (the team-attachment
                // refusal landed ~700px off-screen).
                <SendErrorBanner />

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

                // Composer card — two zones: full-width auto-grow textarea
                // on top, a toolbar row below (attach + voice on the left,
                // clear / queue / abort / send on the right). The textarea
                // grows up to 140px then scrolls internally.
                <div class="aleph-composer flex flex-col gap-1.5 px-3 py-2">
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

                    <textarea
                        class="w-full resize-none overflow-y-auto bg-transparent px-1 py-[6px] text-sm leading-snug
                               text-text-primary placeholder:text-text-tertiary
                               focus:outline-none min-h-[32px] max-h-[140px]"
                        placeholder=move || t_string!(i18n, chat.send_placeholder).to_string()
                        rows=1
                        node_ref=textarea_ref
                        prop:value=move || input_text.get()
                        on:input=move |ev| {
                            let val = event_target_value(&ev);
                            input_text.set(val.clone());
                            update_palette(&val);
                            // Read the caret from the underlying textarea DOM node.
                            // `selection_start` is a UTF-16 code-unit index; convert it
                            // to a UTF-8 byte offset so it lands on a char boundary (a
                            // direct cast would slice mid-codepoint after multibyte text).
                            let sel = ev
                                .target()
                                .and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
                                .and_then(|ta| ta.selection_start().ok().flatten());
                            let caret = match sel {
                                Some(u16_off) => {
                                    let mut units = 0u32;
                                    val.char_indices()
                                        .find_map(|(b, c)| {
                                            if units >= u16_off {
                                                Some(b)
                                            } else {
                                                units += c.len_utf16() as u32;
                                                None
                                            }
                                        })
                                        .unwrap_or(val.len())
                                }
                                None => val.len(),
                            };
                            let at = update_mention_palette(
                                &val,
                                caret,
                                chat.team_id.get_untracked(),
                                &chat.team_members.get_untracked(),
                                show_mention,
                                mention_members,
                                mention_selected,
                            );
                            mention_at.set(at);
                        }
                        on:keydown=on_keydown
                    />

                    // Toolbar row — left: attach + voice + project/model/gauge;
                    // right cluster: export + conditional clear / queue / abort / send.
                    // (The old standalone project-row was folded into this
                    // row so its controls sit level with the attach paperclip.)
                    <div class="flex items-center gap-2 flex-wrap min-w-0">
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

                        // Voice loop — record → STT → send → spoken reply.
                        <voice::VoiceInputButton
                            disabled=Signal::derive(move || is_sending.get())
                        />

                        // Migrated from the old project row — now level
                        // with the attach paperclip. Dropdowns still flip upward.
                        <ProjectMenu />
                        <crate::components::model_picker::ModelPicker />
                        // Per-session usage mode (chat / work / code) + tool
                        // execution tier (Ask / Auto / Full). Hidden in team
                        // chat: the team send path (`TeamChatApi::send`) carries
                        // neither, and with `session_key` cleared a pick could
                        // not persist either — an override dot that does
                        // nothing would just mislead.
                        <Show when=move || chat.team_id.get().is_none()>
                            <crate::views::chat::mode_picker::ModePicker />
                            <crate::views::chat::exec_tier_picker::ExecTierPicker />
                        </Show>
                        // Live context-window gauge (self-hides until first usage).
                        <super::context_gauge::ContextGauge />

                        <div class="ml-auto flex items-center gap-2 flex-wrap min-w-0">
                            // Export conversation → Markdown (far right of the
                            // cluster). Only once the thread has content.
                            <Show when=move || !chat.messages.get().is_empty()>
                                <button
                                    class="p-1.5 rounded-lg text-text-tertiary hover:text-text-primary
                                           hover:bg-surface-sunken transition-colors flex-shrink-0"
                                    title="导出对话为 Markdown"
                                    on:click=move |_| {
                                        let msgs = chat.messages.get_untracked();
                                        super::transcript::download_transcript(&msgs);
                                    }
                                >
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4"
                                         viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                                        <polyline points="7 10 12 15 17 10" />
                                        <line x1="12" y1="15" x2="12" y2="3" />
                                    </svg>
                                </button>
                            </Show>
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
                                        show_mention.set(false);
                                        mention_at.set(None);
                                    }
                                >
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-3.5 h-3.5"
                                         viewBox="0 0 20 20" fill="currentColor">
                                        <path d="M6.28 5.22a.75.75 0 0 0-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 1 0 1.06 1.06L10 11.06l3.72 3.72a.75.75 0 1 0 1.06-1.06L11.06 10l3.72-3.72a.75.75 0 0 0-1.06-1.06L10 8.94 6.28 5.22Z" />
                                    </svg>
                                </button>
                            </Show>

                            // Queue button — only while a run is active. Lets the user
                            // line up a follow-up that auto-sends when the turn settles.
                            // Withdrawn while a question is pending: the turn cannot
                            // reach another boundary until it is answered, so a queued
                            // draft would just sit there behind the parked tool.
                            <Show when=move || chat.active_run_id.get().is_some()
                                               && pending_ask.get().is_none()>
                                <button
                                    class="w-8 h-8 rounded-full bg-surface-sunken text-text-secondary
                                           flex items-center justify-center hover:bg-surface-raised
                                           hover:text-text-primary disabled:opacity-35
                                           disabled:cursor-not-allowed transition-colors flex-shrink-0"
                                    title=move || t_string!(i18n, chat.queue).to_string()
                                    disabled=move || !has_draft.get()
                                    on:click=move |_| enqueue_message()
                                >
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4"
                                         viewBox="0 0 20 20" fill="currentColor">
                                        <path fill-rule="evenodd"
                                              d="M10 3a.75.75 0 0 1 .75.75v5.5h5.5a.75.75 0 0 1 0 1.5h-5.5v5.5a.75.75 0 0 1-1.5 0v-5.5h-5.5a.75.75 0 0 1 0-1.5h5.5v-5.5A.75.75 0 0 1 10 3Z"
                                              clip-rule="evenodd" />
                                    </svg>
                                </button>
                            </Show>

                            // Force-insert ⚡ — interrupt now and flush the queue
                            // (+ draft) immediately instead of waiting for the
                            // next turn boundary. Mirrors Esc.
                            <Show when=move || can_force.get()>
                                <button
                                    class="w-8 h-8 rounded-full bg-primary/15 text-primary flex items-center
                                           justify-center hover:bg-primary/25 transition-colors flex-shrink-0"
                                    title=move || t_string!(i18n, chat.force_insert).to_string()
                                    on:click=move |_| force_insert()
                                >
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4"
                                         viewBox="0 0 20 20" fill="currentColor">
                                        <path d="M11 3 4 11h4l-1 6 7-8h-4l1-6Z" />
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
                </div>  // /relative floating-overlay anchor
            </div>
        </div>
    }
}

/// Inline banner for the most recent `ChatSendError`. Empty when none.
#[component]
fn SendErrorBanner() -> impl IntoView {
    let i18n = use_i18n();
    let chat = expect_context::<ChatState>();
    view! {
        <Show when=move || chat.send_error.get().is_some()>
            {move || {
                let err = chat.send_error.get();
                err.map(|e| {
                    let is_warning = matches!(e.code, ChatSendErrorCode::PromptReview);
                    let class_str = if is_warning {
                        "mx-1 mb-1 px-3 py-2 rounded-lg border text-sm bg-warning-subtle border-warning/30 text-warning"
                    } else {
                        "mx-1 mb-1 px-3 py-2 rounded-lg border text-sm bg-danger-subtle border-danger/30 text-danger"
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
                                    title=move || t_string!(i18n, chat.dismiss).to_string()
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
