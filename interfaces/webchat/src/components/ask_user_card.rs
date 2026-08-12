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
//!
//! # Two shapes, one interpreter
//!
//! A request carries one or more questions. With one outstanding, a choice
//! click answers immediately — the one-click interaction this card has always
//! had. With several, the card becomes a short form and submits every answer
//! at once (`clarification.resolve { answers }`); core walks them across
//! consecutive questions using the same per-question rules. Neither shape
//! interprets anything locally.
//!
//! A core that predates the structured `questions` view sends none, and the
//! card falls back to the flat `question` / `options` pair — the same
//! degradation a plain-text channel gets, for the same reason.

use crate::api::ClarificationApi;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::notifications::{AskOptionView, AskQuestionView, PendingAskView};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Post `answers` back for `session_key` and drop the question from the pending
/// list once the whole request is over.
///
/// The card goes on two different facts, and conflating them is the bug this
/// function exists to avoid:
///
/// * `accepted == false` — nobody was waiting any more (expired / superseded /
///   run cancelled). The question is dead, so the card goes.
/// * `is_finished()` — every question was answered and the parked tool
///   resumed. Also done.
///
/// `accepted && !is_finished()` is neither: the answer landed and there is more
/// to ask. The card stays, and the `stream.ask_user` frame core publishes on
/// advance re-renders it at the new cursor.
fn answer(dashboard: DashboardState, session_key: String, answers: Vec<String>) {
    spawn_local(async move {
        let outcome = if answers.len() == 1 {
            ClarificationApi::resolve(&dashboard, &session_key, &answers[0]).await
        } else {
            ClarificationApi::resolve_all(&dashboard, &session_key, &answers).await
        };
        match outcome {
            Ok(outcome) => {
                if !outcome.accepted {
                    web_sys::console::warn_1(
                        &"Question was no longer pending — answer discarded".into(),
                    );
                }
                if !outcome.accepted || outcome.is_finished() {
                    dashboard
                        .pending_clarifications
                        .update(|l| l.retain(|x| x.session_key != session_key));
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&format!("Failed to answer question: {e:?}").into());
            }
        }
    });
}

/// The questions this card must render.
///
/// Falls back to a one-question view built from the flat projection when the
/// structured list is absent, so one render path serves both.
fn questions_of(ask: &PendingAskView) -> Vec<AskQuestionView> {
    let remaining = ask.remaining();
    if !remaining.is_empty() {
        return remaining.to_vec();
    }
    vec![AskQuestionView {
        id: String::new(),
        header: None,
        prompt: ask.question.clone(),
        options: ask
            .options
            .iter()
            .map(|label| AskOptionView {
                label: label.clone(),
                description: None,
            })
            .collect(),
        multi_select: false,
        secret: false,
    }]
}

/// Toggle `index` (1-based, as a string) into a comma-separated multi-select
/// answer, preserving pick order.
///
/// Comma-separated indices are exactly what a user types into a channel for the
/// same question, so the panel is not inventing a second encoding — core's
/// `interpret_reply` parses this string identically from either surface.
fn toggle_index(current: &str, index: usize) -> String {
    let token = index.to_string();
    let mut picks: Vec<&str> = current
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    match picks.iter().position(|p| *p == token) {
        Some(at) => {
            picks.remove(at);
        }
        None => picks.push(&token),
    }
    picks.join(", ")
}

/// Whether `answer` currently selects the 1-based `index`.
fn is_selected(answer: &str, index: usize) -> bool {
    let token = index.to_string();
    answer.split(',').map(str::trim).any(|p| p == token)
}

