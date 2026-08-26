//! Human-in-the-loop clarification: the types a parked agent turn uses to ask
//! the human something and interpret what comes back.
//!
//! A [`ClarificationRequest`] is **one or more** [`ClarificationQuestion`]s
//! parked under a single session key. That plurality is the whole point: a
//! rich client (Panel / TUI) answers every question in one interaction, while a
//! plain-text channel — or any client that only understands the legacy
//! single-question projection — answers them one at a time, the registry
//! advancing a cursor per reply. Both land on the same [`ClarificationAnswer`]
//! vector, so the tool that is parked cannot tell which surface answered it.
//!
//! # Example
//!
//! ```rust,no_run
//! use alephcore::clarification::{ClarificationRequest, ClarificationOption};
//!
//! // One select question.
//! let request = ClarificationRequest::select(
//!     "What style would you like?",
//!     vec![
//!         ClarificationOption::new("professional", "Professional"),
//!         ClarificationOption::new("casual", "Casual"),
//!     ],
//! );
//!
//! // One free-text question.
//! let request = ClarificationRequest::text("Enter target language:");
//! ```

pub mod ask;
pub mod render;
pub mod session;

pub use ask::{ask, AskOutcome, ClarificationDeps};
pub use session::{ClarificationManager, ResolveOutcome, DEFAULT_CLARIFY_TIMEOUT};

// `PendingClarification` is constructed by `ClarificationManager::list_pending`
// and serialized as part of the gateway's clarification API response. It is
// not part of the public Rust surface — Panel / TUI consume it via the JSON
// gateway, not by importing the type — so it stays a private detail of the
// `session` module.

use serde::{Deserialize, Serialize};

/// A single option in a select-type question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationOption {
    /// Display label for the option
    pub label: String,
    /// Value to return when selected
    pub value: String,
    /// Optional description for additional context
    pub description: Option<String>,
}

impl ClarificationOption {
    /// Create a new option with value and label.
    #[must_use]
    pub fn new(value: &str, label: &str) -> Self {
        Self {
            label: label.to_string(),
            value: value.to_string(),
            description: None,
        }
    }

    /// Attach an explanatory description, helping the user choose between
    /// otherwise-terse options. An empty/whitespace description is ignored so
    /// callers can pass through optional input without a branch.
    #[must_use]
    pub fn with_description(mut self, description: &str) -> Self {
        let trimmed = description.trim();
        if !trimmed.is_empty() {
            self.description = Some(trimmed.to_string());
        }
        self
    }
}

/// One question inside a [`ClarificationRequest`].
///
/// # Why there is no `clarification_type`
///
/// There used to be a `ClarificationType::{Select, Text}` discriminator
/// *alongside* `options: Option<Vec<_>>` — two representations of one fact,
/// free to disagree (a `Select` with `None` options interpreted every reply as
/// free text while every renderer promised a menu). The shape now answers it:
/// **empty `options` is a free-text question**, and nothing can drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationQuestion {
    /// Stable identifier the answer is keyed by. Never shown to the user; it
    /// exists so a multi-question answer set can be read positionally *and*
    /// by name, which is what lets a caller add a question later without
    /// silently reshuffling what an earlier index meant.
    pub id: String,
    /// Short chip rendered beside the prompt by clients that have room for one
    /// (codex `header`, pi `label`). Never load-bearing: a client that ignores
    /// it loses nothing but the chip.
    pub header: Option<String>,
    /// The prompt shown to the user.
    pub prompt: String,
    /// Offered choices. **Empty = free-text question.**
    pub options: Vec<ClarificationOption>,
    /// Accept several picks (`"1,3"`). Ignored when `options` is empty.
    ///
    /// Inline keyboards are deliberately suppressed for a multi-select
    /// question: a tap can only ever carry one index, so a keyboard here would
    /// render a control that silently answers *less* than the question asks.
    pub multi_select: bool,
    /// The answer is a credential. Load-bearing, not cosmetic: a secret
    /// question is never handed to a third-party channel transport (see
    /// [`ask`]), and rich clients mask the input.
    pub secret: bool,
}

