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
//! # One request, one ticket
//!
//! The card stages the answers of every outstanding question and sends them
//! as ONE `submit` action, or as ONE `cancel` action. A click never answers
//! on its own: an answer that was meant for a superseded request would
//! otherwise land on its successor.
//!
//! Every action carries the identity of the request and of the question it
//! addresses (`expected_request_id` / `expected_question_id`), so core can
//! refuse an action that was meant for a superseded request instead of
//! answering the wrong one.
//!
//! # The card is not the conversation
//!
//! A question the agent is parked on is answered IN the card. The composer
//! never answers it implicitly, never echoes the answer into the transcript
//! and never falls back to sending it as a chat message — a fallback would
//! answer a question nobody asked and bury the answer in the conversation.
//!
//! # Secrets
//!
//! A secret question is typed into a masked field, and its review line says
//! only that something was entered. The contents never reach the transcript,
//! an error message or a log.

use std::rc::Rc;

use crate::api::ClarificationApi;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::clarification::{build_cancel_params, build_submit_params, draft_summary};
use crate::state::notifications::{AskOptionView, AskQuestionView, PendingAskView};
use aleph_protocol::clarification::{ClarificationActionStatus, ClarificationInput};
use leptos::prelude::*;
use leptos::task::spawn_local;

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

/// Which action the card is about to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardAction {
    /// Send the staged answers of every outstanding question.
    Submit,
    /// Discard the whole request.
    Cancel,
}

/// The text a staged answer holds, if it holds text.
fn staged_text(input: &ClarificationInput) -> String {
    match input {
        ClarificationInput::Text { text } => text.clone(),
        ClarificationInput::Choices { .. } => String::new(),
    }
}

/// The indices a staged answer selects.
fn staged_choices(input: &ClarificationInput) -> Vec<u32> {
    match input {
        ClarificationInput::Choices { indices } => indices.clone(),
        ClarificationInput::Text { .. } => Vec::new(),
    }
}

