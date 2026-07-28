//! Task lifecycle — the single source of truth for which actions a
//! `CoordTask` in a given status may take, which backend move each action
//! is, and where a drag onto a board column lands.
//!
//! Consumed by three surfaces so they can never drift:
//! - the detail drawer's action footer (`task_drawer.rs`),
//! - the on-card hover quick-actions (`task_card.rs`),
//! - the kanban board's drag-and-drop drop routing (`board.rs`).
//!
//! The pure functions here (`actions_for_status`, `resolve_move`,
//! `TaskAction::*`) are total and host-testable without a DOM; the backend
//! remains the final authority on transition validity — these only hide the
//! obviously-invalid moves so the UI never offers a guaranteed no-op.

use leptos_i18n::I18nContext;

use crate::api::teams::{TaskPatch, TeamsApi};
use crate::context::DashboardState;
use crate::i18n::{t_string, Locale};

/// A lifecycle action a task can take. Mirrors the backend's verb + direct
/// status-write vocabulary exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAction {
    Start,
    Complete,
    Fail,
    Cancel,
    Pause,
    Resume,
    Skip,
    Retry,
    Approve,
    Reject,
}

/// A concrete backend move: either a direct status write (`teams.update_task`
/// with one of the five settable statuses) or a dedicated lifecycle verb.
/// Total + host-testable; `apply_move` is the one place that turns it into an
/// RPC call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskMove {
    /// `teams.update_task(status=…)` — one of pending / in_progress /
    /// completed / failed / cancelled (the only directly-settable statuses;
    /// blocked/unsatisfiable are derived and paused/skipped/waiting_review go
    /// through dedicated verbs).
    SetStatus(&'static str),
    Pause,
    Resume,
    Skip,
    Retry,
    Approve,
    /// Reject a waiting-review task. The drawer captures an optional reason;
    /// drag/quick-action paths send none.
    Reject,
}

impl TaskMove {
    /// Whether applying this move lands the task in a hard-to-reverse state, so
    /// a drag drop should confirm first. Mirrors [`TaskAction::is_destructive`]
    /// for the post-`resolve_move` value the DnD path actually holds.
    #[must_use]
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            Self::SetStatus("completed")
                | Self::SetStatus("failed")
                | Self::SetStatus("cancelled")
                | Self::Skip
                | Self::Reject
        )
    }
}

impl TaskAction {
    /// The board column a task lands in after this action succeeds. Used to
    /// derive drag-drop routing from the same table that gates the drawer, so
    /// the two can never disagree.
    #[must_use]
    pub const fn target_column(self) -> &'static str {
        match self {
            Self::Start => "in_progress",
            Self::Complete | Self::Approve => "completed",
            Self::Fail | Self::Reject => "failed",
            Self::Cancel => "cancelled",
            Self::Pause => "paused",
            // Resume returns a paused task to pending; Retry re-queues a failed
            // task as pending. Both land in the Pending column.
            Self::Resume | Self::Retry => "pending",
            Self::Skip => "skipped",
        }
    }

    /// The concrete backend move this action performs.
    #[must_use]
    pub const fn to_move(self) -> TaskMove {
        match self {
            Self::Start => TaskMove::SetStatus("in_progress"),
            Self::Complete => TaskMove::SetStatus("completed"),
            Self::Fail => TaskMove::SetStatus("failed"),
            Self::Cancel => TaskMove::SetStatus("cancelled"),
            Self::Pause => TaskMove::Pause,
            Self::Resume => TaskMove::Resume,
            Self::Skip => TaskMove::Skip,
            Self::Retry => TaskMove::Retry,
            Self::Approve => TaskMove::Approve,
            Self::Reject => TaskMove::Reject,
        }
    }

    /// Whether this action ends the task in a hard-to-reverse state and thus
    /// warrants a confirmation before a drag applies it. Explicit drawer
    /// clicks are intentional and skip the prompt; a stray drop should not.
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        matches!(
            self,
            Self::Complete | Self::Fail | Self::Cancel | Self::Skip | Self::Reject
        )
    }
}

/// Which lifecycle actions the UI offers for a task in `status`.
///
/// Pure + total so it is unit-testable without a DOM. Terminal and unknown
/// statuses expose nothing. For any single status the returned actions target
/// distinct columns, which is what lets [`resolve_move`] pick at most one.
#[must_use]
pub fn actions_for_status(status: &str) -> Vec<TaskAction> {
    use TaskAction::{Approve, Cancel, Complete, Fail, Pause, Reject, Resume, Retry, Skip, Start};
    match status {
        "pending" => vec![Start, Pause, Skip, Cancel],
        "blocked" | "unsatisfiable" => vec![Pause, Skip, Cancel],
        "in_progress" => vec![Complete, Fail, Pause, Cancel],
        "waiting_review" => vec![Approve, Reject, Skip, Cancel],
        "paused" => vec![Resume, Cancel],
        "failed" => vec![Retry],
        _ => vec![],
    }
}

