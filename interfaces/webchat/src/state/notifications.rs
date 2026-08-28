//! `NotificationsState` — UI-only projection over `DashboardState.alerts`.
//!
//! The data layer already aggregates server-side alerts in
//! [`DashboardState::alerts`] via the `alerts.**` topic subscription. What's
//! missing is an aggregate UI surface — a bell with a count badge and a
//! popover listing every active alert. This module holds the small piece of
//! UI-private state that surface needs (open/closed, locally-dismissed keys)
//! and the pure derivations that drive the bell badge + the list.
//!
//! Notes on scope:
//!   * `dismissed` is session-local — we do not RPC the gateway to ack
//!     alerts. Server-side alerts re-appear if the underlying condition
//!     persists past reconnect, which is the right behaviour: a stale UI
//!     dismissal must not mask a real fault.
//!   * No new gateway event variant is required. Everything reads off the
//!     existing `RwSignal<HashMap<String, SystemAlert>>`.

use crate::components::sidebar::{AlertLevel, SystemAlert};
use leptos::prelude::*;
use std::collections::{HashMap, HashSet};

/// A pending operator-approval request rendered by the `NotificationCenter` with
/// inline allow-once / allow-session / deny buttons. Sourced from the
/// `exec.approvals.pending` RPC (the `approval.**` events are sparse — they only
/// trigger a refetch). Display-only.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingApprovalView {
    /// Approval request id (passed to `exec.approval.resolve`).
    pub id: String,
    /// The config tool name being requested (ExecApprovalRecord.command).
    pub command: String,
    /// The requesting agent id.
    pub agent_id: String,
    /// Session the requesting turn belongs to. Scopes the inline chat card to
    /// the conversation that is actually waiting.
    pub session_key: String,
    /// The harness call id of the tool this approval belongs to — the same id
    /// `stream.tool_start` carries. The key the inline card is paired on.
    /// `None` for approvals with no owning tool row (cluster-node approvals,
    /// raw exec-command approvals): those render unattached, in the bell.
    pub tool_call_id: Option<String>,
    /// Why the tool asked (server-supplied escalation context).
    pub reason: Option<String>,
    /// Absolute epoch-ms deadline, derived at fetch time from the server's
    /// `remaining_ms` snapshot. Absolute (not a duration) so the card can count
    /// down against the shared 1s clock instead of freezing at fetch time.
    pub expires_at_ms: i64,
    /// The decision tiers the SERVER raised this card with (kebab-case wire
    /// values: `allow-once` / `allow-session` / `allow-always` / `deny`).
    ///
    /// The card renders buttons from this list instead of a fixed three,
    /// because which tiers a card may offer depends on why it fired and who is
    /// being asked — an "always allow" is not offered to a member, nor on a
    /// tool that declares its own confirmation floor. The server enforces the
    /// same list when the decision comes back, so drawing a button we should
    /// not have is a cosmetic bug, not a hole; drawing one too few is the
    /// safe direction. A record from an older core arrives without the field
    /// and falls back to the historical session ceiling.
    pub allowed_decisions: Vec<String>,
}

impl PendingApprovalView {
    /// Whole seconds left before this approval times out. Clamped at 0 — an
    /// expired-but-not-yet-refetched row must never render a negative countdown.
    ///
    /// `expires_at_ms == 0` is the no-expiry sentinel (attended approvals wait
    /// forever): the answer is meaningless, and the card renders the static
    /// no-timeout line instead of calling this.
    #[must_use]
    pub const fn remaining_secs(&self, now_ms: i64) -> i64 {
        let remaining = self.expires_at_ms - now_ms;
        if remaining < 0 {
            0
        } else {
            remaining / 1000
        }
    }
}

/// A question the agent is parked on (`ask_user`), rendered as an inline card
/// in the conversation that is waiting. Sourced from the `stream.ask_user`
/// frame (live) and `clarification.pending` (connect / reload). Display-only —
/// the reply is posted straight back to `clarification.resolve` on
/// `session_key`; the panel interprets nothing (R4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAskView {
    /// Clarification registry key the answer is posted back on.
    pub session_key: String,
    /// The question at the cursor, as core rendered it. Legacy projection —
    /// still what the composer's Enter-hijack answers.
    pub question: String,
    /// Choice labels for that question. Empty = open-ended.
    pub options: Vec<String>,
    /// Every question of the request, in order. Empty when talking to a core
    /// that predates the structured view — the card then falls back to
    /// `question` / `options`.
    pub questions: Vec<AskQuestionView>,
    /// How many of `questions` already have answers: the card renders from
    /// here.
    pub answered: usize,
}

