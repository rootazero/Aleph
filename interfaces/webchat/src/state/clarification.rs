//! Panel-side state for the parked `ask_user` questionnaire.
//!
//! One board owns the live questions, the answers staged for them, and the
//! fences that keep a late frame from resurrecting a request that already
//! ended. It is deliberately plain data with no reactive machinery: the rules
//! below are the ones the wire contract depends on, so they are unit-testable
//! without a browser.
//!
//! Three facts drive everything here:
//!
//! * `clarification.pending` is the authoritative snapshot; a push is a
//!   notification. A snapshot therefore replaces what it knows, but it must
//!   never overwrite a push that arrived while the snapshot was in flight.
//! * A terminal frame is matched by `(session_key, request_id)`. A frame
//!   without a request identity names nothing, so it triggers a snapshot
//!   instead of a removal.
//! * The cursor of one request only moves forward. A frame carrying an older
//!   cursor is stale and is ignored.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use aleph_protocol::clarification::{
    ClarificationAction, ClarificationActionParams, ClarificationAnswerInput, ClarificationInput,
    PendingClarification, PendingListResponse,
};

use crate::state::notifications::{AskQuestionView, PendingAskView};

/// How many ended requests stay fenced. The fence has to outlive the async
/// work that was already in flight when the request ended — a snapshot that
/// was fetched before the terminal frame must not bring the card back.
const ENDED_FENCE_CAPACITY: usize = 64;

/// A request that has ended. Kept so a late snapshot cannot resurrect it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EndedRequest {
    session_key: String,
    request_id: String,
}

/// A live question together with the generation that installed it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BoardEntry {
    view: PendingAskView,
    generation: u64,
}

/// What a push did to the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// The push is now the live view of its session.
    Installed,
    /// The push was fenced or carried an older cursor.
    Ignored,
    /// Different request IDs cannot be ordered without asking Core.
    NeedsSnapshot,
}

/// What a terminal frame did to the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndOutcome {
    /// The named request was live and is now gone.
    Removed,
    /// The named request was not the live one — a successor keeps the card.
    NotOwned,
    /// The frame named no request, so nothing can be removed. The caller must
    /// reconcile with a snapshot instead of guessing.
    NeedsSnapshot,
}

/// What a snapshot did to the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotOutcome {
    /// Entries the snapshot installed or refreshed.
    pub applied: usize,
    /// Entries kept because the board already knew something newer.
    pub kept: usize,
}

/// The live questionnaire state of one panel.
///
/// `Clone` shares the submission latches, so a card that is re-created while a
/// submission is in flight cannot start a second one.
#[derive(Debug, Clone, Default)]
pub struct ClarificationBoard {
    entries: Vec<BoardEntry>,
    /// Bumped on every applied change. A snapshot remembers the generation it
    /// was fetched at, so it can tell what happened while it was in flight.
    generation: u64,
    last_snapshot_start: u64,
    ended: Vec<EndedRequest>,
    /// Staged answers, keyed by request id and then by question id.
    drafts: HashMap<String, HashMap<String, ClarificationInput>>,
    /// One latch per request. A latch is per request, not global: a card of
    /// one session must not block the card of another.
    submissions: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

/// Whether two views name the same request.
///
/// A missing request id cannot identify a questionnaire.
fn same_request(a: &PendingAskView, b: &PendingAskView) -> bool {
    !a.request_id.is_empty() && a.request_id == b.request_id
}

/// Convert the shared wire projection into the panel's view type.
#[must_use]
pub fn view_from_pending(p: &PendingClarification) -> PendingAskView {
    PendingAskView {
        session_key: p.session_key.clone(),
        request_id: p.request_id.clone(),
        question: p.question.clone(),
        options: p.options.clone(),
        questions: p
            .questions
            .iter()
            .map(|q| AskQuestionView {
                id: q.id.clone(),
                header: q.header.clone(),
                prompt: q.prompt.clone(),
                options: q
                    .options
                    .iter()
                    .map(|o| crate::state::notifications::AskOptionView {
                        label: o.label.clone(),
                        description: o.description.clone(),
                    })
                    .collect(),
                multi_select: q.multi_select,
                secret: q.secret,
            })
            .collect(),
        answered: p.answered,
    }
}

impl ClarificationBoard {
    /// The live questions, one per session.
    #[must_use]
    pub fn pending(&self) -> Vec<PendingAskView> {
        self.entries.iter().map(|e| e.view.clone()).collect()
    }