impl ClarificationQuestion {
    /// A free-text question with `id`.
    #[must_use]
    pub fn text(id: &str, prompt: &str) -> Self {
        Self {
            id: id.to_string(),
            header: None,
            prompt: prompt.to_string(),
            options: Vec::new(),
            multi_select: false,
            secret: false,
        }
    }

    /// A pick-one question with `id`.
    #[must_use]
    pub fn select(id: &str, prompt: &str, options: Vec<ClarificationOption>) -> Self {
        Self {
            options,
            ..Self::text(id, prompt)
        }
    }

    /// Attach the short display chip. Blank input is ignored.
    #[must_use]
    pub fn with_header(mut self, header: &str) -> Self {
        let trimmed = header.trim();
        if !trimmed.is_empty() {
            self.header = Some(trimmed.to_string());
        }
        self
    }

    /// Allow several picks.
    #[must_use]
    pub const fn with_multi_select(mut self, multi: bool) -> Self {
        self.multi_select = multi;
        self
    }

    /// Mark the answer as a credential.
    #[must_use]
    pub const fn with_secret(mut self, secret: bool) -> Self {
        self.secret = secret;
        self
    }

    /// Whether this question offers a menu.
    #[must_use]
    pub fn has_options(&self) -> bool {
        !self.options.is_empty()
    }
}

/// Request for user clarification — one or more questions parked together.
///
/// # Why there is no `id` here
///
/// The registry is keyed by `session_key` — one live request per session —
/// and all read paths (`list_pending`, `resolve`) address it that way. An `id`
/// field existed, was written by every caller, and was read by **nothing**
/// outside this module's own unit tests. Removed under P6 rather than left as
/// "a hook for future id-addressing": the approval twin genuinely is
/// id-addressed (`ExecApprovalManager::resolve_with_reason(id)`), so if
/// clarifications ever need it, that shape is there to copy — a dead field is
/// not a head start, it is a claim that something is wired when it is not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationRequest {
    /// The questions, answered in this order by sequential (plain-text)
    /// surfaces. Never empty — every constructor guarantees at least one.
    ///
    /// `pub(crate)` (not `pub`) to make the "never empty" invariant actually
    /// load-bearing: external callers cannot build a `ClarificationRequest`
    /// by struct literal and bypass [`Self::new`]. In-crate code reaches the
    /// field directly (this is a hot path — `iter().filter()` reads on the
    /// internal face), but external callers go through the constructors and
    /// the [`Self::questions`] / [`Self::first`] / [`Self::len`] accessors.
    pub(crate) questions: Vec<ClarificationQuestion>,
}

impl ClarificationRequest {
    /// Build from an explicit question list.
    ///
    /// An empty list is rejected rather than silently accepted: a request with
    /// nothing to ask would register a waiter that no reply can ever complete.
    pub fn new(questions: Vec<ClarificationQuestion>) -> Result<Self, &'static str> {
        if questions.is_empty() {
            return Err("a clarification must carry at least one question");
        }
        Ok(Self { questions })
    }

    /// Borrow the questions vector as a slice.
    ///
    /// The slice is never empty — the constructors guarantee at least one.
    /// Internal callers that need a strict guarantee use [`Self::first`]
    /// (which still applies a debug assertion).
    #[must_use]
    pub fn questions(&self) -> &[ClarificationQuestion] {
        &self.questions
    }

    /// Create a single free-text clarification request.
    #[must_use]
    pub fn text(prompt: &str) -> Self {
        Self {
            questions: vec![ClarificationQuestion::text(DEFAULT_QUESTION_ID, prompt)],
        }
    }

    /// Create a single pick-one clarification request.
    #[must_use]
    pub fn select(prompt: &str, options: Vec<ClarificationOption>) -> Self {
        Self {
            questions: vec![ClarificationQuestion::select(
                DEFAULT_QUESTION_ID,
                prompt,
                options,
            )],
        }
    }

    /// The first question. Every request has one by construction.
    ///
    /// Returns `Option` rather than panicking: now that `questions` is
    /// `pub(crate)`, in-crate callers CAN bypass [`Self::new`] (struct
    /// literal inside the module is still possible), so the API honours
    /// the published contract instead of unwrapping into a caller-visible
    /// panic. All production call sites going through `new()` / `text()` /
    /// `select()` see a `Some`.
    #[must_use]
    pub fn first(&self) -> Option<&ClarificationQuestion> {
        // debug-assert the invariant the docstring promises — release builds
        // return None quietly while `cargo test` catches any drift.
        debug_assert!(
            !self.questions.is_empty(),
            "a ClarificationRequest built via new()/text()/select() is never empty"
        );
        self.questions.first()
    }

    /// How many questions are outstanding in total.
    #[allow(clippy::len_without_is_empty)] // see is_empty doc below — answer is genuinely 'never'
    #[must_use]
    pub fn len(&self) -> usize {
        self.questions.len()
    }

    // `is_empty()` was cut in the 2026-08-18 audit (sw-clarification-03):
    // zero non-test callers repo-wide, and the doc already admitted the
    // value was always false (the field is invariantly non-empty by
    // construction). The `len_without_is_empty` clippy lint is allowed
    // locally because the answer is genuinely 'never' — a request with
    // no questions is unrepresentable.
}

