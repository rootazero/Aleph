//! Phone composer with a runtime-append queue. While a run is active, Enter (or
//! the ＋ button) stages the draft as a ghost bubble instead of sending it
//! blindly; the batch auto-flushes at the next turn boundary (Steer) or when the
//! run settles naturally, and ⚡ force-inserts immediately by interrupting the
//! run. Faithful subset of the wide composer's queue flow, reduced to phone's
//! surface: no model override, no slash-commands / @-mentions / voice (server
//! remains the prompt-injection authority). Ghost bubbles render in the shared
//! `MessageList`; this file only feeds the shared `ChatState` queue and the
//! existing `ChatApi` send/abort.
//!
//! Three things the phone shares rather than reimplements:
//!   * **Attachments** — the tray is `ChatState.pending_attachments` (already
//!     shared: a recalled ghost puts its files back there) and both the reader
//!     and the chip strip come from the wide composer's `attachments` module.
//!   * **Exec tier / session mode** — `ExecTierPicker` / `ModePicker` own the
//!     whole flow (they write `ChatState`, and patch a live session themselves);
//!     the composer only has to *carry* their values on the send. It used to
//!     hard-code `None` for both, so a phone user could not arm a tier or a mode
//!     for the very turn the pickers exist for — the first one.
//!   * The carriage rule is the wide composer's, verbatim: the **tier rides
//!     every send** (a brand-new conversation has no session row to have been
//!     patched, and that is the turn the picker was armed for), the **mode rides
//!     only the first** (once a session exists the store is authoritative, and
//!     re-carrying the pill's cached value would out-rank and silently revert a
//!     `session_set_mode` the model made mid-conversation).

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::chat::ChatApi;
use crate::context::DashboardState;
use crate::views::chat::composer::attachments::{read_file_list_into, AttachmentPreviewBar};
use crate::views::chat::exec_tier_picker::ExecTierPicker;
use crate::views::chat::mode_picker::ModePicker;
use crate::views::chat::state::{ChatPhase, ChatSendError, QueuedPrompt};
use crate::views::chat::ChatState;
use shared_ui_logic::state::{
    session_dials_for_send, should_auto_drain_on_settle, should_flush_on_turn_boundary,
};