/// Whether a card in `status` can be dragged at all — true iff at least one
/// legal move exists from it. Terminal cards (completed/skipped/cancelled) are
/// drag-inert.
#[must_use]
pub fn is_draggable(status: &str) -> bool {
    !actions_for_status(status).is_empty()
}

/// The one or two most-relevant actions to surface directly on a card (hover
/// quick-actions), a curated subset of [`actions_for_status`] — the full set
/// stays in the drawer. Keeps the card uncluttered while covering the common
/// forward move for each state.
#[must_use]
pub fn primary_actions(status: &str) -> Vec<TaskAction> {
    use TaskAction::{Approve, Complete, Reject, Resume, Retry, Start};
    match status {
        "pending" => vec![Start],
        "in_progress" => vec![Complete],
        "waiting_review" => vec![Approve, Reject],
        "paused" => vec![Resume],
        "failed" => vec![Retry],
        _ => vec![],
    }
}

/// Localized label for a lifecycle action. `t!` / `t_string!` take compile-time
/// key paths, so the runtime action resolves through this match — the single
/// source shared by the drawer footer and the on-card quick-actions.
pub fn action_label(i18n: I18nContext<Locale>, action: TaskAction) -> String {
    match action {
        TaskAction::Start => t_string!(i18n, teams.kanban.actions.start).to_string(),
        TaskAction::Complete => t_string!(i18n, teams.kanban.actions.complete).to_string(),
        TaskAction::Fail => t_string!(i18n, teams.kanban.actions.fail).to_string(),
        TaskAction::Cancel => t_string!(i18n, teams.kanban.actions.cancel).to_string(),
        TaskAction::Pause => t_string!(i18n, teams.kanban.actions.pause).to_string(),
        TaskAction::Resume => t_string!(i18n, teams.kanban.actions.resume).to_string(),
        TaskAction::Skip => t_string!(i18n, teams.kanban.actions.skip).to_string(),
        TaskAction::Retry => t_string!(i18n, teams.kanban.actions.retry).to_string(),
        TaskAction::Approve => t_string!(i18n, teams.kanban.actions.approve).to_string(),
        TaskAction::Reject => t_string!(i18n, teams.kanban.actions.reject).to_string(),
    }
}

/// Resolve a drag from a card in `from_status` dropped onto the board column
/// `to_col` into the concrete backend move, or `None` when no legal transition
/// reaches that column (self-drops, derived columns, terminal targets).
///
/// Derived entirely from [`actions_for_status`] + [`TaskAction::target_column`]
/// so the drop routing and the drawer footer share one rule set. `blocked` and
/// `unsatisfiable` are never drop targets (no action lands there); a `blocked`
/// card can still be *dragged* out to paused/skipped/cancelled.
#[must_use]
pub fn resolve_move(from_status: &str, to_col: &str) -> Option<TaskMove> {
    actions_for_status(from_status)
        .into_iter()
        .find(|a| a.target_column() == to_col)
        .map(TaskAction::to_move)
}

/// Execute a [`TaskMove`] against the backend. The one shared dispatch point
/// for the drawer, the card quick-actions, and drag-drop; every caller routes
/// here so a new verb is wired in exactly one place.
pub async fn apply_move(dash: &DashboardState, task_id: &str, mv: TaskMove) -> Result<(), String> {
    match mv {
        TaskMove::SetStatus(status) => TeamsApi::update_task(
            dash,
            task_id,
            TaskPatch {
                status: Some(status.to_string()),
                ..Default::default()
            },
        )
        .await
        .map(|_| ()),
        TaskMove::Pause => TeamsApi::task_pause(dash, task_id).await,
        TaskMove::Resume => TeamsApi::task_resume(dash, task_id).await,
        TaskMove::Skip => TeamsApi::task_skip(dash, task_id).await,
        TaskMove::Retry => TeamsApi::task_retry(dash, task_id).await,
        TaskMove::Approve => TeamsApi::task_approve(dash, task_id).await,
        TaskMove::Reject => TeamsApi::task_reject(dash, task_id, None).await,
    }
}

#[cfg(test)]
mod tests {
    use super::{actions_for_status, is_draggable, resolve_move, TaskAction, TaskMove};

    #[test]
    fn gating_matches_lifecycle_rules() {
        use TaskAction::{
            Approve, Cancel, Complete, Fail, Pause, Reject, Resume, Retry, Skip, Start,
        };
        assert_eq!(
            actions_for_status("pending"),
            vec![Start, Pause, Skip, Cancel]
        );
        assert_eq!(actions_for_status("blocked"), vec![Pause, Skip, Cancel]);
        // "unsatisfiable" (derived blocked) mirrors blocked exactly.
        assert_eq!(
            actions_for_status("unsatisfiable"),
            actions_for_status("blocked")
        );
        assert_eq!(
            actions_for_status("in_progress"),
            vec![Complete, Fail, Pause, Cancel]
        );
        assert_eq!(
            actions_for_status("waiting_review"),
            vec![Approve, Reject, Skip, Cancel]
        );
        assert_eq!(actions_for_status("paused"), vec![Resume, Cancel]);
        assert_eq!(actions_for_status("failed"), vec![Retry]);
        for s in ["completed", "skipped", "cancelled", "garbage"] {
            assert!(
                actions_for_status(s).is_empty(),
                "{s} must be terminal/inert"
            );
        }
    }