/// Id given to the single question of [`ClarificationRequest::text`] /
/// [`ClarificationRequest::select`]. Callers that only ever ask one thing
/// (workflow `clarify` steps, plan approval) never need to invent one.
pub const DEFAULT_QUESTION_ID: &str = "answer";

// =============================================================================
// Wire views
// =============================================================================

/// Wire projection of one [`ClarificationOption`].
///
/// `value` is deliberately absent: it is the *interpreter's* vocabulary, and
/// every client answers by 1-based index or free text so that
/// [`session::interpret_reply`] stays the only place a reply becomes a value.
/// Shipping `value` too would invite a client to send it back and quietly
/// become a second interpreter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationOptionView {
    /// What the user reads.
    pub label: String,
    /// Why they might pick it. **This is the field whose absence made the
    /// Panel and TUI render a bare label while channels rendered
    /// `label — description`.**
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Wire projection of one [`ClarificationQuestion`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationQuestionView {
    /// Stable id — echoed back in the answer set.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub options: Vec<ClarificationOptionView>,
    #[serde(default)]
    pub multi_select: bool,
    #[serde(default)]
    pub secret: bool,
}

impl From<&ClarificationQuestion> for ClarificationQuestionView {
    fn from(q: &ClarificationQuestion) -> Self {
        Self {
            id: q.id.clone(),
            header: q.header.clone(),
            prompt: q.prompt.clone(),
            options: q
                .options
                .iter()
                .map(|o| ClarificationOptionView {
                    label: o.label.clone(),
                    description: o.description.clone(),
                })
                .collect(),
            multi_select: q.multi_select,
            secret: q.secret,
        }
    }
}

/// How a clarification ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClarificationResultType {
    /// Every question was answered.
    Answered,
    /// User cancelled the request, or it was superseded.
    Cancelled,
    /// Request timed out.
    Timeout,
}

/// One question's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationAnswer {
    /// [`ClarificationQuestion::id`] this answers.
    pub question_id: String,
    /// 0-based indices of the options the reply matched. Empty when the reply
    /// was free text (including the "Other" case a menu never forbids).
    pub selected_indices: Vec<u32>,
    /// The answer: the matched option value(s), `", "`-joined for a
    /// multi-select, or the raw reply text when nothing matched.
    pub value: String,
}

impl ClarificationAnswer {
    /// Whether the reply fell through to free text rather than matching a
    /// listed option.
    #[must_use]
    pub fn is_custom(&self) -> bool {
        self.selected_indices.is_empty()
    }
}

/// Result of a clarification request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationResult {
    /// Type of result
    pub result_type: ClarificationResultType,
    /// One entry per question, in question order. Empty for
    /// [`ClarificationResultType::Cancelled`] / `Timeout`.
    pub answers: Vec<ClarificationAnswer>,
}