    /// The live question of one session, if any.
    #[must_use]
    pub fn request_of(&self, session_key: &str) -> Option<&PendingAskView> {
        self.entries
            .iter()
            .find(|e| e.view.session_key == session_key)
            .map(|e| &e.view)
    }

    /// Capture a unique start generation before fetching a snapshot.
    pub fn begin_snapshot(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Apply a live notification without ordering conflicting request IDs.
    pub fn apply_push(&mut self, ask: PendingAskView) -> PushOutcome {
        if ask.request_id.is_empty() {
            return PushOutcome::NeedsSnapshot;
        }
        if self.is_fenced(&ask.session_key, &ask.request_id) {
            return PushOutcome::Ignored;
        }
        let pos = self
            .entries
            .iter()
            .position(|e| e.view.session_key == ask.session_key);
        let Some(pos) = pos else {
            self.install(ask);
            return PushOutcome::Installed;
        };
        let existing = self.entries[pos].view.clone();
        if same_request(&existing, &ask) {
            // The cursor only moves forward: an older cursor is a stale frame.
            if ask.answered < existing.answered {
                return PushOutcome::Ignored;
            }
            if ask.answered > existing.answered {
                self.release_answered_drafts(
                    &existing.request_id,
                    &existing.questions,
                    ask.answered,
                );
            }
            self.install(ask);
            PushOutcome::Installed
        } else {
            PushOutcome::NeedsSnapshot
        }
    }

    fn install(&mut self, ask: PendingAskView) {
        self.generation += 1;
        let generation = self.generation;
        let entry = BoardEntry {
            view: ask,
            generation,
        };
        match self
            .entries
            .iter()
            .position(|e| e.view.session_key == entry.view.session_key)
        {
            Some(pos) => self.entries[pos] = entry,
            None => self.entries.push(entry),
        }
    }

    /// Apply an authoritative snapshot.
    ///
    /// `start_generation` is the generation the snapshot was fetched at. An
    /// entry the board learned after that moment is newer than the snapshot
    /// and survives it; an entry the snapshot knows about replaces what the
    /// board held — unless the board already holds a higher cursor for the
    /// same request.
    pub fn apply_snapshot(
        &mut self,
        snapshot: &PendingListResponse,
        start_generation: u64,
    ) -> SnapshotOutcome {
        if start_generation < self.last_snapshot_start {
            return SnapshotOutcome {
                applied: 0,
                kept: self.entries.len(),
            };
        }
        self.last_snapshot_start = start_generation;
        let mut applied = 0usize;
        let mut kept = 0usize;
        let mut next: Vec<BoardEntry> = Vec::new();

        for pending in &snapshot.pending {
            let incoming = view_from_pending(pending);
            if self.is_fenced(&incoming.session_key, &incoming.request_id) {
                // This request already ended here; a snapshot that still
                // lists it is stale and must not resurrect the card.
                continue;
            }
            let existing = self
                .entries
                .iter()
                .find(|e| e.view.session_key == incoming.session_key)
                .cloned();
            if let Some(ex) = existing {
                let same = same_request(&ex.view, &incoming);
                if ex.generation > start_generation
                    || (same && ex.view.answered > incoming.answered)
                {
                    kept += 1;
                    next.push(ex);
                    continue;
                }
                if !same {
                    self.retire(&ex.view);
                } else if incoming.answered > ex.view.answered {
                    self.release_answered_drafts(
                        &ex.view.request_id,
                        &ex.view.questions,
                        incoming.answered,
                    );
                }
            }
            applied += 1;
            next.push(BoardEntry {
                view: incoming,
                generation: start_generation,
            });
        }

        // Entries the snapshot does not mention are gone — unless the board
        // learned about them while the snapshot was in flight.
        for entry in std::mem::take(&mut self.entries) {
            if next
                .iter()
                .any(|e| e.view.session_key == entry.view.session_key)
            {
                continue;
            }
            if entry.generation > start_generation {
                kept += 1;
                next.push(entry);
            } else {
                self.retire(&entry.view);
            }
        }

        self.entries = next;
        SnapshotOutcome { applied, kept }
    }

    /// Apply a terminal frame.
    ///
    /// Only the exact `(session_key, request_id)` pair is removed: a terminal
    /// frame of a superseded request must not take its successor with it.
    pub fn apply_ended(&mut self, session_key: &str, request_id: &str) -> EndOutcome {
        if request_id.is_empty() {
            // The frame names nothing. Removing by session would drop a
            // successor; the only safe move is to reconcile with a snapshot.
            return EndOutcome::NeedsSnapshot;
        }
        let pos = self
            .entries
            .iter()
            .position(|e| e.view.session_key == session_key && e.view.request_id == request_id);
        let outcome = match pos {
            Some(pos) => {
                let entry = self.entries.remove(pos);
                self.retire(&entry.view);
                self.generation += 1;
                EndOutcome::Removed
            }
            None => EndOutcome::NotOwned,
        };
        self.remember_ended(session_key, request_id);
        outcome
    }

    fn retire(&mut self, view: &PendingAskView) {
        self.clear_drafts(&view.request_id);
        self.forget_latch(&view.request_id);
        if !view.request_id.is_empty() {
            self.remember_ended(&view.session_key, &view.request_id);
        }
    }

    fn remember_ended(&mut self, session_key: &str, request_id: &str) {
        if self
            .ended
            .iter()
            .any(|e| e.session_key == session_key && e.request_id == request_id)
        {
            return;
        }
        self.ended.push(EndedRequest {
            session_key: session_key.to_string(),
            request_id: request_id.to_string(),
        });
        if self.ended.len() > ENDED_FENCE_CAPACITY {
            let overflow = self.ended.len() - ENDED_FENCE_CAPACITY;
            self.ended.drain(0..overflow);
        }
    }

    fn is_fenced(&self, session_key: &str, request_id: &str) -> bool {
        self.ended
            .iter()
            .any(|e| e.session_key == session_key && e.request_id == request_id)
    }

    /// The staged answers of one request.
    #[must_use]
    pub fn drafts(&self, request_id: &str) -> HashMap<String, ClarificationInput> {
        self.drafts.get(request_id).cloned().unwrap_or_default()
    }

    /// The staged answer of one question.
    #[must_use]
    pub fn draft(&self, request_id: &str, question_id: &str) -> Option<ClarificationInput> {
        self.drafts
            .get(request_id)
            .and_then(|d| d.get(question_id))
            .cloned()
    }

    /// Stage (or replace) the answer of one question.
    pub fn set_draft(&mut self, request_id: &str, question_id: &str, input: ClarificationInput) {
        self.drafts
            .entry(request_id.to_string())
            .or_default()
            .insert(question_id.to_string(), input);
    }

    /// Forget the staged answer of one question. An emptied field is no answer
    /// at all, so its staged value is dropped instead of sent as empty text.
    pub fn clear_draft(&mut self, request_id: &str, question_id: &str) {
        if let Some(drafts) = self.drafts.get_mut(request_id) {
            drafts.remove(question_id);
        }
    }

    /// Forget the staged answers of one request. Secrets included.
    pub fn clear_drafts(&mut self, request_id: &str) {
        self.drafts.remove(request_id);
    }

    /// Forget the staged answers of the questions the cursor already passed.
    fn release_answered_drafts(
        &mut self,
        request_id: &str,
        questions: &[AskQuestionView],
        answered: usize,
    ) {
        let Some(drafts) = self.drafts.get_mut(request_id) else {
            return;
        };
        for question in questions.iter().take(answered) {
            drafts.remove(&question.id);
        }
    }

    /// Take the submission latch of one request.
    ///
    /// Taken synchronously, before the request is spawned, so a double click
    /// cannot produce two tickets for one question.
    pub fn acquire_submission(&self, request_id: &str) -> bool {
        if request_id.is_empty() {
            return false;
        }
        let mut latches = self
            .submissions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let latch = latches
            .entry(request_id.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)));
        !latch.swap(true, Ordering::SeqCst)
    }

