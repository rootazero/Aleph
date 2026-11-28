//! Pure modal presentation and clarification submission controller.
//!
//! No RPC, persistence, Core interpretation, or logging lives here. Both
//! drawing and key routing must consult `active_owner()`; AppState integration
//! is deliberately deferred until the other worker has finished app/*.rs.
//!
//! The questionnaire is owned here: every outstanding question keeps its own
//! draft buffer, its own staged answer, and its own identity guard. Nothing
//! is shared with the ordinary composer, so a staged credential never reaches
//! a text field that is not the question it belongs to.

use std::collections::VecDeque;

use aleph_protocol::{
    AskUserOption, AskUserQuestion, ClarificationAction, ClarificationActionParams,
    ClarificationActionResponse, ClarificationActionStatus, ClarificationAnswerInput,
    ClarificationInput, PendingClarification, StreamEvent,
};

use super::app::{ApprovalState, DialogState};

/// What a staged secret answer looks like in the review step. The cleartext
/// never leaves the question's own buffer.
const FILLED_MARKER: &str = "•••• (filled)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalOwner {
    Clarification {
        session_key: String,
        request_id: String,
    },
    Approval {
        session_key: String,
        id: String,
    },
}

impl ModalOwner {
    /// The session the owner belongs to. Key routing uses it to address the
    /// overlay it is about to act on.
    pub fn session_key(&self) -> &str {
        match self {
            Self::Clarification { session_key, .. } | Self::Approval { session_key, .. } => {
                session_key
            }
        }
    }
}

/// A reconciliation result, not a claim that an answer reached Core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionPhase {
    Editing,
    Submitting,
    Reconciling,
    Error,
    /// An identity-less projection cannot be addressed safely: it stays
    /// read-only until a pending snapshot supplies both identities.
    LegacyConfirmation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckEffect {
    Ignored,
    Closed,
    RefreshPending,
}

/// Which part of the questionnaire the user is looking at.
///
/// `Editing` shows one question at a time; `Review` shows every staged answer
/// before anything is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionnaireStage {
    Editing,
    Review,
}

/// One staged answer as the review step shows it.
///
/// A secret answer is rendered as a filled marker, never as its cleartext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRow {
    pub question_id: String,
    /// The question's prompt, as presented.
    pub question: String,
    /// What will be sent. Masked when the question is secret.
    pub value: String,
    pub secret: bool,
}

/// The reply may be a credential. Keep this value only in the in-flight RPC;
/// deliberately do not derive Debug, Serialize, or display it in diagnostics.
pub struct Submission {
    pub params: ClarificationActionParams,
    pub generation: u64,
    nonce: u64,
}

impl Submission {
    /// Opaque correlation key for `RpcTag.identity`. It never contains the
    /// possibly secret reply. Retain the original ticket for `acknowledge`.
    #[must_use]
    pub fn rpc_identity(&self) -> String {
        format!("clarification:{}:{}", self.generation, self.nonce)
    }
}

struct QuestionCard {
    owner: ModalOwner,
    run_id: String,
    answered: usize,
    /// Identity of the first outstanding question — the guard the server
    /// re-checks before applying an action.
    question_id: String,
    /// Every outstanding question, in presented order.
    questions: Vec<AskUserQuestion>,
    /// One independent draft buffer per outstanding question.
    dialogs: Vec<DialogState>,
    /// Staged answers; `None` while a question is still being edited.
    drafts: Vec<Option<ClarificationAnswerInput>>,
    /// The question currently being edited.
    cursor: usize,
    /// The highlighted row of the review step.
    review_cursor: usize,
    stage: QuestionnaireStage,
    phase: SubmissionPhase,
    hidden: bool,
    superseded: bool,
    nonce: u64,
}

struct ApprovalCard {
    owner: ModalOwner,
    hidden: bool,
}

enum Card {
    Question(QuestionCard),
    Approval(ApprovalCard),
}

impl Card {
    fn owner(&self) -> &ModalOwner {
        match self {
            Self::Question(q) => &q.owner,
            Self::Approval(a) => &a.owner,
        }
    }

    fn hidden(&self) -> bool {
        match self {
            Self::Question(q) => q.hidden,
            Self::Approval(a) => a.hidden,
        }
    }

    fn hide(&mut self) {
        match self {
            Self::Question(q) => q.hidden = true,
            Self::Approval(a) => a.hidden = true,
        }
    }

    fn reveal(&mut self) {
        match self {
            Self::Question(q) => q.hidden = false,
            Self::Approval(a) => a.hidden = false,
        }
    }
}

/// A single ordered presentation queue; the approval service retains its own
/// authoritative business queue. Session switches do not discard drafts.
pub struct InteractionController {
    cards: VecDeque<Card>,
    session_key: String,
    generation: u64,
    paused_for_picker: bool,
    next_nonce: u64,
    /// Retired identities are tombstoned so an out-of-order AskUser frame
    /// cannot resurrect a superseded or completed waiter. Bound memory per UI.
    retired: VecDeque<(String, String)>,
    retired_approvals: VecDeque<(String, String)>,
}