#[component]
#[must_use]
pub fn AskUserCard(ask: PendingAskView) -> impl IntoView {
    let i18n = use_i18n();
    let Some(dashboard) = use_context::<DashboardState>() else {
        return ().into_any();
    };

    let questions = questions_of(&ask);
    // One draft per outstanding question. Created here (not in a signal of
    // signals) because the parent re-runs this component whenever the pending
    // view changes, which is exactly when the question set changes.
    let drafts: Vec<RwSignal<String>> = questions
        .iter()
        .map(|_| RwSignal::new(String::new()))
        .collect();
    // A single question is answered the moment a choice is clicked — that is
    // the interaction this card has always had, and it is still right when
    // there is nothing else to fill in.
    let instant = questions.len() == 1 && !questions[0].multi_select;
    let session_key = StoredValue::new(ask.session_key.clone());
    let drafts_store = StoredValue::new(drafts.clone());

    let submit_all = move || {
        let answers: Vec<String> = drafts_store
            .get_value()
            .iter()
            .map(|d| d.get_untracked().trim().to_string())
            .collect();
        if answers.iter().any(String::is_empty) {
            return;
        }
        answer(dashboard, session_key.get_value(), answers);
    };
    let all_answered = {
        let drafts_store = drafts_store;
        move || {
            drafts_store
                .get_value()
                .iter()
                .all(|d| !d.get().trim().is_empty())
        }
    };
    let total = questions.len();

    view! {
        <div class="rounded-lg border border-primary/40 bg-primary/5 px-3 py-2">
            <div class="flex items-center gap-1.5">
                <span class="text-sm leading-none">"❓"</span>
                <span class="text-sm font-medium text-text-primary">
                    {t!(i18n, chat.ask_user_header)}
                </span>
            </div>
            {questions.into_iter().zip(drafts).enumerate().map(|(qi, (question, draft))| {
                let has_options = !question.options.is_empty();
                let multi = question.multi_select;
                let secret = question.secret;
                let header = question.header.clone();
                let prompt = question.prompt.clone();
                // `StoredValue` so the closure stays `Copy` — the text field's
                // Enter handler and the Answer button both need it.
                let session_key_q = StoredValue::new(ask.session_key.clone());
                // Enter in a text field. On a one-question card it answers
                // outright. On a form it submits the WHOLE set once every
                // question has something in it — pressing Enter used to do
                // nothing at all there, silently, which reads as a broken
                // field rather than as "there is a button below". Incomplete
                // is still a no-op, and visibly so: the submit button is
                // disabled by the same predicate.
                let submit_text = move || {
                    if !instant {
                        submit_all();
                        return;
                    }
                    let reply = draft.get_untracked().trim().to_string();
                    if reply.is_empty() {
                        return;
                    }
                    draft.set(String::new());
                    answer(dashboard, session_key_q.get_value(), vec![reply]);
                };
                view! {
                    <div class=move || if qi == 0 { "mt-1" } else { "mt-3 pt-3 border-t border-border/60" }>
                        {header.map(|h| view! {
                            <span class="inline-block px-1.5 py-0.5 mb-1 rounded bg-surface-raised
                                         text-[10px] uppercase tracking-wide text-text-tertiary">
                                {h}
                            </span>
                        })}
                        <p class="text-sm my-1 text-text-primary whitespace-pre-wrap">{prompt}</p>
                        {multi.then(|| view! {
                            <p class="text-[11px] text-text-tertiary">
                                {t!(i18n, chat.ask_user_multi_hint)}
                            </p>
                        })}
                        {has_options.then(|| {
                            let session_key_o = ask.session_key.clone();
                            view! {
                                <div class="flex flex-wrap gap-2 mt-2">
                                    {question.options.into_iter().enumerate().map(|(i, opt)| {
                                        let session_key_o = session_key_o.clone();
                                        // 1-based index — the string core's
                                        // `interpret_reply` maps to this option,
                                        // exactly as for a typed number.
                                        let idx = i + 1;
                                        let reply = idx.to_string();
                                        let label = opt.label.clone();
                                        let description = opt.description.clone();
                                        view! {
                                            <button
                                                type="button"
                                                class=move || {
                                                    let base = "px-3 py-1.5 rounded text-left text-xs font-medium \
                                                                border transition-colors";
                                                    if is_selected(&draft.get(), idx) {
                                                        format!("{base} bg-primary text-white border-primary")
                                                    } else {
                                                        format!("{base} bg-surface-raised hover:bg-primary \
                                                                 hover:text-white text-text-primary border-border")
                                                    }
                                                }
                                                on:click=move |_| {
                                                    if instant {
                                                        answer(dashboard, session_key_o.clone(), vec![reply.clone()]);
                                                    } else if multi {
                                                        draft.update(|d| *d = toggle_index(d, idx));
                                                    } else {
                                                        draft.set(reply.clone());
                                                    }
                                                }
                                            >
                                                <span class="block">{format!("{idx}. {label}")}</span>
                                                // The description a channel has
                                                // rendered since the option type
                                                // gained the field. The panel used
                                                // to show a bare label because it
                                                // read the flat label array, which
                                                // structurally cannot carry one.
                                                {description.map(|d| view! {
                                                    <span class="block mt-0.5 font-normal opacity-70">{d}</span>
                                                })}
                                            </button>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }
                        })}
                        // Always offered: a choice list is a shortcut, not a
                        // constraint — core takes an unmatched reply as free
                        // text either way.
                        <div class="flex gap-2 mt-2">
                            <input
                                type=if secret { "password" } else { "text" }
                                class="flex-1 min-w-0 px-2 py-1.5 rounded bg-surface-sunken border border-border
                                       text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none
                                       focus:border-primary transition-colors"
                                placeholder=move || if secret {
                                    t_string!(i18n, chat.ask_user_secret_placeholder).to_string()
                                } else {
                                    t_string!(i18n, chat.ask_user_placeholder).to_string()
                                }
                                prop:value=move || draft.get()
                                on:input=move |ev| draft.set(event_target_value(&ev))
                                on:keydown=move |ev: web_sys::KeyboardEvent| {
                                    if ev.key() == "Enter" {
                                        ev.prevent_default();
                                        submit_text();
                                    }
                                }
                            />
                            {instant.then(|| view! {
                                <button
                                    type="button"
                                    class="px-3 py-1.5 rounded bg-primary hover:bg-primary-hover text-white text-xs
                                           font-semibold disabled:opacity-35 disabled:cursor-not-allowed transition-colors"
                                    disabled=move || draft.get().trim().is_empty()
                                    on:click=move |_| submit_text()
                                >
                                    {t!(i18n, chat.ask_user_answer)}
                                </button>
                            })}
                        </div>
                    </div>
                }
            }).collect::<Vec<_>>()}
            // One submit for the whole set. Present exactly when a single
            // click cannot finish the request — a multi-select question needs
            // its picks confirmed, and several questions need every one filled.
            {(!instant).then(|| view! {
                <button
                    type="button"
                    class="mt-3 px-3 py-1.5 rounded bg-primary hover:bg-primary-hover text-white text-xs
                           font-semibold disabled:opacity-35 disabled:cursor-not-allowed transition-colors"
                    disabled=move || !all_answered()
                    on:click=move |_| submit_all()
                >
                    {move || if total > 1 {
                        t_string!(i18n, chat.ask_user_submit).to_string()
                    } else {
                        t_string!(i18n, chat.ask_user_answer).to_string()
                    }}
                </button>
            })}
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(questions: Vec<AskQuestionView>, answered: usize) -> PendingAskView {
        PendingAskView {
            session_key: "s".into(),
            question: "legacy prompt".into(),
            options: vec!["a".into(), "b".into()],
            questions,
            answered,
        }
    }

    fn q(prompt: &str) -> AskQuestionView {
        AskQuestionView {
            id: prompt.into(),
            header: None,
            prompt: prompt.into(),
            options: vec![],
            multi_select: false,
            secret: false,
        }
    }

    /// A core that predates the structured view sends none. The card must still
    /// render a question rather than an empty box — the same fallback a
    /// plain-text channel gets.
    #[test]
    fn falls_back_to_the_flat_projection_when_there_is_no_structured_view() {
        let rendered = questions_of(&view(vec![], 0));
        assert_eq!(rendered.len(), 1);
        assert_eq!(rendered[0].prompt, "legacy prompt");
        assert_eq!(rendered[0].options.len(), 2);
        assert_eq!(rendered[0].options[0].label, "a");
    }

    /// Already-answered questions must not re-render: the card resumes at the
    /// cursor, exactly where a reconnecting channel would.
    #[test]
    fn renders_only_the_questions_still_outstanding() {
        let rendered = questions_of(&view(vec![q("first"), q("second"), q("third")], 2));
        assert_eq!(rendered.len(), 1);
        assert_eq!(rendered[0].prompt, "third");
    }

    /// An `answered` past the end (a frame that raced a completion) must not
    /// panic or resurrect question 0 — it falls back like an empty list.
    #[test]
    fn an_out_of_range_cursor_degrades_instead_of_panicking() {
        let rendered = questions_of(&view(vec![q("only")], 9));
        assert_eq!(rendered.len(), 1);
        assert_eq!(rendered[0].prompt, "legacy prompt");
    }

    #[test]
    fn multi_select_toggle_preserves_pick_order_and_removes() {
        assert_eq!(toggle_index("", 2), "2");
        assert_eq!(toggle_index("2", 1), "2, 1");
        assert_eq!(toggle_index("2, 1", 2), "1");
        assert_eq!(toggle_index("2, 1", 3), "2, 1, 3");
    }

    #[test]
    fn selection_test_matches_whole_tokens_only() {
        assert!(is_selected("1, 2", 2));
        assert!(!is_selected("1, 2", 3));
        // "12" must not read as a selection of 1 or 2.
        assert!(!is_selected("12", 1));
        assert!(is_selected("12", 12));
    }
}
