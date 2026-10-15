//! Pure modal presentation and clarification submission controller.
//!
//! No RPC, persistence, Core interpretation, or logging lives here. Both
//! drawing and key routing must consult `active_owner()`; AppState integration
//! is deliberately deferred until the other worker has finished app/*.rs.

use std::collections::VecDeque;

use aleph_protocol::{
    AskUserQuestion, ClarificationResolveParams, ClarificationResolveResponse,
    PendingClarification, StreamEvent,
};

use super::app::{ApprovalState, DialogState};

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
    fn session_key(&self) -> &str {
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
    /// Older servers cannot protect against same-session supersede. Explicit
    /// confirmation is required after the pending snapshot has been inspected.
    LegacyConfirmation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckEffect {
    Ignored,
    Closed,
    RefreshPending,
}

/// The reply may be a credential. Keep this value only in the in-flight RPC;
/// deliberately do not derive Debug, Serialize, or display it in diagnostics.
pub struct Submission {
    pub params: ClarificationResolveParams,
    pub generation: u64,
    pub run_id: String,
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
    question_id: String,
    dialog: DialogState,
    phase: SubmissionPhase,
    hidden: bool,
    superseded: bool,
    /// Identity-less frames require a successful pending snapshot before the
    /// user can explicitly accept their weaker legacy semantics.
    snapshot_confirmed: bool,
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
        self.cards
            .iter()
            .find(|c| c.owner().session_key() == self.session_key && !c.hidden())
            .map(Card::owner)
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
            Card::Question(q) if &q.owner == owner => Some(&q.dialog),
            _ => None,
        })
    }

    /// A mutable draft is available only for the question actually displayed.
    /// Key routing must use this accessor, never search by session alone.
    pub fn active_dialog_mut(&mut self) -> Option<&mut DialogState> {
        let owner = self.active_owner()?.clone();
        self.cards.iter_mut().find_map(|card| match card {
            Card::Question(q) if q.owner == owner && q.phase != SubmissionPhase::Submitting => {
                Some(&mut q.dialog)
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

    pub fn retire_approval(&mut self, session_key: &str, id: &str) {
        self.cards.retain(|card| {
            !matches!(card.owner(), ModalOwner::Approval { session_key: key, id: current }
                if key == session_key && current == id)
        });
    }

    /// Offer a frame's public projection. Same-session supersede replaces its
    /// predecessor in place; a delayed frame with an older cursor is ignored.
    /// Legacy empty IDs are accepted only for display/reconciliation, never
    /// silently considered protected against supersede.
    pub fn offer_question(&mut self, pending: PendingClarification) {
        let owner = ModalOwner::Clarification {
            session_key: pending.session_key.clone(),
            request_id: pending.request_id.clone(),
        };
        let Some((question_id, dialog)) = dialog_from_pending(&pending) else {
            return;
        };
        if self
            .retired
            .iter()
            .any(|(session, id)| session == &pending.session_key && id == &pending.request_id)
        {
            return;
        }
        if let Some(index) = self.cards.iter().position(|card| {
            matches!(card.owner(), ModalOwner::Clarification { session_key, .. }
                if session_key == &pending.session_key)
        }) {
            let Card::Question(existing) = &mut self.cards[index] else {
                unreachable!("clarification owner must have a question card")
            };
            if existing.owner == owner {
                if pending.answered < existing.answered {
                    return;
                }
                if pending.answered == existing.answered && existing.question_id == question_id {
                    if !pending.run_id.is_empty() {
                        existing.run_id = pending.run_id;
                    }
                    return; // Duplicate frame must not overwrite an in-progress draft.
                }
            }
            let superseded = existing.owner != owner;
            let retired = if superseded {
                match &existing.owner {
                    ModalOwner::Clarification {
                        session_key,
                        request_id,
                    } => Some((session_key.clone(), request_id.clone())),
                    ModalOwner::Approval { .. } => None,
                }
            } else {
                None
            };
            if let Some((session, id)) = retired {
                self.remember_retired(&session, &id);
                // The old card's secret draft is dropped by replacement.
            }
            self.next_nonce = self.next_nonce.wrapping_add(1);
            self.cards[index] = Card::Question(QuestionCard::new(
                owner,
                pending.run_id,
                pending.answered,
                question_id,
                dialog,
                superseded,
                self.next_nonce,
            ));
        } else {
            self.next_nonce = self.next_nonce.wrapping_add(1);
            self.cards.push_back(Card::Question(QuestionCard::new(
                owner,
                pending.run_id,
                pending.answered,
                question_id,
                dialog,
                false,
                self.next_nonce,
            )));
        }
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

    /// Consume the wire events without duplicating their public shape in the
    /// app. Returns true when a legacy terminal needs an authoritative pending
    /// refresh (it has no identity with which to safely close a modal).
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
                    return true; // Legacy frame: discover session/request from pending.
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
                false
            }
            StreamEvent::ClarificationEnded {
                session_key,
                request_id,
                ..
            } => {
                if request_id.is_empty() {
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

    /// Legacy requires a successful pending snapshot AND an explicit action.
    pub fn confirm_legacy(&mut self, owner: &ModalOwner) -> bool {
        if self.active_owner() != Some(owner) {
            return false;
        }
        let Some(Card::Question(q)) = self.cards.iter_mut().find(|c| c.owner() == owner) else {
            return false;
        };
        if q.phase != SubmissionPhase::LegacyConfirmation || !q.snapshot_confirmed {
            return false;
        }
        q.phase = SubmissionPhase::Editing;
        true
    }

    /// Key handler edits the active `DialogState`; submission takes its exact
    /// raw reply and the guards. Core alone interprets indices/free text.
    pub fn submit(&mut self) -> Option<Submission> {
        let owner = self.active_owner()?.clone();
        let q = self.cards.iter_mut().find_map(|card| match card {
            Card::Question(q) if q.owner == owner => Some(q),
            _ => None,
        })?;
        if q.phase != SubmissionPhase::Editing {
            return None;
        }
        let reply = q.dialog.pending_reply()?;
        q.phase = SubmissionPhase::Submitting;
        let ModalOwner::Clarification {
            session_key,
            request_id,
        } = &q.owner
        else {
            unreachable!()
        };
        Some(Submission {
            params: ClarificationResolveParams {
                session_key: session_key.clone(),
                reply,
                answers: Vec::new(),
                expected_request_id: (!request_id.is_empty()).then(|| request_id.clone()),
                expected_question_id: (!q.question_id.is_empty()).then(|| q.question_id.clone()),
            },
            generation: self.generation,
            run_id: q.run_id.clone(),
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
        response: Option<&ClarificationResolveResponse>,
    ) -> AckEffect {
        if ticket.generation != self.generation {
            return AckEffect::Ignored;
        }
        let Some(q) = self.cards.iter_mut().find_map(|card| match card {
            Card::Question(q)
                if q.nonce == ticket.nonce
                    && q.owner.session_key() == ticket.params.session_key
                    && matches!(&q.owner, ModalOwner::Clarification { request_id, .. }
                        if ticket.params.expected_request_id.as_deref() == Some(request_id.as_str())
                            || (request_id.is_empty() && ticket.params.expected_request_id.is_none()))
                    && (q.question_id == ticket.params.expected_question_id.as_deref().unwrap_or("")) => Some(q),
            _ => None,
        }) else {
            return AckEffect::Ignored;
        };
        if q.phase != SubmissionPhase::Submitting {
            return AckEffect::Ignored;
        }
        if matches!(response, Some(r) if r.resolved && !r.stale && r.pending_questions == 0) {
            if let Some(request_id) = ticket.params.expected_request_id.as_deref() {
                self.remember_retired(&ticket.params.session_key, request_id);
            }
            self.cards.retain(
                |card| !matches!(card, Card::Question(current) if current.nonce == ticket.nonce),
            );
            AckEffect::Closed
        } else {
            q.phase = if response.is_some() {
                SubmissionPhase::Reconciling
            } else {
                SubmissionPhase::Error
            };
            AckEffect::RefreshPending
        }
    }

    /// Successful, session-scoped pending snapshot: remove entries absent from
    /// the authoritative list and advance cursors. A failed snapshot must NOT
    /// call this method; it leaves every draft intact and visibly uncertain.
    /// Only one live Core clarification exists per session.
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
                            q.snapshot_confirmed = true;
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
}

impl QuestionCard {
    fn new(
        owner: ModalOwner,
        run_id: String,
        answered: usize,
        question_id: String,
        dialog: DialogState,
        superseded: bool,
        nonce: u64,
    ) -> Self {
        let has_identity = matches!(&owner, ModalOwner::Clarification { request_id, .. } if !request_id.is_empty())
            && !question_id.is_empty();
        Self {
            owner,
            run_id,
            answered,
            question_id,
            dialog,
            phase: if has_identity {
                SubmissionPhase::Editing
            } else {
                SubmissionPhase::LegacyConfirmation
            },
            hidden: false,
            superseded,
            snapshot_confirmed: false,
            nonce,
        }
    }
}

fn dialog_from_pending(pending: &PendingClarification) -> Option<(String, DialogState)> {
    let current: Option<&AskUserQuestion> = pending.questions.get(pending.answered);
    if !pending.questions.is_empty() && current.is_none() {
        return None;
    }
    let (id, question, options, multi_select, secret) = if let Some(current) = current {
        let position = (pending.questions.len() > 1)
            .then(|| format!("{}/{} · ", pending.answered + 1, pending.questions.len()))
            .unwrap_or_default();
        let header = current
            .header
            .as_deref()
            .filter(|h| !h.is_empty())
            .map(|h| format!("[{h}] "))
            .unwrap_or_default();
        let hint = if current.multi_select {
            " (multiple)"
        } else {
            ""
        };
        (
            current.id.clone(),
            format!("{position}{header}{}{hint}", current.prompt),
            current
                .options
                .iter()
                .map(
                    |o| match o.description.as_deref().filter(|d| !d.is_empty()) {
                        Some(description) => format!("{} — {description}", o.label),
                        None => o.label.clone(),
                    },
                )
                .collect(),
            current.multi_select,
            current.secret,
        )
    } else {
        (
            String::new(),
            pending.question.clone(),
            pending.options.clone(),
            false,
            false,
        )
    };
    let typing = options.is_empty() || multi_select;
    Some((
        id,
        DialogState {
            session_key: pending.session_key.clone(),
            question,
            options,
            selected: 0,
            multi_select,
            secret,
            input: String::new(),
            typing,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::AskUserOption;

    fn pending(session: &str, id: &str, run: &str, secret: bool) -> PendingClarification {
        PendingClarification {
            session_key: session.into(),
            request_id: id.into(),
            run_id: run.into(),
            question: "Secret?".into(),
            options: vec![],
            questions: vec![AskUserQuestion {
                id: "q1".into(),
                header: Some("Access".into()),
                prompt: "Secret?".into(),
                options: vec![AskUserOption {
                    label: "yes".into(),
                    description: Some("approved".into()),
                }],
                multi_select: false,
                secret,
            }],
            answered: 0,
        }
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
            matches!(c.active_owner(), Some(ModalOwner::Clarification { request_id, .. }) if request_id == "third")
        );
        c.set_context("s1", 2);
        assert_eq!(c.active_dialog().unwrap().input, "private");
        let first = c.hide_active().unwrap();
        assert!(
            matches!(c.active_owner(), Some(ModalOwner::Approval { id, .. }) if id == "second")
        );
        c.retire_approval("s1", "second");
        assert!(c.active_owner().is_none());
        assert!(c.reopen(&first));
        assert_eq!(c.active_dialog().unwrap().input, "private");
    }

    #[test]
    fn same_session_supersede_replaces_draft_and_late_ack_or_end_does_not_close_it() {
        let mut c = InteractionController::new("s");
        c.offer_question(pending("s", "old", "run-old", true));
        c.active_dialog_mut().unwrap().input = "top-secret".into();
        c.active_dialog_mut().unwrap().typing = true;
        let old = c.submit().unwrap();
        c.offer_question(pending("s", "new", "run-new", true));
        assert!(c.active_was_superseded());
        assert!(c.active_dialog().unwrap().input.is_empty());
        assert_eq!(
            c.acknowledge(
                &old,
                Some(&ClarificationResolveResponse {
                    resolved: true,
                    stale: false,
                    pending_questions: 0
                })
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
        let ticket = c.submit().unwrap();
        assert_eq!(ticket.params.reply, "1");
        assert!(ticket.rpc_identity().starts_with("clarification:7:"));
        assert_eq!(ticket.params.expected_request_id.as_deref(), Some("req"));
        assert_eq!(ticket.params.expected_question_id.as_deref(), Some("q1"));
        assert!(c.submit().is_none());
        assert_eq!(
            c.acknowledge(
                &ticket,
                Some(&ClarificationResolveResponse {
                    resolved: true,
                    stale: true,
                    pending_questions: 0
                })
            ),
            AckEffect::RefreshPending
        );
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Reconciling));
        assert!(c.submit().is_none());
        c.reconcile_pending("s", Some(pending("s", "req", "run", false)));
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Editing));
        let ticket = c.submit().unwrap();
        assert_eq!(
            c.acknowledge(
                &ticket,
                Some(&ClarificationResolveResponse {
                    resolved: true,
                    stale: false,
                    pending_questions: 1
                })
            ),
            AckEffect::RefreshPending
        );
        c.reconcile_pending("s", Some(pending("s", "req", "run", false)));
        let ticket = c.submit().unwrap();
        assert_eq!(
            c.acknowledge(
                &ticket,
                Some(&ClarificationResolveResponse {
                    resolved: true,
                    stale: false,
                    pending_questions: 0
                })
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
        let ticket = c.submit().unwrap();
        assert_eq!(ticket.params.reply, "top-secret");
        assert_eq!(c.acknowledge(&ticket, None), AckEffect::RefreshPending);
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Error));
        assert_eq!(c.active_dialog().unwrap().input, "top-secret");
        assert!(c.submit().is_none()); // Refresh pending before any manual retry.
        c.reconcile_pending("s", Some(pending("s", "req", "run", true)));
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Editing));
        assert_eq!(c.active_dialog().unwrap().input, "top-secret");
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
        let old = c.submit().unwrap();
        c.set_context("s2", 1);
        c.offer_question(pending("s2", "r2", "", false));
        assert_eq!(c.active_cancel_run(), None);
        assert_eq!(
            c.acknowledge(
                &old,
                Some(&ClarificationResolveResponse {
                    resolved: true,
                    stale: false,
                    pending_questions: 0
                })
            ),
            AckEffect::Ignored
        );
        c.set_context("s1", 2);
        assert_eq!(c.active_dialog().unwrap().input, "secret-s1");
        assert_eq!(c.active_phase(), Some(SubmissionPhase::Reconciling));
        c.reconcile_pending("s1", None);
        assert!(c.active_owner().is_none());
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
    fn sequential_snapshot_advances_cursor_without_replaying_old_answer() {
        let mut c = InteractionController::new("s");
        let mut first = pending("s", "req", "run", false);
        first.questions.push(AskUserQuestion {
            id: "q2".into(),
            header: None,
            prompt: "Second?".into(),
            options: vec![],
            multi_select: false,
            secret: false,
        });
        c.offer_question(first.clone());
        let old = c.submit().unwrap();
        let mut second = first.clone();
        second.answered = 1;
        c.reconcile_pending("s", Some(second));
        assert!(c.active_dialog().unwrap().question.contains("2/2"));
        assert_eq!(
            c.acknowledge(
                &old,
                Some(&ClarificationResolveResponse {
                    resolved: true,
                    stale: false,
                    pending_questions: 0,
                })
            ),
            AckEffect::Ignored
        );
        c.offer_question(first);
        assert!(c.active_dialog().unwrap().question.contains("Second?"));
        c.active_dialog_mut().unwrap().input = "second answer".into();
        let next = c.submit().unwrap();
        assert_eq!(next.params.expected_question_id.as_deref(), Some("q2"));
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
        assert!(c.submit().is_none());
        c.active_dialog_mut().unwrap().input = "1,2".into();
        let ticket = c.submit().unwrap();
        assert_eq!(ticket.params.reply, "1,2");
        assert_eq!(ticket.params.answers, Vec::<String>::new());
    }

    #[test]
    fn legacy_needs_snapshot_and_manual_confirmation_before_unguarded_submit() {
        let mut c = InteractionController::new("s");
        let mut old = pending("s", "", "", false);
        old.questions.clear();
        c.offer_question(old.clone());
        assert_eq!(c.active_phase(), Some(SubmissionPhase::LegacyConfirmation));
        let owner = c.active_owner().unwrap().clone();
        assert!(!c.confirm_legacy(&owner));
        assert!(c.submit().is_none());
        c.reconcile_pending("s", Some(old));
        assert!(c.confirm_legacy(&owner));
        c.active_dialog_mut().unwrap().input = "legacy answer".into();
        let ticket = c.submit().unwrap();
        assert_eq!(ticket.params.expected_request_id, None);
        assert_eq!(ticket.params.expected_question_id, None);
    }
}