impl std::fmt::Debug for InteractionController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InteractionController")
            .field("cards", &self.cards.len())
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl InteractionController {
    #[must_use]
    pub fn new(session_key: impl Into<String>) -> Self {
        Self {
            cards: VecDeque::new(),
            session_key: session_key.into(),
            generation: 0,
            paused_for_picker: false,
            next_nonce: 0,
            retired: VecDeque::new(),
            retired_approvals: VecDeque::new(),
        }
    }

    /// Caller-supplied connection/session generation scopes background effects.
    /// Changing the session invalidates every earlier submission ticket while
    /// preserving queued cards and drafts for a later return to that session.
    pub fn set_context(&mut self, session_key: impl Into<String>, generation: u64) {
        let session_key = session_key.into();
        if self.generation != generation || self.session_key != session_key {
            for card in &mut self.cards {
                if let Card::Question(q) = card {
                    if q.phase == SubmissionPhase::Submitting {
                        q.phase = SubmissionPhase::Reconciling;
                    }
                }
            }
        }
        self.session_key = session_key;
        self.generation = generation;
        self.paused_for_picker = false;
    }

    pub fn pause_for_picker(&mut self, paused: bool) {
        self.paused_for_picker = paused;
    }

    /// Only this accessor should choose the drawing and keyboard target.
    #[must_use]
    pub fn active_owner(&self) -> Option<&ModalOwner> {
        if self.paused_for_picker {
            return None;
        }
        self.cards.iter().find(|c| !c.hidden()).map(Card::owner)
    }

    /// The actionable owner for both renderer and key dispatch. A queued
    /// approval whose state is no longer present must never receive keys while
    /// invisible. The caller should retire it and advance the queue.
    #[must_use]
    pub fn displayed_owner(&self, approval: Option<&ApprovalState>) -> Option<&ModalOwner> {
        let owner = self.active_owner()?;
        match owner {
            ModalOwner::Approval { id, .. }
                if approval.is_none_or(|a| a.id.as_str() != id.as_str()) =>
            {
                None
            }
            _ => Some(owner),
        }
    }

    #[must_use]
    pub fn active_dialog(&self) -> Option<&DialogState> {
        let owner = self.active_owner()?;
        self.cards.iter().find_map(|card| match card {
            Card::Question(q) if &q.owner == owner => q.dialogs.get(q.cursor),
            _ => None,
        })
    }

    /// A mutable draft is available only for the question actually displayed.
    /// Key routing must use this accessor, never search by session alone.
    ///
    /// The review step has no live buffer: the staged answers are what the
    /// user is looking at, so editing must go back through the question.
    pub fn active_dialog_mut(&mut self) -> Option<&mut DialogState> {
        let owner = self.active_owner()?.clone();
        self.cards.iter_mut().find_map(|card| match card {
            Card::Question(q)
                if q.owner == owner
                    && q.phase != SubmissionPhase::Submitting
                    && q.stage == QuestionnaireStage::Editing =>
            {
                q.dialogs.get_mut(q.cursor)
            }
            _ => None,
        })
    }

    #[must_use]
    pub fn active_phase(&self) -> Option<SubmissionPhase> {
        let owner = self.active_owner()?;
        self.cards.iter().find_map(|card| match card {
            Card::Question(q) if &q.owner == owner => Some(q.phase),
            _ => None,
        })
    }

    /// Whether the review step is on screen: every outstanding question has a
    /// staged answer and nothing has been sent yet.
    #[must_use]
    pub fn in_review(&self) -> bool {
        self.active_question_card()
            .is_some_and(|q| q.stage == QuestionnaireStage::Review)
    }

    /// The staged answers, in presented order.
    #[must_use]
    pub fn review_rows(&self) -> Vec<ReviewRow> {
        let Some(q) = self.active_question_card() else {
            return Vec::new();
        };
        q.questions
            .iter()
            .enumerate()
            .map(|(index, question)| {
                let draft = q.drafts.get(index).and_then(|draft| draft.as_ref());
                let secret = question.secret;
                let value = match draft.map(|draft| &draft.answer) {
                    Some(ClarificationInput::Text { text }) => {
                        if secret {
                            FILLED_MARKER.to_string()
                        } else {
                            text.clone()
                        }
                    }
                    Some(ClarificationInput::Choices { indices }) => {
                        if secret {
                            FILLED_MARKER.to_string()
                        } else {
                            indices
                                .iter()
                                .filter_map(|&index| question.options.get(index as usize))
                                .map(|option| option.label.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    }
                    None => String::new(),
                };
                ReviewRow {
                    question_id: question.id.clone(),
                    question: question.prompt.clone(),
                    value,
                    secret,
                }
            })
            .collect()
    }

    /// The highlighted row of the review step.
    #[must_use]
    pub fn review_cursor(&self) -> usize {
        self.active_question_card().map_or(0, |q| q.review_cursor)
    }

    /// Move the review highlight. Clamped at both ends.
    pub fn move_review_cursor(&mut self, delta: isize) -> bool {
        let Some(q) = self.active_question_card_mut() else {
            return false;
        };
        if q.stage != QuestionnaireStage::Review || q.questions.is_empty() {
            return false;
        }
        let last = q.questions.len() - 1;
        let next = (q.review_cursor as isize + delta).clamp(0, last as isize);
        let changed = next as usize != q.review_cursor;
        q.review_cursor = next as usize;
        changed
    }

    /// Go back to editing the highlighted question so a staged answer can be
    /// changed before the questionnaire is submitted.
    pub fn edit_review_row(&mut self) -> bool {
        let Some(q) = self.active_question_card_mut() else {
            return false;
        };
        if q.stage != QuestionnaireStage::Review || q.phase != SubmissionPhase::Editing {
            return false;
        }
        q.stage = QuestionnaireStage::Editing;
        q.cursor = q.review_cursor.min(q.questions.len().saturating_sub(1));
        true
    }

    /// Move to another outstanding question without losing its draft.
    pub fn move_question(&mut self, delta: isize) -> bool {
        let Some(q) = self.active_question_card_mut() else {
            return false;
        };
        if q.stage != QuestionnaireStage::Editing || q.questions.is_empty() {
            return false;
        }
        let last = q.questions.len() - 1;
        let next = (q.cursor as isize + delta).clamp(0, last as isize);
        let changed = next as usize != q.cursor;
        q.cursor = next as usize;
        changed
    }

    #[must_use]
    pub fn active_was_superseded(&self) -> bool {
        let Some(owner) = self.active_owner() else {
            return false;
        };
        self.cards
            .iter()
            .any(|card| matches!(card, Card::Question(q) if &q.owner == owner && q.superseded))
    }

    /// No fallback to `current_run`: a missing run ID cannot identify the task
    /// to cancel. Approvals use their separate decision/denial flow.
    #[must_use]
    pub fn active_cancel_run(&self) -> Option<&str> {
        let owner = self.active_owner()?;
        self.cards.iter().find_map(|card| match card {
            Card::Question(q) if &q.owner == owner && !q.run_id.is_empty() => {
                Some(q.run_id.as_str())
            }
            _ => None,
        })
    }

    /// An approval ID is display-only; resolution remains with the existing
    /// approval service. A new approval never steals a currently edited card.
    pub fn offer_approval(&mut self, session_key: impl Into<String>, id: impl Into<String>) {
        let owner = ModalOwner::Approval {
            session_key: session_key.into(),
            id: id.into(),
        };
        if !self.cards.iter().any(|c| c.owner() == &owner) {
            self.cards.push_back(Card::Approval(ApprovalCard {
                owner,
                hidden: false,
            }));
        }
    }

    pub fn approval_retired(&self, session_key: &str, id: &str) -> bool {
        self.retired_approvals
            .iter()
            .any(|(key, retired)| key == session_key && retired == id)
    }

    pub fn retire_approval(&mut self, session_key: &str, id: &str) {
        if !self.approval_retired(session_key, id) {
            self.retired_approvals
                .push_back((session_key.to_owned(), id.to_owned()));
            if self.retired_approvals.len() > 256 {
                self.retired_approvals.pop_front();
            }
        }
        self.cards.retain(|card| {
            !matches!(card.owner(), ModalOwner::Approval { session_key: key, id: current }
                if key == session_key && current == id)
        });
    }

    /// Install a question after the push or snapshot admission checks.
    /// A confirmed replacement keeps its predecessor's place in the queue.
    /// Projections without a request ID remain read-only.
    pub fn offer_question(&mut self, pending: PendingClarification) {
        let owner = ModalOwner::Clarification {
            session_key: pending.session_key.clone(),
            request_id: pending.request_id.clone(),
        };
        let questions = outstanding_questions(&pending);
        if questions.is_empty() {
            return;
        }
        let question_id = questions[0].id.clone();
        if self
            .retired
            .iter()
            .any(|(session, id)| session == &pending.session_key && id == &pending.request_id)
        {
            return;
        }
        let Some(index) = self.cards.iter().position(|card| {
            matches!(card.owner(), ModalOwner::Clarification { session_key, .. }
                if session_key == &pending.session_key)
        }) else {
            self.next_nonce = self.next_nonce.wrapping_add(1);
            let card = QuestionCard::new(
                owner,
                pending.run_id.clone(),
                pending.answered,
                questions,
                &pending,
                false,
                self.next_nonce,
            );
            self.cards.push_back(Card::Question(card));
            return;
        };
        let (same_owner, older_cursor, same_cursor) = {
            let Some(Card::Question(existing)) = self.cards.get(index) else {
                return;
            };
            (
                existing.owner == owner,
                pending.answered < existing.answered,
                pending.answered == existing.answered && existing.question_id == question_id,
            )
        };
        if same_owner {
            if older_cursor {
                return;
            }
            if same_cursor {
                if let Some(Card::Question(existing)) = self.cards.get_mut(index) {
                    if !pending.run_id.is_empty() {
                        existing.run_id = pending.run_id.clone();
                    }
                    // The outstanding list may have grown; the drafts stay.
                    existing.rebuild(&pending, questions);
                }
                return;
            }
        }
        let superseded = !same_owner;
        let retired_owner = if superseded {
            match self.cards.get(index) {
                Some(Card::Question(old)) => match &old.owner {
                    ModalOwner::Clarification {
                        session_key,
                        request_id,
                    } => Some((session_key.clone(), request_id.clone())),
                    ModalOwner::Approval { .. } => None,
                },
                _ => None,
            }
        } else {
            None
        };
        if let Some((session, id)) = retired_owner {
            self.remember_retired(&session, &id);
            // The old card's secret draft is dropped by replacement.
        }
        self.next_nonce = self.next_nonce.wrapping_add(1);
        let mut card = QuestionCard::new(
            owner,
            pending.run_id.clone(),
            pending.answered,
            questions,
            &pending,
            superseded,
            self.next_nonce,
        );
        if !superseded {
            // The same request moved on: drafts of the questions that are
            // still outstanding are still applicable, the rest are obsolete.
            if let Some(Card::Question(old)) = self.cards.get(index) {
                card.adopt_from(old);
            }
        }
        self.cards[index] = Card::Question(card);
    }

    fn remember_retired(&mut self, session_key: &str, request_id: &str) {
        if request_id.is_empty()
            || self
                .retired
                .iter()
                .any(|(s, r)| s == session_key && r == request_id)
        {
            return;
        }
        self.retired
            .push_back((session_key.to_owned(), request_id.to_owned()));
        if self.retired.len() > 128 {
            self.retired.pop_front();
        }
    }

    /// Consume notifications without letting competing request IDs order each
    /// other. Returns true when the authoritative pending snapshot must decide.
    pub fn observe_event(&mut self, event: &StreamEvent) -> bool {
        match event {
            StreamEvent::AskUser {
                run_id,
                session_key,
                request_id,
                question,
                options,
                questions,
                answered,
                ..
            } => {
                if session_key.is_empty() {
                    return true;
                }
                if self
                    .retired
                    .iter()
                    .any(|(s, r)| s == session_key && r == request_id)
                {
                    return false;
                }
                // Request IDs are opaque. If this frame names a different
                // request, only a snapshot can say which is live.
                if self.cards.iter().any(|card| {
                    matches!(card.owner(), ModalOwner::Clarification {
                        session_key: key, request_id: current,
                    } if key == session_key && current != request_id)
                }) {
                    return true;
                }
                self.offer_question(PendingClarification {
                    session_key: session_key.clone(),
                    request_id: request_id.clone(),
                    run_id: run_id.clone(),
                    question: question.clone(),
                    options: options.clone(),
                    questions: questions.clone(),
                    answered: *answered,
                });
                request_id.is_empty()
            }
            StreamEvent::ClarificationEnded {
                session_key,
                request_id,
                ..
            } => {
                if session_key.is_empty() || request_id.is_empty() {
                    true
                } else {
                    self.end_question(session_key, request_id);
                    false
                }
            }
            _ => false,
        }
    }

    /// End only the matching request. A legacy terminal with no request ID
    /// cannot safely close a newer identity-bearing request.
    pub fn end_question(&mut self, session_key: &str, request_id: &str) {
        if request_id.is_empty() {
            return;
        }
        self.remember_retired(session_key, request_id);
        self.cards.retain(|card| {
            !matches!(card.owner(), ModalOwner::Clarification { session_key: key, request_id: current }
                if key == session_key && current == request_id)
        });
    }

    /// Esc hides exactly the visible modal and leaves a reopening path.
    pub fn hide_active(&mut self) -> Option<ModalOwner> {
        let owner = self.active_owner()?.clone();
        if let Some(card) = self.cards.iter_mut().find(|card| card.owner() == &owner) {
            card.hide();
        }
        Some(owner)
    }

    /// Explicit reopen of a hidden card, e.g. a command or pending indicator.
    /// A caller may inspect `hidden_count()` to offer this action.
    pub fn reopen(&mut self, owner: &ModalOwner) -> bool {
        let Some(card) = self.cards.iter_mut().find(|card| card.owner() == owner) else {
            return false;
        };
        card.reveal();
        true
    }

    #[must_use]
    pub fn hidden_count(&self) -> usize {
        self.cards
            .iter()
            .filter(|card| card.hidden() && card.owner().session_key() == self.session_key)
            .count()
    }

    /// Supply the reopen picker/command with actual identities, not guessed
    /// session keys. Never includes a draft or a credential.
    #[must_use]
    pub fn hidden_owners(&self) -> Vec<ModalOwner> {
        self.cards
            .iter()
            .filter(|card| card.hidden() && card.owner().session_key() == self.session_key)
            .map(|card| card.owner().clone())
            .collect()
    }

    /// Stage the answer the user just made for the displayed question and
    /// advance to the next outstanding question. The last staged question
    /// opens the review step; nothing is sent before that.
    pub fn stage_current(&mut self) -> bool {
        let Some(q) = self.active_question_card_mut() else {
            return false;
        };
        if q.phase != SubmissionPhase::Editing || q.stage != QuestionnaireStage::Editing {
            return false;
        }
        let Some(dialog) = q.dialogs.get(q.cursor) else {
            return false;
        };
        let Some(answer) = answer_from_dialog(dialog) else {
            return false;
        };
        let question_id = q
            .questions
            .get(q.cursor)
            .map(|question| question.id.clone())
            .unwrap_or_default();
        if question_id.is_empty() {
            // An identity-less projection cannot be addressed by the server.
            return false;
        }
        let Some(slot) = q.drafts.get_mut(q.cursor) else {
            return false;
        };
        *slot = Some(ClarificationAnswerInput {
            question_id,
            answer,
        });
        let next = (0..q.questions.len()).find(|index| q.drafts[*index].is_none());
        match next {
            Some(next) => q.cursor = next,
            None => {
                q.stage = QuestionnaireStage::Review;
                q.review_cursor = 0;
            }
        }
        true
    }

    /// Submit every staged answer, in the order the questions were presented.
    /// Only the review step can send: an incomplete questionnaire stays local.
    pub fn submit(&mut self) -> Option<Submission> {
        let owner = self.active_owner()?.clone();
        let q = self.cards.iter_mut().find_map(|card| match card {
            Card::Question(q) if q.owner == owner => Some(q),
            _ => None,
        })?;
        if q.phase != SubmissionPhase::Editing || q.stage != QuestionnaireStage::Review {
            return None;
        }
        let answers: Vec<ClarificationAnswerInput> =
            q.drafts.iter().filter_map(|draft| draft.clone()).collect();
        if answers.is_empty() || answers.len() != q.questions.len() {
            return None;
        }
        let ModalOwner::Clarification {
            session_key,
            request_id,
        } = &q.owner
        else {
            return None;
        };
        if request_id.is_empty() || q.question_id.is_empty() {
            // No identity to guard the write with: stay read-only.
            return None;
        }
        self.next_nonce = self.next_nonce.wrapping_add(1);
        q.nonce = self.next_nonce;
        q.phase = SubmissionPhase::Submitting;
        Some(Submission {
            params: ClarificationActionParams {
                session_key: session_key.clone(),
                expected_request_id: request_id.clone(),
                expected_question_id: q.question_id.clone(),
                action: ClarificationAction::Submit { answers },
            },
            generation: self.generation,
            nonce: q.nonce,
        })
    }

    /// Discard the whole request. The action is scoped to the question that is
    /// on screen, so a cancel can never discard a different questionnaire.
    pub fn cancel_question(&mut self) -> Option<Submission> {
        let owner = self.active_owner()?.clone();
        let q = self.cards.iter_mut().find_map(|card| match card {
            Card::Question(q) if q.owner == owner => Some(q),
            _ => None,
        })?;
        if q.phase != SubmissionPhase::Editing {
            return None;
        }
        let ModalOwner::Clarification {
            session_key,
            request_id,
        } = &q.owner
        else {
            return None;
        };
        if request_id.is_empty() || q.question_id.is_empty() {
            return None;
        }
        self.next_nonce = self.next_nonce.wrapping_add(1);
        q.nonce = self.next_nonce;
        q.phase = SubmissionPhase::Submitting;
        Some(Submission {
            params: ClarificationActionParams {
                session_key: session_key.clone(),
                expected_request_id: request_id.clone(),
                expected_question_id: q.question_id.clone(),
                action: ClarificationAction::Cancel {},
            },
            generation: self.generation,
            nonce: q.nonce,
        })
    }

    /// The caller retains the exact ticket identity (not its secret reply) and
    /// feeds it back with the typed RPC result; no unsolicited response can
    /// close a successor. `None` means transport/response uncertainty: refresh
    /// pending before a user-initiated retry, never automatically resend.
    pub fn acknowledge(
        &mut self,
        ticket: &Submission,
        response: Option<&ClarificationActionResponse>,
    ) -> AckEffect {
        if ticket.generation != self.generation {
            return AckEffect::Ignored;
        }
        let Some(q) = self.cards.iter_mut().find_map(|card| match card {
            Card::Question(q)
                if q.nonce == ticket.nonce
                    && q.owner.session_key() == ticket.params.session_key
                    && matches!(&q.owner, ModalOwner::Clarification { request_id, .. }
                        if ticket.params.expected_request_id == *request_id)
                    && q.question_id == ticket.params.expected_question_id =>
            {
                Some(q)
            }
            _ => None,
        }) else {
            return AckEffect::Ignored;
        };
        if q.phase != SubmissionPhase::Submitting {
            return AckEffect::Ignored;
        }
        let Some(response) = response else {
            // Transport uncertainty: the write may or may not have landed.
            // Never replay it on our own.
            q.phase = SubmissionPhase::Error;
            return AckEffect::RefreshPending;
        };
        match response.status {
            ClarificationActionStatus::Cancelled => self.close(ticket),
            ClarificationActionStatus::Answered if response.pending_questions == 0 => {
                self.close(ticket)
            }
            ClarificationActionStatus::Answered
            | ClarificationActionStatus::Advanced
            | ClarificationActionStatus::Stale => {
                q.phase = SubmissionPhase::Reconciling;
                AckEffect::RefreshPending
            }
        }
    }

    /// A terminal answer retires the request identity, so no late frame can
    /// resurrect it.
    fn close(&mut self, ticket: &Submission) -> AckEffect {
        self.remember_retired(
            &ticket.params.session_key,
            &ticket.params.expected_request_id,
        );
        self.cards.retain(
            |card| !matches!(card, Card::Question(current) if current.nonce == ticket.nonce),
        );
        AckEffect::Closed
    }

    /// Successful, session-scoped pending snapshot: remove entries absent from
    /// the authoritative list and advance cursors. A failed snapshot must NOT
    /// call this method; it leaves every draft intact and visibly uncertain.
    /// Only one live Core clarification exists per session.
    /// Sessions whose drafts need reconciliation after a complete pending list.
    pub fn question_sessions(&self) -> Vec<String> {
        self.cards
            .iter()
            .filter_map(|c| match c {
                Card::Question(q) => Some(q.owner.session_key().to_owned()),
                _ => None,
            })
            .collect()
    }

    pub fn reconcile_pending(&mut self, session_key: &str, pending: Option<PendingClarification>) {
        let matching = pending.filter(|p| p.session_key == session_key);
        match matching {
            Some(p) => {
                let id = p.request_id.clone();
                let cursor = p.answered;
                self.offer_question(p);
                if let Some(Card::Question(q)) = self.cards.iter_mut().find(|card| {
                    matches!(card.owner(), ModalOwner::Clarification { session_key: key, request_id }
                        if key == session_key && request_id == &id)
                }) {
                    if q.answered == cursor {
                        if id.is_empty() || q.question_id.is_empty() {
                            if q.phase == SubmissionPhase::Error || q.phase == SubmissionPhase::Reconciling {
                                q.phase = SubmissionPhase::LegacyConfirmation;
                            }
                        } else if matches!(q.phase, SubmissionPhase::Reconciling | SubmissionPhase::Error) {
                            q.phase = SubmissionPhase::Editing;
                        }
                    }
                }
            }
            None => {
                let retired: Vec<String> = self
                    .cards
                    .iter()
                    .filter_map(|card| match card.owner() {
                        ModalOwner::Clarification {
                            session_key: key,
                            request_id,
                        } if key == session_key => Some(request_id.clone()),
                        _ => None,
                    })
                    .collect();
                for id in retired {
                    self.remember_retired(session_key, &id);
                }
                self.cards.retain(|card| {
                    !matches!(card.owner(), ModalOwner::Clarification { session_key: key, .. } if key == session_key)
                });
            }
        }
    }

    fn active_question_card(&self) -> Option<&QuestionCard> {
        let owner = self.active_owner()?;
        self.cards.iter().find_map(|card| match card {
            Card::Question(q) if &q.owner == owner => Some(q),
            _ => None,
        })
    }

    fn active_question_card_mut(&mut self) -> Option<&mut QuestionCard> {
        let owner = self.active_owner()?.clone();
        self.cards.iter_mut().find_map(|card| match card {
            Card::Question(q) if q.owner == owner => Some(q),
            _ => None,
        })
    }
}

impl QuestionCard {
    fn new(
        owner: ModalOwner,
        run_id: String,
        answered: usize,
        questions: Vec<AskUserQuestion>,
        pending: &PendingClarification,
        superseded: bool,
        nonce: u64,
    ) -> Self {
        let question_id = questions
            .first()
            .map(|question| question.id.clone())
            .unwrap_or_default();
        let has_identity = matches!(&owner, ModalOwner::Clarification { request_id, .. } if !request_id.is_empty())
            && !question_id.is_empty();
        let total = questions.len();
        let dialogs = questions
            .iter()
            .enumerate()
            .map(|(index, question)| dialog_for(pending, question, index))
            .collect();
        Self {
            owner,
            run_id,
            answered,
            question_id,
            questions,
            dialogs,
            drafts: vec![None; total],
            cursor: 0,
            review_cursor: 0,
            stage: QuestionnaireStage::Editing,
            phase: if has_identity {
                SubmissionPhase::Editing
            } else {
                SubmissionPhase::LegacyConfirmation
            },
            hidden: false,
            superseded,
            nonce,
        }
    }

    /// Adopt the drafts of the questions that are still outstanding after the
    /// cursor moved. Everything else is obsolete and is dropped.
    fn adopt_from(&mut self, old: &QuestionCard) {
        for (index, question) in self.questions.iter().enumerate() {
            if question.id.is_empty() {
                continue;
            }
            let Some(old_index) = old.questions.iter().position(|q| q.id == question.id) else {
                continue;
            };
            if let Some(draft) = old.drafts.get(old_index) {
                self.drafts[index] = draft.clone();
            }
            if let (Some(dialog), Some(old_dialog)) =
                (self.dialogs.get_mut(index), old.dialogs.get(old_index))
            {
                dialog.input = old_dialog.input.clone();
                dialog.selected = old_dialog
                    .selected
                    .min(dialog.options.len().saturating_sub(1));
                dialog.checked.clone_from(&old_dialog.checked);
                dialog.typing = old_dialog.typing;
            }
        }
        self.cursor = self.cursor.min(self.questions.len().saturating_sub(1));
        self.review_cursor = self
            .review_cursor
            .min(self.questions.len().saturating_sub(1));
    }

    /// Refresh the outstanding list from a pending snapshot without touching
    /// the drafts of the questions that are still outstanding.
    fn rebuild(&mut self, pending: &PendingClarification, questions: Vec<AskUserQuestion>) {
        let old_questions = std::mem::replace(&mut self.questions, questions);
        let old_dialogs = std::mem::take(&mut self.dialogs);
        let old_drafts = std::mem::take(&mut self.drafts);
        let total = self.questions.len();
        self.dialogs = Vec::with_capacity(total);
        self.drafts = vec![None; total];
        for (index, question) in self.questions.iter().enumerate() {
            let mut dialog = dialog_for(pending, question, index);
            if !question.id.is_empty() {
                if let Some(old_index) = old_questions.iter().position(|q| q.id == question.id) {
                    if let Some(old_dialog) = old_dialogs.get(old_index) {
                        dialog.input = old_dialog.input.clone();
                        dialog.selected = old_dialog
                            .selected
                            .min(dialog.options.len().saturating_sub(1));
                        dialog.checked.clone_from(&old_dialog.checked);
                        dialog.typing = old_dialog.typing;
                    }
                    if let Some(old_draft) = old_drafts.get(old_index) {
                        self.drafts[index] = old_draft.clone();
                    }
                }
            }
            self.dialogs.push(dialog);
        }
        self.question_id = self
            .questions
            .first()
            .map(|question| question.id.clone())
            .unwrap_or_default();
        self.cursor = self.cursor.min(total.saturating_sub(1));
        self.review_cursor = self.review_cursor.min(total.saturating_sub(1));
        if total == 0 || self.drafts.iter().any(|draft| draft.is_none()) {
            self.stage = QuestionnaireStage::Editing;
        }
    }
}

/// The outstanding questions of a pending projection, in presented order.
///
/// A frame without a structured question list degrades to the flat pair, which
/// has no identity of its own: such a card stays read-only until a snapshot
/// supplies both identities.
fn outstanding_questions(pending: &PendingClarification) -> Vec<AskUserQuestion> {
    if pending.questions.is_empty() {
        return vec![AskUserQuestion {
            id: String::new(),
            header: None,
            prompt: pending.question.clone(),
            options: pending
                .options
                .iter()
                .map(|label| AskUserOption {
                    label: label.clone(),
                    description: None,
                    preview: None,
                })
                .collect(),
            multi_select: false,
            secret: false,
        }];
    }
    pending
        .questions
        .get(pending.answered..)
        .unwrap_or_default()
        .to_vec()
}

/// One independent buffer per outstanding question.
fn dialog_for(
    pending: &PendingClarification,
    question: &AskUserQuestion,
    index: usize,
) -> DialogState {
    // The position is the one the user sees: the question's place in the
    // whole questionnaire, not in the remaining ones.
    let overall = if pending.questions.is_empty() {
        1
    } else {
        pending.questions.len()
    };
    let base = pending.answered.min(overall.saturating_sub(1));
    let position = if overall > 1 {
        format!("{}/{} · ", base + index + 1, overall)
    } else {
        String::new()
    };
    let header = question
        .header
        .as_deref()
        .filter(|h| !h.is_empty())
        .map(|h| format!("[{h}] "))
        .unwrap_or_default();
    let hint = if question.multi_select {
        " (multiple)"
    } else {
        ""
    };
    let options: Vec<String> = question
        .options
        .iter()
        .map(
            |option| match option.description.as_deref().filter(|d| !d.is_empty()) {
                Some(description) => format!("{} — {description}", option.label),
                None => option.label.clone(),
            },
        )
        .collect();
    let typing = options.is_empty();
    DialogState {
        question: format!("{position}{header}{}{hint}", question.prompt),
        options,
        selected: 0,
        multi_select: question.multi_select,
        checked: Vec::new(),
        secret: question.secret,
        input: String::new(),
        typing,
    }
}

/// The answer the displayed question would stage right now.
///
/// A typed buffer is text, verbatim; a menu highlight is an explicit choice.
/// The two never collapse into one another: a typed "2" stays the text "2".
fn answer_from_dialog(dialog: &DialogState) -> Option<ClarificationInput> {
    if dialog.typing {
        return (!dialog.input.trim().is_empty()).then(|| ClarificationInput::Text {
            text: dialog.input.clone(),
        });
    }
    if dialog.multi_select {
        return (!dialog.checked.is_empty()).then(|| ClarificationInput::Choices {
            indices: dialog.checked.clone(),
        });
    }
    Some(ClarificationInput::Choices {
        indices: vec![u32::try_from(dialog.staged_choice()?).ok()?],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::AskUserOption;

    fn question(
        id: &str,
        prompt: &str,
        options: &[&str],
        secret: bool,
        multi_select: bool,
    ) -> AskUserQuestion {
        AskUserQuestion {
            id: id.into(),
            header: None,
            prompt: prompt.into(),
            options: options
                .iter()
                .map(|label| AskUserOption {
                    label: (*label).into(),
                    description: None,
                    preview: None,
                })
                .collect(),
            multi_select,
            secret,
        }
    }

    fn pending(session: &str, id: &str, run: &str, secret: bool) -> PendingClarification {
        PendingClarification {
            session_key: session.into(),
            request_id: id.into(),
            run_id: run.into(),
            question: "Secret?".into(),
            options: vec![],
            questions: vec![AskUserQuestion {
                header: Some("Access".into()),
                options: vec![AskUserOption {
                    label: "yes".into(),
                    description: Some("approved".into()),
                    preview: None,
                }],
                ..question("q1", "Secret?", &[], secret, false)
            }],
            answered: 0,
        }
    }

    fn two_questions(session: &str, id: &str, run: &str) -> PendingClarification {
        let mut pending = pending(session, id, run, false);
        pending.questions = vec![
            question("q1", "Pick one", &["alpha", "bravo"], false, false),
            question("q2", "How many?", &[], false, false),
        ];
        pending
    }

    fn answered(
        status: ClarificationActionStatus,
        pending_questions: usize,
    ) -> ClarificationActionResponse {
        ClarificationActionResponse {
            status,
            resolved: matches!(
                status,
                ClarificationActionStatus::Advanced | ClarificationActionStatus::Answered
            ),
            pending_questions,
            stale: false,
        }
    }

    /// Stage the displayed question and open the review step.
    fn stage_and_review(c: &mut InteractionController) {
        assert!(c.stage_current());
        c.active_dialog_mut().unwrap().input = "42".into();
        assert!(c.stage_current());
        assert!(c.in_review());
    }

    #[test]
    fn one_owner_controls_display_and_edits_while_other_modal_queues() {
        let mut c = InteractionController::new("s");
        c.offer_approval("s", "approve");
        c.offer_question(pending("s", "r1", "run", false));
        assert_eq!(
            c.active_owner(),
            Some(&ModalOwner::Approval {
                session_key: "s".into(),
                id: "approve".into()
            })
        );
        assert!(c.active_dialog_mut().is_none());
        c.retire_approval("s", "approve");
        assert!(
            matches!(c.active_owner(), Some(ModalOwner::Clarification { request_id, .. }) if request_id == "r1")
        );
        assert!(c.active_dialog().unwrap().question.contains("[Access]"));
        assert_eq!(c.active_dialog().unwrap().options[0], "yes — approved");
        assert_eq!(c.active_cancel_run(), Some("run"));
        c.offer_approval("s", "later");
        assert!(matches!(
            c.active_owner(),
            Some(ModalOwner::Clarification { .. })
        ));
        let owner = c.hide_active().unwrap();
        assert!(matches!(c.active_owner(), Some(ModalOwner::Approval { id, .. }) if id == "later"));
        assert_eq!(c.hidden_count(), 1);
        c.retire_approval("s", "later");
        assert!(c.active_owner().is_none());
        assert!(c.reopen(&owner));
        assert_eq!(c.active_owner(), Some(&owner));
    }

    #[test]
    fn fifo_pause_and_reopen_keep_each_sessions_draft() {
        let mut c = InteractionController::new("s1");
        c.offer_question(pending("s1", "first", "run-1", true));
        c.active_dialog_mut().unwrap().input = "private".into();
        c.offer_approval("s1", "second");
        c.offer_question(pending("s2", "third", "run-2", false));
        c.pause_for_picker(true);
        assert!(c.active_owner().is_none());
        c.pause_for_picker(false);
        assert!(
            matches!(c.active_owner(), Some(ModalOwner::Clarification { request_id, .. }) if request_id == "first")
        );
        c.set_context("s2", 1);
        assert!(
            matches!(c.active_owner(), Some(ModalOwner::Clarification { request_id, .. }) if request_id == "first")
        );
        c.set_context("s1", 2);
        assert_eq!(c.active_dialog().unwrap().input, "private");
        let first = c.hide_active().unwrap();
        assert!(
            matches!(c.active_owner(), Some(ModalOwner::Approval { id, .. }) if id == "second")
        );
        c.retire_approval("s1", "second");
        assert!(
            matches!(c.active_owner(), Some(ModalOwner::Clarification { request_id, .. }) if request_id == "third")
        );
        assert!(c.reopen(&first));
        assert_eq!(c.active_dialog().unwrap().input, "private");
    }

    #[test]
    fn same_session_supersede_replaces_draft_and_late_ack_or_end_does_not_close_it() {
        let mut c = InteractionController::new("s");
        c.offer_question(pending("s", "old", "run-old", true));
        c.active_dialog_mut().unwrap().input = "top-secret".into();
        c.active_dialog_mut().unwrap().typing = true;
        assert!(c.stage_current());
        let old = c.submit().unwrap();
        c.offer_question(pending("s", "new", "run-new", true));
        assert!(c.active_was_superseded());
        assert!(c.active_dialog().unwrap().input.is_empty());
        assert_eq!(
            c.acknowledge(
                &old,
                Some(&answered(ClarificationActionStatus::Answered, 0))
            ),
            AckEffect::Ignored
        );
        c.end_question("s", "old");
        c.offer_question(pending("s", "old", "run-old", true));
        assert!(
            matches!(c.active_owner(), Some(ModalOwner::Clarification { request_id, .. }) if request_id == "new")
        );
        assert_eq!(c.active_cancel_run(), Some("run-new"));
    }

    #[test]
    fn submission_is_guarded_and_closes_only_on_exact_success() {
        let mut c = InteractionController::new("s");
        c.set_context("s", 7);
        c.offer_question(pending("s", "req", "run", false));
        assert!(c.stage_current());
        assert!(c.in_review());
        let ticket = c.submit().unwrap();
        assert!(ticket.rpc_identity().starts_with("clarification:7:"));
        assert_eq!(ticket.params.expected_request_id, "req");
        assert_eq!(ticket.params.expected_question_id, "q1");
        assert!(c.submit().is_none());
        assert_eq!(
            c.acknowledge(
                &ticket,
                Some(&ClarificationActionResponse {
                    status: ClarificationActionStatus::Stale,
                    resolved: false,
                    pending_questions: 0,
                    stale: true,
                })
            ),
            AckEffect::RefreshPending
        );
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Reconciling));
        assert!(c.submit().is_none());
        c.reconcile_pending("s", Some(pending("s", "req", "run", false)));
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Editing));
        assert!(c.in_review());
        let ticket = c.submit().unwrap();
        assert_eq!(
            c.acknowledge(
                &ticket,
                Some(&answered(ClarificationActionStatus::Advanced, 1))
            ),
            AckEffect::RefreshPending
        );
        c.reconcile_pending("s", Some(pending("s", "req", "run", false)));
        assert!(c.in_review());
        let ticket = c.submit().unwrap();
        assert_eq!(
            c.acknowledge(
                &ticket,
                Some(&answered(ClarificationActionStatus::Answered, 0))
            ),
            AckEffect::Closed
        );
        assert!(c.active_owner().is_none());
        c.offer_question(pending("s", "req", "run", false));
        assert!(c.active_owner().is_none()); // Late frame cannot reopen completed request.
    }

    #[test]
    fn error_keeps_secret_draft_but_never_echoes_it_and_snapshot_is_not_resend() {
        let mut c = InteractionController::new("s");
        c.offer_question(pending("s", "req", "run", true));
        c.active_dialog_mut().unwrap().typing = true;
        c.active_dialog_mut().unwrap().input = "top-secret".into();
        assert!(c.stage_current());
        let ticket = c.submit().unwrap();
        match &ticket.params.action {
            ClarificationAction::Submit { answers } => {
                assert_eq!(answers.len(), 1);
                match &answers[0].answer {
                    ClarificationInput::Text { text } => assert_eq!(text, "top-secret"),
                    other => panic!("expected text, got {other:?}"),
                }
            }
            other => panic!("expected a submit, got {other:?}"),
        }
        assert_eq!(c.acknowledge(&ticket, None), AckEffect::RefreshPending);
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Error));
        assert!(c.submit().is_none()); // Refresh pending before any manual retry.
        c.reconcile_pending("s", Some(pending("s", "req", "run", true)));
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Editing));
        assert!(
            c.in_review(),
            "reconciliation keeps the review ready for an explicit retry"
        );
        assert_eq!(c.review_rows()[0].value, FILLED_MARKER);
        assert_eq!(c.active_cancel_run(), Some("run"));
        c.end_question("s", "req");
        assert!(c.active_dialog().is_none());
    }

    #[test]
    fn session_and_generation_guard_late_results_and_keep_other_drafts() {
        let mut c = InteractionController::new("s1");
        c.offer_question(pending("s1", "r1", "run1", true));
        c.active_dialog_mut().unwrap().input = "secret-s1".into();
        c.active_dialog_mut().unwrap().typing = true;
        assert!(c.stage_current());
        let old = c.submit().unwrap();
        c.set_context("s2", 1);
        c.offer_question(pending("s2", "r2", "", false));
        assert_eq!(c.active_cancel_run(), Some("run1"));
        assert_eq!(
            c.acknowledge(
                &old,
                Some(&answered(ClarificationActionStatus::Answered, 0))
            ),
            AckEffect::Ignored
        );
        c.set_context("s1", 2);
        assert_eq!(c.active_dialog().unwrap().input, "secret-s1");
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Reconciling));
        c.reconcile_pending("s1", None);
        assert_eq!(
            c.active_cancel_run(),
            None,
            "the queued identity-less run must never target the previous task"
        );
        c.set_context("s2", 3);
        assert!(c.active_dialog().is_some());
    }

    #[test]
    fn wire_frame_and_late_terminal_cannot_steal_new_identity() {
        let mut c = InteractionController::new("s");
        let old = StreamEvent::AskUser {
            run_id: "run-1".into(),
            seq: 1,
            session_key: "s".into(),
            request_id: "old".into(),
            question: "Old?".into(),
            options: vec![],
            questions: pending("s", "old", "run-1", false).questions,
            answered: 0,
        };
        assert!(!c.observe_event(&old));
        c.offer_question(pending("s", "new", "run-2", false));
        assert!(!c.observe_event(&old));
        assert!(!c.observe_event(&StreamEvent::ClarificationEnded {
            session_key: "s".into(),
            request_id: "old".into(),
            outcome: "cancelled".into(),
        }));
        assert!(
            matches!(c.active_owner(), Some(ModalOwner::Clarification { request_id, .. }) if request_id == "new")
        );
        assert!(c.observe_event(&StreamEvent::ClarificationEnded {
            session_key: "s".into(),
            request_id: String::new(),
            outcome: "expired".into(),
        }));
        assert!(c.active_dialog().is_some());
        let missing_session = StreamEvent::AskUser {
            run_id: "run".into(),
            seq: 2,
            session_key: String::new(),
            request_id: String::new(),
            question: "Legacy?".into(),
            options: vec![],
            questions: vec![],
            answered: 0,
        };
        assert!(c.observe_event(&missing_session));
        assert!(
            matches!(c.active_owner(), Some(ModalOwner::Clarification { request_id, .. }) if request_id == "new")
        );
    }

    #[test]
    fn conflicting_push_does_not_retire_the_snapshot_confirmed_request() {
        let mut c = InteractionController::new("s");
        c.reconcile_pending("s", Some(pending("s", "current", "run-b", false)));
        c.active_dialog_mut().unwrap().input = "draft".into();
        let late = StreamEvent::AskUser {
            run_id: "run-a".into(),
            seq: 1,
            session_key: "s".into(),
            request_id: "late".into(),
            question: "Old?".into(),
            options: vec![],
            questions: pending("s", "late", "run-a", false).questions,
            answered: 0,
        };
        let needs_refresh = c.observe_event(&late);
        c.reconcile_pending("s", Some(pending("s", "current", "run-b", false)));
        assert!(
            matches!(c.active_owner(), Some(ModalOwner::Clarification { request_id, .. }) if request_id == "current")
        );
        assert_eq!(c.active_dialog().unwrap().input, "draft");
        assert!(needs_refresh);
    }

    #[test]
    fn sequential_snapshot_advances_cursor_without_replaying_old_answer() {
        let mut c = InteractionController::new("s");
        let first = two_questions("s", "req", "run");
        c.offer_question(first.clone());
        stage_and_review(&mut c);
        let old = c.submit().unwrap();
        let mut second = first.clone();
        second.answered = 1;
        c.reconcile_pending("s", Some(second));
        assert!(c.active_dialog().unwrap().question.contains("2/2"));
        assert_eq!(
            c.acknowledge(
                &old,
                Some(&answered(ClarificationActionStatus::Answered, 0))
            ),
            AckEffect::Ignored
        );
        c.offer_question(first);
        assert!(c.active_dialog().unwrap().question.contains("How many?"));
        c.active_dialog_mut().unwrap().input = "second answer".into();
        assert!(c.stage_current());
        let next = c.submit().unwrap();
        assert_eq!(next.params.expected_question_id, "q2");
        match &next.params.action {
            ClarificationAction::Submit { answers } => {
                assert_eq!(answers.len(), 1);
                assert_eq!(answers[0].question_id, "q2");
            }
            other => panic!("expected a submit, got {other:?}"),
        }
    }

    #[test]
    fn free_text_and_multi_select_keep_core_as_single_interpreter() {
        let mut c = InteractionController::new("s");
        let mut batch = pending("s", "req", "run", false);
        batch.questions[0].options.clear();
        batch.questions[0].multi_select = true;
        c.offer_question(batch);
        assert!(c.active_dialog().unwrap().typing);
        assert!(!c.active_dialog().unwrap().has_quick_pick());
        assert!(!c.stage_current(), "an empty answer is not an answer");
        c.active_dialog_mut().unwrap().input = "1,2".into();
        assert!(c.stage_current());
        let ticket = c.submit().unwrap();
        match &ticket.params.action {
            ClarificationAction::Submit { answers } => match &answers[0].answer {
                ClarificationInput::Text { text } => assert_eq!(text, "1,2"),
                other => panic!("expected text, got {other:?}"),
            },
            other => panic!("expected a submit, got {other:?}"),
        }
    }

    #[test]
    fn multi_select_stages_checked_indices_instead_of_comma_text() {
        let mut c = InteractionController::new("s");
        let mut batch = pending("s", "req", "run", false);
        batch.questions = vec![question(
            "q1",
            "Pick several",
            &["a", "b", "c"],
            false,
            true,
        )];
        c.offer_question(batch);
        assert!(
            !c.stage_current(),
            "highlighting alone cannot stage a multi-select"
        );
        let dialog = c.active_dialog_mut().unwrap();
        dialog.selected = 2;
        dialog.toggle_choice();
        dialog.selected = 1;
        dialog.toggle_choice();
        dialog.toggle_choice();
        dialog.selected = 0;
        dialog.toggle_choice();
        assert!(c.stage_current());
        assert_eq!(c.review_rows()[0].value, "c, a");
        let ticket = c.submit().unwrap();
        assert_eq!(
            ticket.params.action,
            ClarificationAction::Submit {
                answers: vec![ClarificationAnswerInput {
                    question_id: "q1".into(),
                    answer: ClarificationInput::Choices {
                        indices: vec![2, 0]
                    },
                }],
            }
        );
    }
    #[test]
    fn legacy_projection_is_read_only_until_the_snapshot_supplies_both_ids() {
        let mut c = InteractionController::new("s");
        let mut old = pending("s", "", "", false);
        old.questions.clear();
        c.offer_question(old.clone());
        assert_eq!(c.active_phase(), Some(SubmissionPhase::LegacyConfirmation));
        assert!(!c.stage_current());
        assert!(c.submit().is_none());
        assert!(c.cancel_question().is_none());
        c.reconcile_pending("s", Some(old));
        assert_eq!(c.active_phase(), Some(SubmissionPhase::LegacyConfirmation));
        assert!(c.submit().is_none());
        // The snapshot supplies the identities: the card becomes addressable.
        c.reconcile_pending("s", Some(pending("s", "req", "run", false)));
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Editing));
        assert!(c.stage_current());
        assert!(c.submit().is_some());
    }

    /// The red/green coverage of Task 4: staging, review, and an addressed
    /// cancel.
    #[test]
    fn questionnaire_stage_review_and_cancel_are_addressed() {
        let mut c = InteractionController::new("s");
        c.offer_question(two_questions("s", "req", "run"));
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Editing));

        // Q1: the highlighted choice is staged.
        assert!(c.stage_current());
        assert!(!c.in_review(), "the review step waits for every question");
        assert!(
            c.submit().is_none(),
            "an incomplete questionnaire stays local"
        );
        assert_eq!(c.active_dialog().unwrap().question.contains("2/2"), true);

        // Q2: the exact custom text is staged.
        c.active_dialog_mut().unwrap().input = "42".into();
        assert!(c.stage_current());
        assert!(c.in_review());

        let rows = c.review_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].value, "alpha");
        assert_eq!(rows[1].value, "42");

        // The review step submits the ordered question ids exactly once.
        let ticket = c.submit().unwrap();
        assert_eq!(ticket.params.session_key, "s");
        assert_eq!(ticket.params.expected_request_id, "req");
        assert_eq!(ticket.params.expected_question_id, "q1");
        match &ticket.params.action {
            ClarificationAction::Submit { answers } => {
                assert_eq!(answers.len(), 2);
                assert_eq!(answers[0].question_id, "q1");
                assert_eq!(answers[1].question_id, "q2");
                assert!(matches!(
                    answers[0].answer,
                    ClarificationInput::Choices { .. }
                ));
                assert!(matches!(answers[1].answer, ClarificationInput::Text { .. }));
            }
            other => panic!("expected a submit, got {other:?}"),
        }
        assert!(c.submit().is_none(), "a questionnaire is submitted once");

        // A staged answer can still be changed before the submission.
        let mut c = InteractionController::new("s");
        c.offer_question(two_questions("s", "req", "run"));
        stage_and_review(&mut c);
        assert!(c.edit_review_row());
        assert_eq!(
            c.active_dialog().unwrap().question.contains("Pick one"),
            true
        );
        c.active_dialog_mut().unwrap().selected = 1;
        assert!(c.stage_current());
        assert!(c.in_review());
        let ticket = c.submit().unwrap();
        match &ticket.params.action {
            ClarificationAction::Submit { answers } => match &answers[0].answer {
                ClarificationInput::Choices { indices } => assert_eq!(indices, &vec![1]),
                other => panic!("expected choices, got {other:?}"),
            },
            other => panic!("expected a submit, got {other:?}"),
        }

        // The explicit cancel is scoped to the question that is displayed.
        let mut c = InteractionController::new("s");
        c.offer_question(two_questions("s", "req", "run"));
        let ticket = c.cancel_question().unwrap();
        assert_eq!(ticket.params.expected_request_id, "req");
        assert_eq!(ticket.params.expected_question_id, "q1");
        assert!(matches!(
            ticket.params.action,
            ClarificationAction::Cancel {}
        ));
    }

    /// A secret answer is reviewed as a filled marker, never as its
    /// cleartext.
    #[test]
    fn secret_review_shows_a_filled_marker_and_never_the_credential() {
        let mut c = InteractionController::new("s");
        let mut secret = pending("s", "req", "run", true);
        secret.questions[0].options.clear();
        c.offer_question(secret);
        c.active_dialog_mut().unwrap().input = "top-secret".into();
        assert!(c.stage_current());
        assert!(c.in_review());
        let rows = c.review_rows();
        assert!(rows[0].secret);
        assert_ne!(rows[0].value, "top-secret");
        assert!(rows[0].value.contains("filled"));
    }

    /// A terminal answer closes its own request and nothing else: a stale
    /// fetch and a late acknowledgement cannot touch the next questionnaire or
    /// resurrect the finished one.
    #[test]
    fn questionnaire_terminal_and_uncertain_ack_do_not_retarget() {
        let mut c = InteractionController::new("s");
        c.offer_question(two_questions("s", "a", "run-a"));
        stage_and_review(&mut c);
        let ticket = c.submit().unwrap();

        // An uncertain write is never replayed on our own.
        assert_eq!(c.acknowledge(&ticket, None), AckEffect::RefreshPending);
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Error));
        assert!(c.submit().is_none());
        assert!(c.cancel_question().is_none());

        // Reconciling the same live cursor keeps the draft and sends nothing.
        c.reconcile_pending("s", Some(two_questions("s", "a", "run-a")));
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Editing));
        assert!(
            c.in_review(),
            "the staged answer survived the reconciliation"
        );
        let rows = c.review_rows();
        assert_eq!(rows[0].value, "alpha");
        assert_eq!(rows[1].value, "42");

        // The manual retry reaches the server and closes the request.
        let retry = c.submit().unwrap();
        assert_ne!(retry.rpc_identity(), ticket.rpc_identity());
        assert_eq!(
            c.acknowledge(
                &ticket,
                Some(&answered(ClarificationActionStatus::Answered, 0))
            ),
            AckEffect::Ignored
        );
        assert_eq!(
            c.acknowledge(
                &retry,
                Some(&answered(ClarificationActionStatus::Answered, 0))
            ),
            AckEffect::Closed
        );
        assert!(c.active_owner().is_none());

        // The terminal request is retired: a late frame cannot resurrect it.
        c.offer_question(two_questions("s", "a", "run-a"));
        assert!(c.active_owner().is_none());

        // A late acknowledgement of the finished request is ignored.
        assert_eq!(
            c.acknowledge(
                &ticket,
                Some(&answered(ClarificationActionStatus::Answered, 0))
            ),
            AckEffect::Ignored
        );

        // The next questionnaire is a different request and is untouched by
        // anything that belonged to the finished one.
        c.offer_question(two_questions("s", "b", "run-b"));
        assert!(matches!(
            c.active_owner(),
            Some(ModalOwner::Clarification { request_id, .. }) if request_id == "b"
        ));
        assert_eq!(
            c.acknowledge(
                &retry,
                Some(&answered(ClarificationActionStatus::Cancelled, 0))
            ),
            AckEffect::Ignored
        );
        assert!(matches!(
            c.active_owner(),
            Some(ModalOwner::Clarification { request_id, .. }) if request_id == "b"
        ));

        // A stale fetch of the finished request does not retarget the new one.
        c.offer_question(two_questions("s", "a", "run-a"));
        assert!(matches!(
            c.active_owner(),
            Some(ModalOwner::Clarification { request_id, .. }) if request_id == "b"
        ));
    }

    /// A cancelled request is terminal too, and closes only its own card.
    #[test]
    fn a_cancelled_questionnaire_is_closed_by_the_cancel_status() {
        let mut c = InteractionController::new("s");
        c.offer_question(two_questions("s", "req", "run"));
        let ticket = c.cancel_question().unwrap();
        assert_eq!(
            c.acknowledge(
                &ticket,
                Some(&answered(ClarificationActionStatus::Cancelled, 0))
            ),
            AckEffect::Closed
        );
        assert!(c.active_owner().is_none());
        c.offer_question(two_questions("s", "req", "run"));
        assert!(c.active_owner().is_none());
    }
}
