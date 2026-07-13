//! `AskUserCard` — the one renderer for a question the agent is parked on.
//!
//! The clarification twin of [`crate::components::approval_card`]: an inline
//! card in the conversation that is waiting, resolved through a single RPC.
//! Rendered at the tail of the transcript (`views::chat::messages`) — the
//! question is always the newest thing that happened, and the card sits where
//! the answer is given.
//!
//! Answering does NOT start a run: it unblocks the `ask_user` tool call inside
//! the turn that is still in flight, and the model carries on by itself.
//!
//! The reply is whatever core interprets it as (R4): a choice click sends the
//! 1-based index — byte-for-byte the payload Telegram's `clarify:<idx>` inline
//! button produces — and free text is sent verbatim. The panel never maps a
//! click to an option value itself; core's `interpret_reply` is the only
//! interpreter, so the two surfaces can never disagree.

use crate::api::ClarificationApi;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::notifications::PendingAskView;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Post `reply` back for `session_key` and drop the question from the pending
/// list once it is over. `resolved == false` means nobody was waiting any more
/// (expired / superseded / run cancelled) — the question is just as dead, so
/// the card goes either way. The `clarification_ended` frame is the authority
/// behind this optimistic removal.
fn answer(dashboard: DashboardState, session_key: String, reply: String) {
    spawn_local(async move {
        match ClarificationApi::resolve(&dashboard, &session_key, &reply).await {
            Ok(resolved) => {
                if !resolved {
                    web_sys::console::warn_1(
                        &"Question was no longer pending — answer discarded".into(),
                    );
                }
                dashboard
                    .pending_clarifications
                    .update(|l| l.retain(|x| x.session_key != session_key));
            }
            Err(e) => {
                web_sys::console::warn_1(&format!("Failed to answer question: {e:?}").into());
            }
        }
    });
}

#[component]
#[must_use]
pub fn AskUserCard(ask: PendingAskView) -> impl IntoView {
    let i18n = use_i18n();
    let Some(dashboard) = use_context::<DashboardState>() else {
        return ().into_any();
    };

    let free_text = RwSignal::new(String::new());
    let question = ask.question.clone();
    let options = ask.options.clone();
    let has_options = !options.is_empty();
    // Stored (not captured by value) so the submit closure stays `Copy` — the
    // text field's Enter handler and the Answer button both need it.
    let session_for_text = StoredValue::new(ask.session_key.clone());

    let submit_text = move || {
        let reply = free_text.get_untracked().trim().to_string();
        if reply.is_empty() {
            return;
        }
        free_text.set(String::new());
        answer(dashboard, session_for_text.get_value(), reply);
    };

    view! {
        <div class="rounded-lg border border-primary/40 bg-primary/5 px-3 py-2">
            <div class="flex items-center gap-1.5">
                <span class="text-sm leading-none">"❓"</span>
                <span class="text-sm font-medium text-text-primary">
                    {t!(i18n, chat.ask_user_header)}
                </span>
            </div>
            <p class="text-sm my-1 text-text-primary whitespace-pre-wrap">{question}</p>
            {has_options.then(|| {
                let session_key = ask.session_key.clone();
                view! {
                    <div class="flex flex-wrap gap-2 mt-2">
                        {options.into_iter().enumerate().map(|(i, label)| {
                            let session_key = session_key.clone();
                            // 1-based index — the string core's `interpret_reply`
                            // maps to this option, exactly as for a typed number.
                            let reply = (i + 1).to_string();
                            view! {
                                <button
                                    type="button"
                                    class="px-3 py-1.5 rounded bg-surface-raised hover:bg-primary hover:text-white
                                           text-text-primary text-xs font-medium border border-border transition-colors"
                                    on:click=move |_| answer(dashboard, session_key.clone(), reply.clone())
                                >
                                    {format!("{}. {label}", i + 1)}
                                </button>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                }
            })}
            // Always offered: a choice list is a shortcut, not a constraint —
            // core takes an unmatched reply as free text either way.
            <div class="flex gap-2 mt-2">
                <input
                    type="text"
                    class="flex-1 min-w-0 px-2 py-1.5 rounded bg-surface-sunken border border-border
                           text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none
                           focus:border-primary transition-colors"
                    placeholder=move || t_string!(i18n, chat.ask_user_placeholder).to_string()
                    prop:value=move || free_text.get()
                    on:input=move |ev| free_text.set(event_target_value(&ev))
                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                        if ev.key() == "Enter" {
                            ev.prevent_default();
                            submit_text();
                        }
                    }
                />
                <button
                    type="button"
                    class="px-3 py-1.5 rounded bg-primary hover:bg-primary-hover text-white text-xs
                           font-semibold disabled:opacity-35 disabled:cursor-not-allowed transition-colors"
                    disabled=move || free_text.get().trim().is_empty()
                    on:click=move |_| submit_text()
                >
                    {t!(i18n, chat.ask_user_answer)}
                </button>
            </div>
        </div>
    }
    .into_any()
}