impl ClarificationResult {
    /// Create an answered result.
    #[must_use]
    pub const fn answered(answers: Vec<ClarificationAnswer>) -> Self {
        Self {
            result_type: ClarificationResultType::Answered,
            answers,
        }
    }

    /// Create a cancelled result
    #[must_use]
    pub const fn cancelled() -> Self {
        Self {
            result_type: ClarificationResultType::Cancelled,
            answers: Vec::new(),
        }
    }

    /// Create a timeout result
    #[must_use]
    pub const fn timeout() -> Self {
        Self {
            result_type: ClarificationResultType::Timeout,
            answers: Vec::new(),
        }
    }

    /// The single-question shorthand: the first answer's value, if any.
    #[must_use]
    pub fn value(&self) -> Option<&str> {
        self.answers.first().map(|a| a.value.as_str())
    }

    /// The single-question shorthand: the first answer's first matched option
    /// index, if the reply matched one.
    #[must_use]
    pub fn selected_index(&self) -> Option<u32> {
        self.answers
            .first()
            .and_then(|a| a.selected_indices.first().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clarification_option_new() {
        let option = ClarificationOption::new("pro", "Professional");
        assert_eq!(option.value, "pro");
        assert_eq!(option.label, "Professional");
        assert!(option.description.is_none());
    }

    #[test]
    fn test_clarification_option_with_description() {
        let option =
            ClarificationOption::new("pro", "Professional").with_description("formal tone");
        assert_eq!(option.description.as_deref(), Some("formal tone"));
        // Blank/whitespace descriptions are ignored, keeping the field None.
        let blank = ClarificationOption::new("pro", "Professional").with_description("   ");
        assert!(blank.description.is_none());
    }

    #[test]
    fn test_clarification_request_select() {
        let request = ClarificationRequest::select(
            "Choose style:",
            vec![
                ClarificationOption::new("a", "Option A"),
                ClarificationOption::new("b", "Option B"),
            ],
        );

        let first = request
            .first()
            .expect("constructor-built request is non-empty");
        assert_eq!(request.len(), 1);
        assert_eq!(first.prompt, "Choose style:");
        assert!(first.has_options());
        assert_eq!(first.options.len(), 2);
    }

    #[test]
    fn test_clarification_request_text() {
        let request = ClarificationRequest::text("Enter name:");

        assert_eq!(
            request
                .first()
                .expect("constructor-built request is non-empty")
                .prompt,
            "Enter name:"
        );
        assert!(!request
            .first()
            .expect("constructor-built request is non-empty")
            .has_options());
    }

    /// A request with nothing to ask registers a waiter no reply can complete,
    /// so it is refused at construction rather than parked.
    #[test]
    fn empty_question_list_is_refused() {
        assert!(ClarificationRequest::new(Vec::new()).is_err());
        assert!(ClarificationRequest::new(vec![ClarificationQuestion::text("q1", "?")]).is_ok());
    }

    #[test]
    fn question_builders_are_additive() {
        let q = ClarificationQuestion::select(
            "env",
            "Where?",
            vec![ClarificationOption::new("prod", "Production")],
        )
        .with_header("Env")
        .with_multi_select(true)
        .with_secret(true);
        assert_eq!(q.header.as_deref(), Some("Env"));
        assert!(q.multi_select);
        assert!(q.secret);
        // Blank header stays None rather than rendering an empty chip.
        assert!(ClarificationQuestion::text("q", "?")
            .with_header("  ")
            .header
            .is_none());
    }

    #[test]
    fn result_shorthands_read_the_first_answer() {
        let result = ClarificationResult::answered(vec![
            ClarificationAnswer {
                question_id: "a".into(),
                selected_indices: vec![2],
                value: "gamma".into(),
            },
            ClarificationAnswer {
                question_id: "b".into(),
                selected_indices: vec![],
                value: "freeform".into(),
            },
        ]);
        assert_eq!(result.value(), Some("gamma"));
        assert_eq!(result.selected_index(), Some(2));
        assert!(!result.answers[0].is_custom());
        assert!(result.answers[1].is_custom());

        assert!(ClarificationResult::timeout().value().is_none());
        assert!(ClarificationResult::cancelled().selected_index().is_none());
    }
}