/// One option of an [`AskQuestionView`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskOptionView {
    /// What the user reads.
    pub label: String,
    /// Why they might pick it. Rendered beside the label — a channel has shown
    /// this since the option type gained the field; the panel showed a bare
    /// label because it read the flat label array, which structurally cannot
    /// carry it.
    pub description: Option<String>,
}

/// One question of a pending `ask_user` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskQuestionView {
    /// Stable id — not shown, sent nowhere; kept so a future face can address
    /// an answer by name instead of by position.
    pub id: String,
    /// Short chip shown beside the prompt.
    pub header: Option<String>,
    /// The prompt.
    pub prompt: String,
    /// Offered choices; empty = free text.
    pub options: Vec<AskOptionView>,
    /// Several picks accepted.
    pub multi_select: bool,
    /// Credential: render a masked input and never echo it.
    pub secret: bool,
}

impl PendingAskView {
    /// The questions still to answer, from the cursor on.
    ///
    /// Empty `questions` (an older core) yields nothing, and every caller then
    /// falls back to the flat `question`/`options` pair — the same degradation
    /// a plain-text channel gets, for the same reason.
    #[must_use]
    pub fn remaining(&self) -> &[AskQuestionView] {
        self.questions.get(self.answered..).unwrap_or(&[])
    }
}

/// The question `session_key` is waiting on, if any. One question is pending
/// per session at most (a second `ask_user` supersedes the first, core-side),
/// so the first match is the answer.
#[must_use]
pub fn pending_ask_for_session<'a>(
    pending: &'a [PendingAskView],
    session_key: Option<&str>,
) -> Option<&'a PendingAskView> {
    let session_key = session_key?;
    pending.iter().find(|p| p.session_key == session_key)
}

/// Per-window notification UI state. Provided once in `app.rs`, consumed by
/// [`crate::components::notification_center::NotificationCenter`].
#[derive(Copy, Clone)]
pub struct NotificationsState {
    /// Popover visibility. Toggled by the bell button.
    pub is_open: RwSignal<bool>,
    /// Locally-dismissed alert keys. Cleared on page reload (no persistence).
    pub dismissed: RwSignal<HashSet<String>>,
}

impl Default for NotificationsState {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationsState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            is_open: RwSignal::new(false),
            dismissed: RwSignal::new(HashSet::new()),
        }
    }
}

/// Severity weight for sorting — higher means surfaced earlier.
const fn severity_weight(level: AlertLevel) -> u8 {
    match level {
        AlertLevel::Critical => 3,
        AlertLevel::Warning => 2,
        AlertLevel::Info => 1,
        AlertLevel::None => 0,
    }
}

/// Filter + sort the alert map into a deterministic list for the popover.
///
/// Skips:
///   * `AlertLevel::None` entries (server may emit them as "cleared" sentinels)
///   * Keys present in `dismissed`
///
/// Order: severity desc, then key asc — so Critical floats to the top and
/// equal-severity rows have a stable lexicographic order.
#[must_use]
pub fn visible_alerts(
    alerts: &HashMap<String, SystemAlert>,
    dismissed: &HashSet<String>,
) -> Vec<SystemAlert> {
    let mut out: Vec<SystemAlert> = alerts
        .values()
        .filter(|a| a.level != AlertLevel::None)
        .filter(|a| !dismissed.contains(&a.key))
        .cloned()
        .collect();
    out.sort_by(|a, b| {
        severity_weight(b.level)
            .cmp(&severity_weight(a.level))
            .then_with(|| a.key.cmp(&b.key))
    });
    out
}