    /// Release the submission latch of one request.
    pub fn release_submission(&self, request_id: &str) {
        let latches = self
            .submissions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(latch) = latches.get(request_id) {
            latch.store(false, Ordering::SeqCst);
        }
    }

    /// Whether a submission of one request is currently in flight.
    #[must_use]
    pub fn submission_in_flight(&self, request_id: &str) -> bool {
        let latches = self
            .submissions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        latches
            .get(request_id)
            .map(|latch| latch.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    /// Forget the latch of a request that is gone.
    fn forget_latch(&self, request_id: &str) {
        let mut latches = self
            .submissions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        latches.remove(request_id);
    }
}

/// Build the guarded `submit` action for the outstanding questions.
///
/// `None` when the identity is missing (a core that names no request cannot
/// be addressed) or when a question still has no answer: a partial batch is
/// never sent.
#[must_use]
pub fn build_submit_params(
    session_key: &str,
    request_id: &str,
    current_question_id: &str,
    questions: &[AskQuestionView],
    drafts: &HashMap<String, ClarificationInput>,
) -> Option<ClarificationActionParams> {
    if request_id.is_empty() || current_question_id.is_empty() {
        return None;
    }
    let mut answers: Vec<ClarificationAnswerInput> = Vec::new();
    for question in questions {
        let answer = drafts.get(&question.id)?;
        answers.push(ClarificationAnswerInput {
            question_id: question.id.clone(),
            answer: answer.clone(),
        });
    }
    if answers.is_empty() {
        return None;
    }
    Some(ClarificationActionParams {
        session_key: session_key.to_string(),
        expected_request_id: request_id.to_string(),
        expected_question_id: current_question_id.to_string(),
        action: ClarificationAction::Submit { answers },
    })
}

/// Build the guarded `cancel` action for the live request.
#[must_use]
pub fn build_cancel_params(
    session_key: &str,
    request_id: &str,
    current_question_id: &str,
) -> Option<ClarificationActionParams> {
    if request_id.is_empty() || current_question_id.is_empty() {
        return None;
    }
    Some(ClarificationActionParams {
        session_key: session_key.to_string(),
        expected_request_id: request_id.to_string(),
        expected_question_id: current_question_id.to_string(),
        action: ClarificationAction::Cancel {},
    })
}

/// A review line for the staged answers.
///
/// A secret draft is reported as entered and never echoed — its contents must
/// not reach the transcript, an error message or a log.
#[must_use]
pub fn draft_summary(
    questions: &[AskQuestionView],
    drafts: &HashMap<String, ClarificationInput>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (index, question) in questions.iter().enumerate() {
        let Some(draft) = drafts.get(&question.id) else {
            continue;
        };
        let rendered = if question.secret {
            "••••".to_string()
        } else {
            match draft {
                ClarificationInput::Choices { indices } => indices
                    .iter()
                    .filter_map(|i| question.options.get(*i as usize).map(|o| o.label.as_str()))
                    .collect::<Vec<_>>()
                    .join(", "),
                ClarificationInput::Text { text } => text.trim().to_string(),
            }
        };
        parts.push(format!("{}: {rendered}", index + 1));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(session: &str, request: &str, answered: usize) -> PendingAskView {
        PendingAskView {
            session_key: session.to_string(),
            request_id: request.to_string(),
            question: "Where?".to_string(),
            options: vec!["here".to_string()],
            questions: vec![AskQuestionView {
                id: "q1".to_string(),
                header: None,
                prompt: "Where?".to_string(),
                options: vec![crate::state::notifications::AskOptionView {
                    label: "here".to_string(),
                    description: None,
                }],
                multi_select: false,
                secret: false,
            }],
            answered,
        }
    }

    fn snapshot(entries: &[PendingClarification]) -> PendingListResponse {
        PendingListResponse {
            pending: entries.to_vec(),
        }
    }

    fn pending_entry(session: &str, request: &str, answered: usize) -> PendingClarification {
        PendingClarification {
            session_key: session.to_string(),
            request_id: request.to_string(),
            run_id: String::new(),
            question: "Where?".to_string(),
            options: vec!["here".to_string()],
            questions: Vec::new(),
            answered,
        }
    }

    /// A terminal frame names one request. It must remove exactly that
    /// request — not the successor that replaced it — and a snapshot that
    /// still lists the ended request must not bring the card back.
    #[test]
    fn ending_a_does_not_remove_b_or_resurrect_a() {
        let mut board = ClarificationBoard::default();
        board.apply_push(view("s", "A", 0));
        assert_eq!(
            board.apply_ended("s", "A"),
            EndOutcome::Removed,
            "the named request is removed"
        );
        assert!(board.pending().is_empty());

        // The successor arrives.
        board.apply_push(view("s", "B", 0));
        assert_eq!(board.pending().len(), 1);

        // A late terminal frame of the OLD request must not take B with it.
        assert_eq!(board.apply_ended("s", "A"), EndOutcome::NotOwned);
        assert_eq!(board.pending().len(), 1, "the successor keeps the card");
        assert_eq!(board.pending()[0].request_id, "B");

        // A snapshot fetched before the terminal frame still lists A. It must
        // not resurrect the ended request.
        let stale = snapshot(&[pending_entry("s", "A", 0)]);
        board.apply_snapshot(&stale, 0);
        assert_eq!(board.apply_push(view("s", "A", 0)), PushOutcome::Ignored);
        assert_eq!(board.pending()[0].request_id, "B");
    }

    /// A push that lands while a snapshot is in flight is newer than that
    /// snapshot. Applying the snapshot must not rewind the card.
    #[test]
    fn older_fetch_never_overwrites_new_push() {
        let mut board = ClarificationBoard::default();
        let start = board.begin_snapshot();
        // A push advances the request to the second question.
        board.apply_push(view("s", "A", 1));
        // The snapshot was fetched before that push and still shows the old
        // cursor.
        let stale = snapshot(&[pending_entry("s", "A", 0)]);
        board.apply_snapshot(&stale, start);
        assert_eq!(
            board.pending()[0].answered,
            1,
            "the newer push survives the older fetch"
        );
    }

    #[test]
    fn conflicting_push_and_reordered_snapshots_cannot_replace_the_live_card() {
        let mut board = ClarificationBoard::default();
        let oldest = board.begin_snapshot();
        board.apply_push(view("s", "B", 0));
        let stale = snapshot(&[pending_entry("s", "A", 0)]);
        board.apply_snapshot(&stale, oldest);
        assert_eq!(
            board.apply_push(view("s", "A", 0)),
            PushOutcome::NeedsSnapshot
        );
        assert_eq!(board.pending()[0].request_id, "B");

        let slow = board.begin_snapshot();
        let fresh = board.begin_snapshot();
        board.apply_snapshot(&snapshot(&[pending_entry("s", "B", 0)]), fresh);
        board.apply_snapshot(&stale, slow);
        assert_eq!(board.pending()[0].request_id, "B");
    }

    /// A snapshot is authoritative for what it knows: a request it no longer
    /// lists is gone — unless the board learned about it afterwards.
    #[test]
    fn snapshot_drops_what_it_no_longer_lists() {
        let mut board = ClarificationBoard::default();
        board.apply_push(view("s", "A", 0));
        let start = board.begin_snapshot();
        let empty = snapshot(&[]);
        board.apply_snapshot(&empty, start);
        assert!(board.pending().is_empty());
    }

    /// A terminal frame without a request identity names nothing: the card
    /// stays and the caller has to reconcile with a snapshot.
    #[test]
    fn identity_less_terminal_asks_for_a_snapshot() {
        let mut board = ClarificationBoard::default();
        board.apply_push(view("s", "A", 0));
        assert_eq!(board.apply_ended("s", ""), EndOutcome::NeedsSnapshot);
        assert_eq!(board.pending().len(), 1, "nothing is removed");
    }

    /// A push with an older cursor never rewinds the card.
    #[test]
    fn an_older_cursor_cannot_regress() {
        let mut board = ClarificationBoard::default();
        board.apply_push(view("s", "A", 2));
        assert_eq!(board.apply_push(view("s", "A", 1)), PushOutcome::Ignored);
        assert_eq!(board.pending()[0].answered, 2);
    }

    /// The submission latch admits one ticket at a time — per request, so a
    /// card of another session is not blocked by it.
    #[test]
    fn one_submission_at_a_time() {
        let board = ClarificationBoard::default();
        assert!(board.acquire_submission("A"));
        assert!(
            !board.acquire_submission("A"),
            "a second activation must not create a second ticket"
        );
        assert!(
            board.acquire_submission("B"),
            "another request owns its own latch"
        );
        board.release_submission("A");
        assert!(board.acquire_submission("A"));
    }

    /// A submit is guarded: it needs the identity of the request and an
    /// answer for every outstanding question, and the summary of a secret
    /// answer never contains the secret.
    #[test]
    fn question_submit_is_guarded_and_masks_secret_summary() {
        let questions = vec![
            AskQuestionView {
                id: "q1".to_string(),
                header: None,
                prompt: "Which?".to_string(),
                options: vec![crate::state::notifications::AskOptionView {
                    label: "a".to_string(),
                    description: None,
                }],
                multi_select: false,
                secret: false,
            },
            AskQuestionView {
                id: "q2".to_string(),
                header: None,
                prompt: "Credential?".to_string(),
                options: Vec::new(),
                multi_select: false,
                secret: true,
            },
        ];
        let mut drafts = HashMap::new();
        drafts.insert(
            "q1".to_string(),
            ClarificationInput::Text {
                text: "1".to_string(),
            },
        );

        // The secret question has no answer yet: nothing is sent.
        assert!(
            build_submit_params("s", "A", "q1", &questions, &drafts).is_none(),
            "a partial batch is never sent"
        );
        drafts.insert(
            "q2".to_string(),
            ClarificationInput::Text {
                text: "top-secret".to_string(),
            },
        );
        let params = build_submit_params("s", "A", "q1", &questions, &drafts).unwrap();
        match &params.action {
            ClarificationAction::Submit { answers } => {
                assert_eq!(answers.len(), 2);
                assert_eq!(answers[0].question_id, "q1");
                assert_eq!(
                    answers[0].answer,
                    ClarificationInput::Text {
                        text: "1".to_string()
                    },
                    "custom text is not parsed as an option index"
                );
                assert_eq!(answers[1].question_id, "q2");
            }
            other => panic!("unexpected action: {other:?}"),
        }

        // The review line masks the secret.
        let summary = draft_summary(&questions, &drafts);
        assert!(
            !summary.contains("top-secret"),
            "the secret leaked into the review: {summary}"
        );
        assert!(
            summary.contains("••••"),
            "the secret is not marked: {summary}"
        );

        // The latch admits one ticket for this request only.
        let board = ClarificationBoard::default();
        assert!(board.acquire_submission("A"));
        assert!(!board.acquire_submission("A"));
        board.release_submission("A");
        assert!(board.acquire_submission("A"));
    }

    /// A submit is only built when every outstanding question has an answer,
    /// and custom text travels verbatim — a numeric answer is text, not an
    /// index.
    #[test]
    fn submit_is_guarded_and_text_is_not_reinterpreted() {
        let question = AskQuestionView {
            id: "q1".to_string(),
            header: None,
            prompt: "Which?".to_string(),
            options: vec![crate::state::notifications::AskOptionView {
                label: "a".to_string(),
                description: None,
            }],
            multi_select: false,
            secret: false,
        };
        let mut drafts = HashMap::new();
        // Missing an answer: nothing is sent.
        assert!(build_submit_params("s", "A", "q1", &[question.clone()], &drafts).is_none());
        drafts.insert(
            "q1".to_string(),
            ClarificationInput::Text {
                text: "1".to_string(),
            },
        );
        let params = build_submit_params("s", "A", "q1", &[question.clone()], &drafts).unwrap();
        match &params.action {
            ClarificationAction::Submit { answers } => {
                assert_eq!(answers.len(), 1);
                assert_eq!(answers[0].question_id, "q1");
                assert_eq!(
                    answers[0].answer,
                    ClarificationInput::Text {
                        text: "1".to_string()
                    },
                    "custom text is not parsed as an option index"
                );
            }
            other => panic!("unexpected action: {other:?}"),
        }
        // A missing identity cannot be addressed.
        assert!(build_submit_params("s", "", "q1", &[question.clone()], &drafts).is_none());
        assert!(build_submit_params("s", "A", "", &[question], &drafts).is_none());
    }

    /// A secret draft is reported as entered; its contents never reach the
    /// summary.
    #[test]
    fn a_secret_draft_is_masked_in_the_summary() {
        let secret = AskQuestionView {
            id: "q1".to_string(),
            header: None,
            prompt: "Credential?".to_string(),
            options: Vec::new(),
            multi_select: false,
            secret: true,
        };
        let mut drafts = HashMap::new();
        drafts.insert(
            "q1".to_string(),
            ClarificationInput::Text {
                text: "top-secret".to_string(),
            },
        );
        let summary = draft_summary(&[secret], &drafts);
        assert!(!summary.contains("top-secret"), "secret leaked: {summary}");
        assert!(summary.contains("••••"), "secret is not marked: {summary}");
    }

    /// The drafts of a replaced request do not survive the replacement, and
    /// neither do the drafts of the questions the cursor passed.
    #[test]
    fn drafts_are_released_on_replacement_and_advance() {
        let mut board = ClarificationBoard::default();
        let mut view_a = view("s", "A", 0);
        view_a.questions.push(AskQuestionView {
            id: "q2".to_string(),
            header: None,
            prompt: "Second?".to_string(),
            options: Vec::new(),
            multi_select: false,
            secret: true,
        });
        board.apply_push(view_a.clone());
        board.set_draft(
            "A",
            "q1",
            ClarificationInput::Text {
                text: "one".to_string(),
            },
        );
        board.set_draft(
            "A",
            "q2",
            ClarificationInput::Text {
                text: "two".to_string(),
            },
        );

        // The cursor advances past q1: its draft is released, q2's stays.
        let mut advanced = view("s", "A", 1);
        advanced.questions = view_a.questions.clone();
        board.apply_push(advanced);
        assert!(board.draft("A", "q1").is_none(), "answered draft released");
        assert!(board.draft("A", "q2").is_some(), "outstanding draft kept");

        // A conflict first goes to Core; an authoritative replacement drops private drafts.
        assert_eq!(
            board.apply_push(view("s", "B", 0)),
            PushOutcome::NeedsSnapshot
        );
        let start = board.begin_snapshot();
        board.apply_snapshot(&snapshot(&[pending_entry("s", "B", 0)]), start);
        assert!(board.drafts("A").is_empty(), "private drafts released");
        assert_eq!(board.apply_push(view("s", "A", 1)), PushOutcome::Ignored);

        board.set_draft(
            "B",
            "q1",
            ClarificationInput::Text {
                text: "transient".into(),
            },
        );
        let start = board.begin_snapshot();
        board.apply_snapshot(&snapshot(&[]), start);
        assert!(
            board.drafts("B").is_empty(),
            "omission also releases private drafts"
        );
    }
}