#[component]
#[must_use]
pub fn AskUserCard(ask: PendingAskView) -> impl IntoView {
    let i18n = use_i18n();
    let Some(dashboard) = use_context::<DashboardState>() else {
        return ().into_any();
    };

    let questions = questions_of(&ask);
    let session_key = ask.session_key.clone();
    let request_id = ask.request_id.clone();
    // The question the action addresses: the one at the cursor.
    let current_question_id = questions.first().map(|q| q.id.clone()).unwrap_or_default();

    // The board owns the staged answers — they must survive a re-render of
    // this card. The local signals are only an echo that keeps the field
    // focused while the card renders.
    let staged: Vec<RwSignal<ClarificationInput>> = {
        let board = dashboard.clarifications.get_untracked();
        let drafts = board.drafts(&request_id);
        questions
            .iter()
            .map(|question| {
                RwSignal::new(drafts.get(&question.id).cloned().unwrap_or(
                    ClarificationInput::Text {
                        text: String::new(),
                    },
                ))
            })
            .collect()
    };

    let answering = RwSignal::new(false);
    let action_error = RwSignal::new(Option::<String>::None);

    // Stage one answer. The board keeps it; the local signal keeps the field
    // in sync.
    let stage: Rc<dyn Fn(String, ClarificationInput)> = {
        let dashboard = dashboard;
        let request_id = request_id.clone();
        Rc::new(move |question_id: String, input: ClarificationInput| {
            dashboard
                .clarifications
                .update_untracked(|board| board.set_draft(&request_id, &question_id, input));
        })
    };
    // Stage a typed answer. An emptied field is no answer at all, so its
    // staged value is dropped instead of sent as an empty text.
    let stage_text: Rc<dyn Fn(String, String)> = {
        let dashboard = dashboard;
        let request_id = request_id.clone();
        let stage = stage.clone();
        Rc::new(move |question_id: String, text: String| {
            let trimmed_empty = text.trim().is_empty();
            stage(
                question_id.clone(),
                ClarificationInput::Text { text: text.clone() },
            );
            if trimmed_empty {
                dashboard
                    .clarifications
                    .update_untracked(|board| board.clear_draft(&request_id, &question_id));
            }
        })
    };

    // Send one action. The latch is taken synchronously, before anything is
    // spawned, so a double click cannot produce two tickets.
    let send: Rc<dyn Fn(CardAction)> = {
        let dashboard = dashboard;
        let i18n = i18n;
        let questions = questions.clone();
        let session_key = session_key.clone();
        let request_id = request_id.clone();
        let current_question_id = current_question_id.clone();
        Rc::new(move |kind: CardAction| {
            if answering.get_untracked() {
                return;
            }
            let board = dashboard.clarifications.get_untracked();
            if !board.acquire_submission(&request_id) {
                // A ticket is already on its way.
                return;
            }
            let drafts = board.drafts(&request_id);
            let params = match kind {
                CardAction::Submit => build_submit_params(
                    &session_key,
                    &request_id,
                    &current_question_id,
                    &questions,
                    &drafts,
                ),
                CardAction::Cancel => {
                    build_cancel_params(&session_key, &request_id, &current_question_id)
                }
            };
            let Some(params) = params else {
                board.release_submission(&request_id);
                action_error.set(Some(
                    t_string!(i18n, chat.ask_user_unaddressable).to_string(),
                ));
                return;
            };
            answering.set(true);
            action_error.set(None);
            let dashboard = dashboard;
            let session_key = session_key.clone();
            let request_id = request_id.clone();
            spawn_local(async move {
                match ClarificationApi::resolve_action(&dashboard, params).await {
                    Ok(response) => match response.status {
                        ClarificationActionStatus::Answered
                        | ClarificationActionStatus::Cancelled => {
                            // Terminal: the card goes — but only if it is
                            // still the owner of this request.
                            dashboard.clarifications.update(|board| {
                                board.apply_ended(&session_key, &request_id);
                            });
                        }
                        ClarificationActionStatus::Advanced => {
                            // More to ask: the card stays. The advance push
                            // moves the cursor and releases the answers of the
                            // questions it passed.
                        }
                        ClarificationActionStatus::Stale => {
                            let _ = action_error
                                .try_set(Some(t_string!(i18n, chat.ask_user_stale).to_string()));
                            // Read-only recovery: reconcile with a snapshot,
                            // never resend.
                            let _ = ClarificationApi::refresh_pending(&dashboard).await;
                        }
                    },
                    Err(_) => {
                        let _ = action_error
                            .try_set(Some(t_string!(i18n, chat.ask_user_failed).to_string()));
                        // Read-only recovery: reconcile with a snapshot, never
                        // resend and never fall back to the conversation.
                        let _ = ClarificationApi::refresh_pending(&dashboard).await;
                    }
                }
                // Release the latch. The read is the sanctioned `try_` form:
                // this runs after an await, where the owning component may
                // already be gone.
                if let Some(board) = dashboard.clarifications.try_get_untracked() {
                    board.release_submission(&request_id);
                }
                let _ = answering.try_set(false);
            });
        })
    };

    let addressable = !request_id.is_empty() && !current_question_id.is_empty();
    let review = {
        let questions = questions.clone();
        let staged = staged.clone();
        Memo::new(move |_| {
            let drafts = questions
                .iter()
                .zip(&staged)
                .map(|(question, staged)| {
                    let answer = staged.get();
                    let ready = match &answer {
                        ClarificationInput::Text { text } => !text.trim().is_empty(),
                        ClarificationInput::Choices { indices } => !indices.is_empty(),
                    };
                    ready.then(|| (question.id.clone(), answer))
                })
                .collect::<Option<std::collections::HashMap<_, _>>>()?;
            (!drafts.is_empty()).then(|| draft_summary(&questions, &drafts))
        })
    };

    view! {
        <div class="rounded-lg border border-primary/40 bg-primary/5 px-3 py-2">
            <div class="flex items-center gap-1.5">
                <span class="text-sm leading-none">"❓"</span>
                <span class="text-sm font-medium text-text-primary">
                    {t!(i18n, chat.ask_user_header)}
                </span>
            </div>
            {questions.into_iter().zip(staged).enumerate().map(|(qi, (question, staged_signal))| {
                let has_options = !question.options.is_empty();
                let multi = question.multi_select;
                let secret = question.secret;
                let header = question.header.clone();
                let prompt = question.prompt.clone();
                let question_id = question.id.clone();
                let options = question.options.clone();
                let request_id = request_id.clone();
                let stage = stage.clone();
                let stage_text = stage_text.clone();
                view! {
                    <div class=move || if qi == 0 { "mt-1" } else { "mt-3 pt-3 border-t border-border/60" }>
                        {header.map(|h| view! {
                            <span class="inline-block px-1.5 mb-1 rounded bg-surface-raised
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
                            view! {
                                <div class="flex flex-wrap gap-2 mt-2">
                                    {options.into_iter().enumerate().map(|(index, option)| {
                                        // Zero-based, as the wire contract says.
                                        let choice = index as u32;
                                        let label = option.label.clone();
                                        let description = option.description.clone();
                                        let staged_signal = staged_signal;
                                        let question_id = question_id.clone();
                                        let stage = stage.clone();
                                        let dashboard = dashboard;
                                        let request_id = request_id.clone();
                                        view! {
                                            <button
                                                type="button"
                                                class=move || {
                                                    let base = "px-3 py-1.5 rounded text-left text-xs font-medium \
                                                                border transition-colors";
                                                    if staged_choices(&staged_signal.get()).contains(&choice) {
                                                        format!("{base} bg-primary text-white border-primary")
                                                    } else {
                                                        format!("{base} bg-surface-raised hover:bg-primary \
                                                                 hover:text-white text-text-primary border-border")
                                                    }
                                                }
                                                aria-pressed=move || staged_choices(&staged_signal.get()).contains(&choice).to_string()
                                                disabled=move || answering.get() || !addressable
                                                on:click=move |_| {
                                                    let mut next = staged_choices(&staged_signal.get_untracked());
                                                    if let Some(at) = next.iter().position(|c| *c == choice) {
                                                        next.remove(at);
                                                    } else {
                                                        if !multi {
                                                            next.clear();
                                                        }
                                                        next.push(choice);
                                                    }
                                                    if next.is_empty() {
                                                        // Un-picking leaves the
                                                        // question unanswered.
                                                        staged_signal.set(ClarificationInput::Text {
                                                            text: String::new(),
                                                        });
                                                        dashboard.clarifications.update_untracked(|board| {
                                                            board.clear_draft(&request_id, &question_id)
                                                        });
                                                    } else {
                                                        staged_signal.set(ClarificationInput::Choices {
                                                            indices: next.clone(),
                                                        });
                                                        stage(
                                                            question_id.clone(),
                                                            ClarificationInput::Choices { indices: next },
                                                        );
                                                    }
                                                }
                                            >
                                                <span class="block">{format!("{}. {label}", index + 1)}</span>
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
                        // constraint — free text is an answer too.
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
                                prop:value=move || staged_text(&staged_signal.get())
                                on:input=move |ev| {
                                    let text = event_target_value(&ev);
                                    staged_signal.set(ClarificationInput::Text { text: text.clone() });
                                    stage_text(question_id.clone(), text);
                                }
                                disabled=move || answering.get() || !addressable
                            />
                        </div>
                    </div>
                }
            }).collect::<Vec<_>>()}
            {move || action_error.get().map(|e| view! {
                <div class="mt-2 text-xs text-danger break-words">{e}</div>
            })}
            {move || review.get().map(|summary| view! {
                <p class="mt-3 text-xs text-text-secondary break-words">{summary}</p>
            })}
            // One submit for the whole set, and one cancel. Exactly one of
            // them can produce a ticket: the latch admits a single action.
            <div class="flex gap-2 mt-3">
                {
                    let send_submit = send.clone();
                    view! {
                        <button
                            type="button"
                            class="px-3 py-1.5 rounded bg-primary hover:bg-primary-hover text-white text-xs
                                   font-semibold disabled:opacity-35 disabled:cursor-not-allowed transition-colors"
                            disabled=move || answering.get() || !addressable || review.get().is_none()
                            on:click=move |_| send_submit(CardAction::Submit)
                        >
                            {t!(i18n, chat.ask_user_submit)}
                        </button>
                    }
                }
                {
                    let send_cancel = send.clone();
                    view! {
                        <button
                            type="button"
                            class="px-3 py-1.5 rounded bg-surface-raised hover:bg-surface-hover text-text-primary text-xs
                                   font-semibold disabled:opacity-35 disabled:cursor-not-allowed transition-colors"
                            disabled=move || answering.get() || !addressable
                            on:click=move |_| send_cancel(CardAction::Cancel)
                        >
                            {t!(i18n, chat.ask_user_cancel)}
                        </button>
                    }
                }
            </div>
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
            request_id: "r".into(),
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

    /// The card must not invent a second encoding: the staged answer is a
    /// typed input, and a choice is a list of zero-based indices.
    #[test]
    fn the_card_stages_typed_inputs_and_zero_based_choices() {
        let text = ClarificationInput::Text { text: "1".into() };
        assert_eq!(staged_text(&text), "1");
        assert!(staged_choices(&text).is_empty());

        let choices = ClarificationInput::Choices {
            indices: vec![0, 2],
        };
        assert!(staged_choices(&choices).contains(&0));
        assert!(!staged_choices(&choices).contains(&1));
        assert_eq!(staged_text(&choices), "");
    }
}