#[component]
#[must_use]
pub fn PhoneComposer() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();

    // Shared with the wide composer via ChatState: the shared `MessageList`
    // renders the starter chips and the queued-ghost bubbles on phone too, and
    // both restore their text by writing this signal. While the draft was a
    // composer-local signal, a tap on a ghost took the prompt out of the queue
    // and dropped it on the floor here.
    let input_text = chat.draft;
    // Same tray the wide composer and the ghost-recall path write to — phone
    // just grew the paperclip that fills it.
    let attachments = chat.pending_attachments;
    let file_input_ref = NodeRef::<leptos::html::Input>::new();
    let is_sending = RwSignal::new(false);
    // Set by Stop to suppress exactly one auto-drain (B6 — Stop keeps ghosts).
    // On ChatState, not local: the queue it gates is per-conversation, so the
    // suppression has to swap with the conversation the way the queue does.
    let user_interrupted = chat.stop_suppresses_next_drain;

    // True while a run is in flight → the composer shows Queue/Force/Stop.
    let running = move || {
        matches!(chat.phase.get(), ChatPhase::Thinking | ChatPhase::Streaming)
            || chat.active_run_id.get().is_some()
    };

    // A draft is text OR staged files. Attachments-only used to be
    // unsendable here: the guard was text-only, so a photo with no caption
    // simply did nothing when Send was tapped.
    let has_draft =
        Memo::new(move |_| !input_text.get().trim().is_empty() || !attachments.get().is_empty());

    // Force-insert is available while a run is active and there's something to
    // insert — queued ghosts or the current draft.
    let can_force = Memo::new(move |_| {
        chat.active_run_id.get().is_some()
            && (!chat.prompt_queue.get().is_empty() || has_draft.get())
    });

    // Idle send: start a fresh run.
    let send = move || {
        if is_sending.get_untracked() {
            return;
        }
        let text = input_text.get_untracked().trim().to_string();
        // The tray is shared, so it could already hold files even before the
        // paperclip existed (a recalled ghost restores its attachments into it).
        // This path used to hard-code `Vec::new()` and never clear the tray, so
        // those files were dropped from the send AND left behind to reattach
        // themselves to whatever was queued next. `enqueue` below always got
        // this right; only the idle send was asymmetric.
        let files = attachments.get_untracked();
        if text.is_empty() && files.is_empty() {
            return;
        }
        is_sending.set(true);
        input_text.set(String::new());
        attachments.set(Vec::new());
        chat.push_user_message(&text);

        // `iter().cloned()`, not `into_iter()`: `files` has to survive the send
        // so a failure can hand it back (see the `Err` arm below).
        let api_attachments: Vec<crate::api::chat::ChatAttachment> = files
            .iter()
            .cloned()
            .map(|f| crate::api::chat::ChatAttachment {
                name: f.name,
                mime_type: f.mime_type,
                data_base64: f.data_base64,
                size: f.size,
            })
            .collect();

        let session_key = chat.session_key.get_untracked();
        let agent_id = chat.agent_id.get_untracked();
        // Room conversations are entered from the wide Projects surface, but
        // `ChatState` is shared with the phone composer at any viewport
        // width, so the same room-vs-picker exclusivity applies here too.
        let room_project_id = chat.room_project_id.get_untracked();
        let project_root = if room_project_id.is_some() {
            None
        } else {
            chat.active_project_root.get_untracked()
        };
        // Tier every send, mode only on the first — one rule, one place.
        let (tier, mode) = session_dials_for_send(
            session_key.is_some(),
            chat.session_exec_tier.get_untracked(),
            chat.session_mode.get_untracked(),
        );
        let dash = dashboard;
        spawn_local(async move {
            let res = ChatApi::send(
                &dash,
                &text,
                session_key.as_deref(),
                api_attachments,
                agent_id.as_deref(),
                project_root.as_deref(),
                room_project_id.as_deref(),
                // No per-turn model pill on phone: the agent's model governs.
                None,
                tier.as_deref(),
                mode.as_deref(),
                false,
            )
            .await;
            match res {
                Ok(resp) => chat.session_key.set(Some(resp.session_key)),
                Err(e) => {
                    chat.set_send_error(ChatSendError::classify(e));
                    // Nothing reached the server, so the payload comes back.
                    // Retry rebuilds the text from the transcript but is
                    // text-only, so without this the files are lost silently.
                    // Same fix, same reasoning as the desktop composer — the
                    // two paths have to agree on what a failed send costs.
                    chat.seed_draft(String::new(), files);
                }
            }
            is_sending.set(false);
        });
    };

    // The question this conversation is parked on, if any. The card itself is
    // rendered by the shared `MessageList`; the composer only has to make sure
    // Enter reaches it instead of the queue.
    let pending_ask = Memo::new(move |_| {
        crate::state::notifications::pending_ask_for_session(
            &dashboard.pending_clarifications.get(),
            chat.session_key.get().as_deref(),
        )
        .cloned()
    });

    // Queue a follow-up while a run is active → it becomes a ghost bubble.
    // No client-side prompt-injection guard (server is the authority).
    let enqueue = move || {
        let text = input_text.get_untracked().trim().to_string();
        // The tray lives on ChatState and the shared surfaces can fill it — a
        // recalled prompt puts its files back there. Phone has no paperclip, but
        // it must still carry what is staged rather than drop it on the floor.
        let files = attachments.get_untracked();
        if text.is_empty() && files.is_empty() {
            return;
        }
        chat.enqueue_prompt(QueuedPrompt {
            text,
            attachments: files,
        });
        input_text.set(String::new());
        attachments.set(Vec::new());
    };

    // Paperclip → the platform file picker. On iOS Safari a bare file input
    // already offers Photo Library / Take Photo / Browse, so there is nothing
    // for the panel to build: `accept` is left open because Aleph's media
    // pipeline takes documents as well as images.
    let on_attach_click = move |_: web_sys::MouseEvent| {
        if let Some(input) = file_input_ref.get() {
            input.click();
        }
    };
    let on_files_picked = move |_: web_sys::Event| {
        let Some(input) = file_input_ref.get() else {
            return;
        };
        if let Some(file_list) = input.files() {
            read_file_list_into(&file_list, attachments);
        }
        // Clear the input's own value, or picking the *same* file twice in a
        // row fires no `change` event and the second pick silently does nothing.
        input.set_value("");
    };

    // Answer a pending `ask_user` with the current draft. Returns `true` when a
    // question was pending — the caller must then NOT fall through to send/queue.
    // Same rule every channel enforces in `inbound_router::try_intercept_hitl`:
    // while a clarification is pending, whatever the user sends IS the answer.
    // Queueing it instead would strand the parked tool until its timeout — the
    // turn cannot reach another boundary to drain the queue at.
    //
    // `resolved: false` = nobody was unblocked (expired / superseded / run
    // cancelled). The reply is then an ordinary chat message, not an answer:
    // restore the draft and send it rather than swallowing the keystroke.
    let answer_pending_ask = move || -> bool {
        let Some(ask) = pending_ask.get_untracked() else {
            return false;
        };
        let reply = input_text.get_untracked().trim().to_string();
        if reply.is_empty() {
            return true;
        }
        input_text.set(String::new());
        let dash = dashboard;
        spawn_local(async move {
            match crate::api::ClarificationApi::resolve(&dash, &ask.session_key, &reply)
                .await
                .map(|o| (o.accepted, o.is_finished()))
            {
                Ok((true, finished)) => {
                    chat.push_user_message(&reply);
                    // Keep the card while questions remain — see the wide
                    // composer's twin arm for why dropping it here strands the
                    // rest of the walk.
                    if finished {
                        dash.pending_clarifications
                            .update(|l| l.retain(|p| p.session_key != ask.session_key));
                    }
                }
                Ok((false, _)) => {
                    dash.pending_clarifications
                        .update(|l| l.retain(|p| p.session_key != ask.session_key));
                    input_text.set(reply);
                    // Probe a *component-owned* signal, not `chat`: `send()` and
                    // `enqueue()` below both read several of these, and `chat`
                    // is root-owned, so it would answer "alive" long after this
                    // composer was disposed (see `crate::disposed_reads`).
                    if is_sending.try_get_untracked().is_none() {
                        return;
                    }
                    if chat.active_run_id.try_get_untracked().flatten().is_some() {
                        enqueue();
                    } else {
                        send();
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

    // Flush the entire queue. While a run is active each send Steer-injects into
    // the live session (picked up at the next turn boundary); when idle the first
    // send starts a fresh run and the rest steer into it. Sends are awaited
    // sequentially so the backend coalesces them in order. The returned run_id of
    // a steered send is intentionally ignored — `active_run_id` is owned by the
    // `run_accepted` event, and a steered send emits none.
    let flush_queue = move || {
        let batch = chat.drain_all_queued();
        if batch.is_empty() {
            return;
        }
        let session_key = chat.session_key.get_untracked();
        let agent_id = chat.agent_id.get_untracked();
        let room_project_id = chat.room_project_id.get_untracked();
        let project_root = if room_project_id.is_some() {
            None
        } else {
            chat.active_project_root.get_untracked()
        };
        // Same rule as the typed send. A queue flush almost always has a live
        // session, so `mode` is `None` in practice — but not when the very
        // first thing a user does is queue two prompts.
        let (tier, mode) = session_dials_for_send(
            session_key.is_some(),
            chat.session_exec_tier.get_untracked(),
            chat.session_mode.get_untracked(),
        );
        let dash = dashboard;
        spawn_local(async move {
            let mut pending = batch.into_iter();
            while let Some(entry) = pending.next() {
                let api_attachments: Vec<crate::api::chat::ChatAttachment> = entry
                    .attachments
                    .iter()
                    .cloned()
                    .map(|f| crate::api::chat::ChatAttachment {
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
                    room_project_id.as_deref(),
                    None,
                    tier.as_deref(),
                    mode.as_deref(),
                    false,
                )
                .await
                {
                    Ok(resp) => {
                        // Only once it is really sent — the bubble used to go up
                        // first, so a failed send left the transcript claiming a
                        // prompt had been delivered that the queue had already
                        // thrown away.
                        chat.push_user_message(&entry.text);
                        chat.session_key.set(Some(resp.session_key));
                    }
                    Err(e) => {
                        chat.set_send_error(ChatSendError::classify(e));
                        let mut unsent = vec![entry];
                        unsent.extend(pending);
                        chat.requeue_front(unsent);
                        break;
                    }
                }
            }
        });
    };

    // Force-insert (B7): fold the current draft into the queue, then interrupt the
    // run WITHOUT setting `user_interrupted` — so the resulting busy→idle settle
    // runs the normal auto-drain, flushing the whole queue as a fresh run. With no
    // active run it degrades to a plain send (B8).
    let force_insert = move || {
        if chat.active_run_id.get_untracked().is_none() {
            send();
            return;
        }
        enqueue(); // no-op when the draft is empty
        user_interrupted.set(false); // ensure the upcoming settle is NOT suppressed
        if let Some(run_id) = chat.active_run_id.get_untracked() {
            let is_team = chat.team_id.get_untracked().is_some();
            let dash = dashboard;
            spawn_local(async move {
                if is_team {
                    let _ = crate::api::team_chat::TeamChatApi::cancel(&dash, &run_id).await;
                    // The busy->idle edge released here is what drains the queue
                    // this just folded the draft into.
                    chat.active_run_id.set(None);
                    chat.phase.set(ChatPhase::Idle);
                } else {
                    // Not session-scoped on purpose: force-insert is "run this
                    // now", not "drop this work" — purging the lane would throw
                    // away the prompts it just folded the draft into.
                    let _ = ChatApi::abort(&dash, &run_id, None).await;
                }
            });
        }
    };

    // Stop (B6): abort and suppress exactly one auto-drain so the queued ghosts
    // aren't immediately re-fired (the "Stop does nothing" trap).
    let stop = move || {
        user_interrupted.set(true);
        let Some(run_id) = chat.active_run_id.get_untracked() else {
            return;
        };
        let session_key = chat.session_key.get_untracked();
        // In team chat the id is a fan-out TREE, not an engine run: `chat.abort`
        // looks it up in `active_runs`, misses, and the group keeps talking with
        // the button stuck on Stop. Same split the wide composer makes.
        let is_team = chat.team_id.get_untracked().is_some();
        let dash = dashboard;
        spawn_local(async move {
            if is_team {
                let _ = crate::api::team_chat::TeamChatApi::cancel(&dash, &run_id).await;
                // No `settled` event follows a poisoned tree — release the slot
                // here or the composer stays stuck on Stop.
                chat.active_run_id.set(None);
                chat.phase.set(ChatPhase::Idle);
            } else {
                // Stop must reach the session's server-side backlog too, or the
                // freed slot lets the lane fire the queued messages one run at a
                // time — exactly what the user just refused.
                let _ = ChatApi::abort(&dash, &run_id, session_key.as_deref()).await;
            }
        });
    };

    // Queue auto-drain — when a run settles naturally (busy→idle), flush the
    // queue. An explicit Stop set `user_interrupted`, suppressing exactly one
    // drain. The decision is the pure `should_auto_drain_on_settle`.
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
        if was_busy && !is_busy {
            user_interrupted.set(false);
        }
        is_busy
    });

    // Turn-boundary flush — `events.rs` bumps `flush_pulse` when the agent crosses
    // into a new Think iteration with prompts still queued. Steer the whole batch
    // into the live run now (pure decision: `should_flush_on_turn_boundary`).
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

    // Enter sends (idle) or queues (running); Shift+Enter inserts a newline.
    // A pending question outranks both — the turn is blocked on the answer.
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            if answer_pending_ask() {
                return;
            }
            if running() {
                enqueue();
            } else {
                send();
            }
        }
    };

    view! {
        <div style="flex:none; display:flex; flex-direction:column; gap:6px; padding:6px 12px calc(8px + env(safe-area-inset-bottom)); border-top:1px solid var(--color-border-subtle); background:var(--color-surface);">
            // Staged files, above everything — same chip strip the wide
            // composer renders, self-hiding while the tray is empty.
            <AttachmentPreviewBar attachments=attachments />

            // Toolbar row: attach + the two session pills. The pickers are the
            // desktop components verbatim, and their popovers are sized for a
            // desktop column (`w-80`, anchored `left-0` to the pill) — at 390 px
            // the tier popover started 101 px in and ran 31 px off the right
            // edge, clipping every description. `phone-composer-tools`
            // (styles/ios.css) re-anchors them to this row and lets them span
            // it; see that block for why the row, not the pill, is the anchor.
            //
            // No `style=` here on purpose: the row's own layout is in that same
            // stylesheet block so the landscape rule can fold it away. An inline
            // `display:flex` outranks a media query, so the fold-away would load,
            // match, and do nothing at all.
            <div class="phone-composer-tools">
                <input
                    type="file"
                    multiple
                    node_ref=file_input_ref
                    on:change=on_files_picked
                    style="display:none;"
                />
                <button
                    on:click=on_attach_click
                    style="flex:none; display:flex; align-items:center; justify-content:center; width:30px; height:30px; padding:0; border:0; border-radius:8px; background:none; color:var(--color-text-tertiary); cursor:pointer;"
                    aria-label="Attach"
                >
                    <svg width="18" height="18" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M15.621 4.379a3 3 0 0 0-4.242 0l-7 7a3 3 0 0 0 4.241 4.243h.001l.497-.5a.75.75 0 0 1 1.064 1.057l-.498.501-.002.002a4.5 4.5 0 0 1-6.364-6.364l7-7a4.5 4.5 0 0 1 6.368 6.36l-3.455 3.553A2.625 2.625 0 1 1 9.52 9.52l3.45-3.451a.75.75 0 1 1 1.061 1.06l-3.45 3.451a1.125 1.125 0 0 0 1.587 1.595l3.454-3.553a3 3 0 0 0 0-4.242Z" clip-rule="evenodd"></path></svg>
                </button>
                // Hidden in team chat for the same reason the wide composer
                // hides them: `TeamChatApi::send` carries neither value, and
                // with `session_key` cleared a pick could not persist either —
                // an override dot that does nothing would only mislead.
                <Show when=move || chat.team_id.get().is_none()>
                    <ModePicker />
                    <ExecTierPicker />
                </Show>
            </div>

            <div style="display:flex; align-items:flex-end; gap:8px;">
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
                    <div style="flex:none; display:flex; align-items:flex-end; gap:8px;">
                        // Withdrawn while a question is pending: the turn cannot
                        // reach another boundary until it is answered, so a
                        // queued draft would sit behind the parked tool.
                        <Show when=move || has_draft.get() && pending_ask.get().is_none()>
                            <button
                                on:click=move |_| enqueue()
                                style="flex:none; width:38px; height:38px; border:0; border-radius:9999px; background:var(--color-surface-raised); color:var(--color-text-secondary); cursor:pointer; display:flex; align-items:center; justify-content:center;"
                                aria-label="Queue"
                            ><svg width="18" height="18" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M10 3a.75.75 0 0 1 .75.75v5.5h5.5a.75.75 0 0 1 0 1.5h-5.5v5.5a.75.75 0 0 1-1.5 0v-5.5h-5.5a.75.75 0 0 1 0-1.5h5.5v-5.5A.75.75 0 0 1 10 3Z" clip-rule="evenodd"></path></svg></button>
                        </Show>
                        <Show when=move || can_force.get()>
                            <button
                                on:click=move |_| force_insert()
                                style="flex:none; width:38px; height:38px; border:0; border-radius:9999px; background:color-mix(in oklch, var(--color-primary) 15%, transparent); color:var(--color-primary); cursor:pointer; display:flex; align-items:center; justify-content:center;"
                                aria-label="Force insert"
                            ><svg width="18" height="18" viewBox="0 0 20 20" fill="currentColor"><path d="M11 3 4 11h4l-1 6 7-8h-4l1-6Z"></path></svg></button>
                        </Show>
                        <button
                            on:click=move |_| stop()
                            style="flex:none; width:38px; height:38px; border:0; border-radius:9999px; background:var(--color-danger); color:white; cursor:pointer; display:flex; align-items:center; justify-content:center;"
                            aria-label="Stop"
                        ><svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"></rect></svg></button>
                    </div>
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
        </div>
    }
}

#[cfg(test)]
mod tests {
    /// Only the production half of this file. Splitting on the test-module
    /// attribute keeps the RED-proof fixtures below out of every count — an
    /// unscoped `include_str!` would count them as real send sites.
    fn production_half(src: &str) -> &str {
        src.split("#[cfg").next().unwrap_or(src)
    }

    /// Every `ChatApi::send` in this composer must resolve its tier/mode
    /// through the shared rule.
    ///
    /// Count equality (not mere presence) is the point: the failure this pins
    /// is a *send site* that carries neither dial, which is precisely what both
    /// of this file's send paths did — they passed a literal `None, None` and
    /// said so in a comment. A third send path added later that forgets the
    /// dials makes the counts diverge.
    fn every_send_resolves_the_dials(src: &str) -> bool {
        let src = production_half(src);
        let sends = src.matches("ChatApi::send(").count();
        sends > 0 && src.matches("session_dials_for_send(").count() == sends
    }

    /// The idle send must take the shared attachment tray and clear it. The
    /// queue path always did; the idle path hard-coded an empty vec and left
    /// the tray full, so files staged by a recalled ghost were dropped from the
    /// send *and* stuck to whatever was queued next.
    fn idle_send_body(src: &str) -> Option<&str> {
        let src = production_half(src);
        let (_, after) = src.split_once("let send = move || {")?;
        let (body, _) = after.split_once("\n    // The question this conversation")?;
        Some(body)
    }

    fn idle_send_drains_the_tray(src: &str) -> bool {
        let Some(body) = idle_send_body(src) else {
            return false;
        };
        body.contains("attachments.get_untracked()") && body.contains("attachments.set(Vec::new())")
    }

    /// …and a send that *fails* must hand the tray back.
    ///
    /// The tray is drained before the request goes out, and the Retry button
    /// rebuilds only the text (`ChatState::last_user_text`). So a send that
    /// errors used to destroy the attachment while leaving on screen a button
    /// that claims it will re-send the message — the loss is total, silent, and
    /// disguised as a recovery affordance. The queue path never had this bug
    /// (`requeue_front` hands back the whole entry); only the typed path did.
    fn failed_send_restores_the_tray(src: &str) -> bool {
        let Some(body) = idle_send_body(src) else {
            return false;
        };
        body.contains("set_send_error") && body.contains("seed_draft(")
    }

    #[test]
    fn both_send_paths_carry_the_session_dials() {
        assert!(
            every_send_resolves_the_dials(include_str!("composer.rs")),
            "a phone send no longer resolves exec_tier / session_mode — the pills \
             are back to being decorative on this surface"
        );
    }

    #[test]
    fn dial_check_rejects_a_send_that_hard_codes_none() {
        // The shape this file had before: one send, both dials nailed shut.
        let before = r"
            let res = ChatApi::send(
                &dash, &text, session_key.as_deref(), Vec::new(),
                agent_id.as_deref(), project_root.as_deref(),
                None,
                // No tier/mode pickers on phone.
                None, None, false,
            ).await;
        ";
        assert!(!every_send_resolves_the_dials(before));
    }

    #[test]
    fn idle_send_takes_and_clears_the_attachment_tray() {
        assert!(
            idle_send_drains_the_tray(include_str!("composer.rs")),
            "the phone idle send no longer carries staged attachments"
        );
    }

    #[test]
    fn tray_check_rejects_a_send_that_ignores_attachments() {
        let before = r"
            let send = move || {
                let text = input_text.get_untracked().trim().to_string();
                if text.is_empty() { return; }
                input_text.set(String::new());
            };
    // The question this conversation";
        assert!(!idle_send_drains_the_tray(before));
    }

    #[test]
    fn a_failed_send_gives_the_attachments_back() {
        assert!(
            failed_send_restores_the_tray(include_str!("composer.rs")),
            "a failed phone send no longer restores the tray — the files are \
             destroyed behind a Retry button that only rebuilds the text"
        );
    }

    /// RED proof: the shape this file had, where the error arm only reported.
    #[test]
    fn restore_check_rejects_an_error_arm_that_only_reports() {
        let before = r"
            let send = move || {
                let files = attachments.get_untracked();
                attachments.set(Vec::new());
                spawn_local(async move {
                    match res {
                        Ok(resp) => chat.session_key.set(Some(resp.session_key)),
                        Err(e) => chat.set_send_error(ChatSendError::classify(e)),
                    }
                });
            };
    // The question this conversation";
        assert!(
            idle_send_drains_the_tray(before),
            "fixture must still drain"
        );
        assert!(!failed_send_restores_the_tray(before));
    }

    /// The toolbar row's class has to name something in the stylesheet, on both
    /// ends. Two separate rules hang off `.phone-composer-tools` and neither is
    /// cosmetic: one re-anchors the pickers' popovers (without it the tier
    /// popover runs off a 390 px screen and its descriptions are unreadable),
    /// the other folds the row away in landscape (without it a 390 px-tall
    /// viewport spends a fifth of its remaining height on it). A rename on
    /// either side leaves both silently unapplied — CSS has no "unknown
    /// selector" error, so nothing anywhere would say so.
    #[test]
    fn the_toolbar_row_class_is_wired_to_the_stylesheet() {
        const IOS_CSS: &str = include_str!("../../../../styles/ios.css");
        let src = production_half(include_str!("composer.rs"));
        assert!(
            src.contains("class=\"phone-composer-tools\""),
            "phone composer no longer marks its toolbar row"
        );
        assert!(
            IOS_CSS.contains(".phone-composer-tools"),
            "styles/ios.css has no rule for the composer toolbar row"
        );
        assert!(
            IOS_CSS.contains("orientation: landscape"),
            "the landscape fold-away rule is gone — a landscape phone is back to \
             spending ~36 px of its ~167 px transcript height on the toolbar row"
        );
        // The row must carry NO inline style. An inline declaration outranks
        // every stylesheet rule short of `!important`, so an inline
        // `display:flex` here silently defeats the landscape rule: it loads, it
        // matches, and the row stays put. That is how it shipped the first time
        // — the media query was verified present in the built CSS and was still
        // doing nothing.
        assert!(
            !src.contains("class=\"phone-composer-tools\" style="),
            "the composer toolbar row grew an inline style again — whatever it \
             sets can no longer be overridden by the landscape media query"
        );
    }

    /// Weak by construction — a host test cannot mount a Leptos view, so the
    /// only available grip is that the component's source still names the three
    /// controls (same reason `landing_is_derived` reads its own source). It
    /// catches deletion, not misplacement; the load-bearing assertions are the
    /// two above, which pin what actually reaches the wire.
    #[test]
    fn the_composer_still_renders_its_three_controls() {
        let src = production_half(include_str!("composer.rs"));
        for control in [
            "<AttachmentPreviewBar",
            "<ModePicker",
            "<ExecTierPicker",
            "type=\"file\"",
        ] {
            assert!(src.contains(control), "phone composer lost {control}");
        }
    }
}