/// Count of alerts the bell badge should display. Same filter as
/// [`visible_alerts`] but returns a count to avoid an unnecessary clone in
/// the hot reactive path (the badge re-renders on every alert update).
#[must_use]
pub fn unread_count(alerts: &HashMap<String, SystemAlert>, dismissed: &HashSet<String>) -> usize {
    alerts
        .values()
        .filter(|a| a.level != AlertLevel::None)
        .filter(|a| !dismissed.contains(&a.key))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_countdown_ticks_down_and_clamps_at_zero() {
        let a = PendingApprovalView {
            id: "a1".to_string(),
            command: "bash".to_string(),
            agent_id: "main".to_string(),
            session_key: "gui:chat:main".to_string(),
            tool_call_id: None,
            reason: None,
            expires_at_ms: 60_000,
            allowed_decisions: Vec::new(),
        };
        assert_eq!(a.remaining_secs(0), 60);
        assert_eq!(a.remaining_secs(30_500), 29);
        // Past the deadline the row is stale, not negative.
        assert_eq!(a.remaining_secs(90_000), 0);
    }

    fn ask(session: &str) -> PendingAskView {
        PendingAskView {
            session_key: session.to_string(),
            question: "Deploy where?".to_string(),
            options: vec!["staging".to_string()],
            questions: vec![],
            answered: 0,
        }
    }

    /// `remaining()` is what the card renders from, so its two degenerate
    /// inputs must degrade rather than panic: an older core sends no
    /// structured view at all, and a frame that raced a completion can carry a
    /// cursor past the end.
    #[test]
    fn remaining_degrades_on_both_degenerate_inputs() {
        assert!(ask("s").remaining().is_empty(), "no structured view");

        let question = AskQuestionView {
            id: "q1".into(),
            header: None,
            prompt: "Which?".into(),
            options: vec![AskOptionView {
                label: "a".into(),
                description: Some("the first".into()),
            }],
            multi_select: false,
            secret: false,
        };
        let mut view = ask("s");
        view.questions = vec![question];

        assert_eq!(view.remaining().len(), 1);
        assert_eq!(
            view.remaining()[0].options[0].description.as_deref(),
            Some("the first"),
            "the description must survive into the card's own view type"
        );

        view.answered = 1;
        assert!(view.remaining().is_empty(), "cursor at the end");
        view.answered = 9;
        assert!(view.remaining().is_empty(), "cursor past the end");
    }

    #[test]
    fn pending_ask_is_scoped_to_the_waiting_conversation() {
        let pending = vec![ask("other"), ask("mine")];
        assert_eq!(
            pending_ask_for_session(&pending, Some("mine")).map(|p| p.session_key.as_str()),
            Some("mine")
        );
        // Another conversation's question must not render here — nor claim this
        // composer's Enter key.
        assert!(pending_ask_for_session(&pending, Some("nobody")).is_none());
        // A conversation with no session key yet cannot own a question.
        assert!(pending_ask_for_session(&pending, None).is_none());
    }

    fn mk(key: &str, level: AlertLevel) -> SystemAlert {
        SystemAlert {
            key: key.to_string(),
            level,
            count: None,
            message: None,
        }
    }

    #[test]
    fn empty_map_yields_nothing() {
        let alerts = HashMap::new();
        let dismissed = HashSet::new();
        assert!(visible_alerts(&alerts, &dismissed).is_empty());
        assert_eq!(unread_count(&alerts, &dismissed), 0);
    }

    #[test]
    fn none_level_is_filtered_out() {
        let mut alerts = HashMap::new();
        alerts.insert("a".to_string(), mk("a", AlertLevel::None));
        alerts.insert("b".to_string(), mk("b", AlertLevel::Info));
        let dismissed = HashSet::new();
        assert_eq!(unread_count(&alerts, &dismissed), 1);
        let list = visible_alerts(&alerts, &dismissed);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].key, "b");
    }

    #[test]
    fn dismissed_keys_are_skipped() {
        let mut alerts = HashMap::new();
        alerts.insert("a".to_string(), mk("a", AlertLevel::Warning));
        alerts.insert("b".to_string(), mk("b", AlertLevel::Critical));
        let mut dismissed = HashSet::new();
        dismissed.insert("b".to_string());
        let list = visible_alerts(&alerts, &dismissed);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].key, "a");
        assert_eq!(unread_count(&alerts, &dismissed), 1);
    }

    #[test]
    fn sorted_critical_first_then_warning_then_info() {
        let mut alerts = HashMap::new();
        alerts.insert("info".to_string(), mk("info", AlertLevel::Info));
        alerts.insert("crit".to_string(), mk("crit", AlertLevel::Critical));
        alerts.insert("warn".to_string(), mk("warn", AlertLevel::Warning));
        let list = visible_alerts(&alerts, &HashSet::new());
        assert_eq!(
            list.iter().map(|a| a.key.as_str()).collect::<Vec<_>>(),
            vec!["crit", "warn", "info"]
        );
    }

    #[test]
    fn equal_severity_sorts_by_key_lex() {
        let mut alerts = HashMap::new();
        alerts.insert("zeta".to_string(), mk("zeta", AlertLevel::Warning));
        alerts.insert("alpha".to_string(), mk("alpha", AlertLevel::Warning));
        let list = visible_alerts(&alerts, &HashSet::new());
        assert_eq!(
            list.iter().map(|a| a.key.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
    }

    #[test]
    fn dismissing_all_reduces_count_to_zero() {
        let mut alerts = HashMap::new();
        alerts.insert("a".to_string(), mk("a", AlertLevel::Warning));
        alerts.insert("b".to_string(), mk("b", AlertLevel::Critical));
        let mut dismissed = HashSet::new();
        dismissed.insert("a".to_string());
        dismissed.insert("b".to_string());
        assert_eq!(unread_count(&alerts, &dismissed), 0);
        assert!(visible_alerts(&alerts, &dismissed).is_empty());
    }
}