    #[test]
    fn each_status_targets_distinct_columns() {
        // The resolve_move `.find()` is only unambiguous if, per status, no two
        // offered actions land in the same column.
        for status in [
            "pending",
            "blocked",
            "in_progress",
            "waiting_review",
            "paused",
            "failed",
        ] {
            let cols: Vec<&str> = actions_for_status(status)
                .iter()
                .map(|a| a.target_column())
                .collect();
            let mut dedup = cols.clone();
            dedup.sort_unstable();
            dedup.dedup();
            assert_eq!(
                cols.len(),
                dedup.len(),
                "{status} has colliding target columns"
            );
        }
    }

    #[test]
    fn resolve_move_routes_drags_to_verbs() {
        // Direct status writes.
        assert_eq!(
            resolve_move("pending", "in_progress"),
            Some(TaskMove::SetStatus("in_progress"))
        );
        assert_eq!(
            resolve_move("in_progress", "completed"),
            Some(TaskMove::SetStatus("completed"))
        );
        assert_eq!(
            resolve_move("in_progress", "failed"),
            Some(TaskMove::SetStatus("failed"))
        );
        assert_eq!(
            resolve_move("pending", "cancelled"),
            Some(TaskMove::SetStatus("cancelled"))
        );
        // Dedicated verbs.
        assert_eq!(resolve_move("pending", "paused"), Some(TaskMove::Pause));
        assert_eq!(resolve_move("paused", "pending"), Some(TaskMove::Resume));
        assert_eq!(resolve_move("failed", "pending"), Some(TaskMove::Retry));
        assert_eq!(resolve_move("pending", "skipped"), Some(TaskMove::Skip));
        // Review gate: approve → completed, reject → failed.
        assert_eq!(
            resolve_move("waiting_review", "completed"),
            Some(TaskMove::Approve)
        );
        assert_eq!(
            resolve_move("waiting_review", "failed"),
            Some(TaskMove::Reject)
        );
    }

    #[test]
    fn resolve_move_rejects_illegal_and_self_drops() {
        // Self-drop: no action targets the card's own column.
        assert_eq!(resolve_move("pending", "pending"), None);
        assert_eq!(resolve_move("in_progress", "in_progress"), None);
        // Derived / non-settable columns are never drop targets.
        assert_eq!(resolve_move("pending", "blocked"), None);
        assert_eq!(resolve_move("pending", "waiting_review"), None);
        assert_eq!(resolve_move("in_progress", "unsatisfiable"), None);
        // Terminal cards can't be dragged anywhere.
        assert_eq!(resolve_move("completed", "pending"), None);
        assert_eq!(resolve_move("cancelled", "failed"), None);
        // waiting_review can't be paused (backend rejects it); no action offers it.
        assert_eq!(resolve_move("waiting_review", "paused"), None);
    }

    #[test]
    fn destructive_moves_flagged_for_confirm() {
        for a in [
            TaskAction::Complete,
            TaskAction::Fail,
            TaskAction::Cancel,
            TaskAction::Skip,
            TaskAction::Reject,
        ] {
            assert!(a.is_destructive(), "{a:?} should require confirm");
        }
        for a in [
            TaskAction::Start,
            TaskAction::Pause,
            TaskAction::Resume,
            TaskAction::Retry,
            TaskAction::Approve,
        ] {
            assert!(!a.is_destructive(), "{a:?} should apply without confirm");
        }
    }

    #[test]
    fn move_destructiveness_matches_action() {
        // Every action's destructiveness must survive the trip through
        // `to_move()`, since the DnD path decides confirm from the `TaskMove`.
        for a in [
            TaskAction::Start,
            TaskAction::Complete,
            TaskAction::Fail,
            TaskAction::Cancel,
            TaskAction::Pause,
            TaskAction::Resume,
            TaskAction::Skip,
            TaskAction::Retry,
            TaskAction::Approve,
            TaskAction::Reject,
        ] {
            assert_eq!(
                a.is_destructive(),
                a.to_move().is_destructive(),
                "{a:?} destructiveness must round-trip through to_move()"
            );
        }
    }

    #[test]
    fn primary_actions_are_a_subset_of_available() {
        use super::primary_actions;
        for s in [
            "pending",
            "in_progress",
            "waiting_review",
            "paused",
            "failed",
            "blocked",
            "completed",
        ] {
            let full = actions_for_status(s);
            for a in primary_actions(s) {
                assert!(
                    full.contains(&a),
                    "primary action {a:?} for {s} not in actions_for_status"
                );
            }
        }
    }

    #[test]
    fn draggability_tracks_available_actions() {
        for s in [
            "pending",
            "blocked",
            "in_progress",
            "waiting_review",
            "paused",
            "failed",
        ] {
            assert!(is_draggable(s), "{s} should be draggable");
        }
        for s in ["completed", "skipped", "cancelled"] {
            assert!(!is_draggable(s), "{s} is terminal, not draggable");
        }
    }
}
