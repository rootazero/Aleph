//! Minimal phone composer with a runtime-append queue. While a run is active,
//! Enter (or the ＋ button) stages the draft as a ghost bubble instead of
//! sending it blindly; the batch auto-flushes at the next turn boundary (Steer)
//! or when the run settles naturally, and ⚡ force-inserts immediately by
//! interrupting the run. Faithful subset of the wide composer's queue flow,
//! reduced to phone's surface: no attachments, no model override, no
//! slash-commands / @-mentions / voice (server remains the prompt-injection
//! authority). Ghost bubbles render in the shared `MessageList`; this file only
//! feeds the shared `ChatState` queue and the existing `ChatApi` send/abort.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::chat::ChatApi;
use crate::context::DashboardState;
use crate::views::chat::state::{ChatPhase, ChatSendError, QueuedPrompt};
use crate::views::chat::ChatState;
use shared_ui_logic::state::{should_auto_drain_on_settle, should_flush_on_turn_boundary};

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

    // Phone has no attachments, so a draft is just non-empty text.
    let has_draft = Memo::new(move |_| !input_text.get().trim().is_empty());

    // Force-insert is available while a run is active and there's something to
    // insert — queued ghosts or the current draft.
    let can_force = Memo::new(move |_| {
        chat.active_run_id.get().is_some()
            && (!chat.prompt_queue.get().is_empty() || !input_text.get().trim().is_empty())
    });

    // Idle send: start a fresh run. Unchanged from the original composer.
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
                // No tier/mode pickers on phone: the session's stored values govern.
                None,
                None,
                false,
            )
            .await;
            match res {
                Ok(resp) => chat.session_key.set(Some(resp.session_key)),
                Err(e) => chat.set_send_error(ChatSendError::classify(e)),
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
        if text.is_empty() {
            return;
        }
        chat.enqueue_prompt(QueuedPrompt {
            text,
            attachments: Vec::new(),
        });
        input_text.set(String::new());
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
            match crate::api::ClarificationApi::resolve(&dash, &ask.session_key, &reply).await {
                Ok(true) => {
                    chat.push_user_message(&reply);
                    dash.pending_clarifications
                        .update(|l| l.retain(|p| p.session_key != ask.session_key));
                }
                Ok(false) => {
                    dash.pending_clarifications
                        .update(|l| l.retain(|p| p.session_key != ask.session_key));
                    input_text.set(reply);
                    if chat.active_run_id.get_untracked().is_some() {
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
        let project_root = chat.active_project_root.get_untracked();
        let dash = dashboard;
        spawn_local(async move {
            let mut pending = batch.into_iter();
            while let Some(entry) = pending.next() {
                match ChatApi::send(
                    &dash,
                    &entry.text,
                    session_key.as_deref(),
                    Vec::new(),
                    agent_id.as_deref(),
                    project_root.as_deref(),
                    None,
                    // No tier/mode pickers on phone: the session's stored values govern.
                    None,
                    None,
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
            let dash = dashboard;
            spawn_local(async move {
                // Not session-scoped on purpose: force-insert is "run this now",
                // not "drop this work" — purging the lane would throw away the
                // prompts it just folded the draft into.
                let _ = ChatApi::abort(&dash, &run_id, None).await;
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
        let dash = dashboard;
        spawn_local(async move {
            // Stop must reach the session's server-side backlog too, or the
            // freed slot lets the lane fire the queued messages one run at a
            // time — exactly what the user just refused.
            let _ = ChatApi::abort(&dash, &run_id, session_key.as_deref()).await;
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
    }
}
