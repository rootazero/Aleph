//! Canonical kanban column table — the single source of truth for column
//! order, colour tone, and status→label mapping.
//!
//! Both the board grid (`board.rs`) and the stats chip bar (`kanban.rs`)
//! consume this list, so the two can never drift: before this module the chip
//! bar hard-coded a 6-status subset while the board rendered 9, silently
//! hiding `waiting_review` / `paused` / `skipped` counts and folding
//! `unsatisfiable` inconsistently.
//!
//! `t!` / `t_string!` take compile-time key paths, so a runtime status string
//! resolves through [`column_label`]'s explicit match — the same pattern as
//! `components/exec_tier_labels.rs`.

use leptos_i18n::I18nContext;

use crate::api::teams::CoordTaskDto;
use crate::i18n::{t_string, Locale};

/// Colour role for a column header dot / stats chip. Kept small; several
/// columns intentionally share a tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Muted,
    Info,
    Primary,
    Success,
    Warning,
    Danger,
}

impl Tone {
    /// Background + text classes for a filled stats chip.
    #[must_use]
    pub const fn chip_class(self) -> &'static str {
        match self {
            Self::Neutral => "bg-surface-sunken text-text-secondary",
            Self::Muted => "bg-surface-sunken text-text-tertiary",
            Self::Info => "bg-info/10 text-info",
            Self::Primary => "bg-primary/10 text-primary",
            Self::Success => "bg-success/10 text-success",
            Self::Warning => "bg-warning/10 text-warning",
            Self::Danger => "bg-danger/10 text-danger",
        }
    }

    /// Solid background for the small column-header status dot.
    #[must_use]
    pub const fn dot_class(self) -> &'static str {
        match self {
            Self::Neutral => "bg-text-tertiary/50",
            Self::Muted => "bg-text-tertiary/40",
            Self::Info => "bg-info",
            Self::Primary => "bg-primary",
            Self::Success => "bg-success",
            Self::Warning => "bg-warning",
            Self::Danger => "bg-danger",
        }
    }
}

/// One board column: the stored status key it groups and its colour tone.
#[derive(Debug, Clone, Copy)]
pub struct BoardColumn {
    /// Matches `CoordTaskStatus::as_str()`. The `blocked` column also absorbs
    /// the derived `unsatisfiable` status (see [`column_matches`]).
    pub status: &'static str,
    pub tone: Tone,
}

/// The canonical column order rendered by the board and summarised by the
/// chip bar. `unsatisfiable` is not its own column — it folds into `blocked`.
pub const BOARD_COLUMNS: &[BoardColumn] = &[
    BoardColumn { status: "pending", tone: Tone::Neutral },
    BoardColumn { status: "blocked", tone: Tone::Warning },
    BoardColumn { status: "in_progress", tone: Tone::Info },
    BoardColumn { status: "waiting_review", tone: Tone::Primary },
    BoardColumn { status: "paused", tone: Tone::Muted },
    BoardColumn { status: "completed", tone: Tone::Success },
    BoardColumn { status: "skipped", tone: Tone::Muted },
    BoardColumn { status: "failed", tone: Tone::Danger },
    BoardColumn { status: "cancelled", tone: Tone::Muted },
];

/// Whether a task with `task_status` belongs in the column for `col_status`.
/// The `blocked` column absorbs `unsatisfiable` (a refinement of blocked: a
/// dependency terminally failed) so those tasks stay visible instead of
/// vanishing. Every task maps to at most one column, so counts never
/// double-tally.
#[must_use]
pub fn column_matches(task_status: &str, col_status: &str) -> bool {
    if col_status == "blocked" {
        task_status == "blocked" || task_status == "unsatisfiable"
    } else {
        task_status == col_status
    }
}

/// Tasks belonging in the column for `col_status`, cloned for signal use.
#[must_use]
pub fn tasks_for_column(tasks: &[CoordTaskDto], col_status: &str) -> Vec<CoordTaskDto> {
    tasks
        .iter()
        .filter(|t| column_matches(&t.status, col_status))
        .cloned()
        .collect()
}

/// Count of tasks in the column for `col_status`.
#[must_use]
pub fn count_for_column(tasks: &[CoordTaskDto], col_status: &str) -> usize {
    tasks
        .iter()
        .filter(|t| column_matches(&t.status, col_status))
        .count()
}

/// Localized label for a stored `CoordTaskStatus` key. An unknown key degrades
/// to the raw string rather than rendering blank.
///
/// Serves both the kanban column headers/chips and the team-chat task strip
/// and drawer. `unsatisfiable` has no column of its own (it folds into
/// `blocked` on the board) but is a real stored status a task can carry, so it
/// still needs a label wherever tasks are listed rather than bucketed.
pub fn column_label(i18n: I18nContext<Locale>, status: &str) -> String {
    match status {
        "pending" => t_string!(i18n, teams.kanban.columns.pending).to_string(),
        "blocked" => t_string!(i18n, teams.kanban.columns.blocked).to_string(),
        "in_progress" => t_string!(i18n, teams.kanban.columns.in_progress).to_string(),
        "waiting_review" => t_string!(i18n, teams.kanban.columns.waiting_review).to_string(),
        "paused" => t_string!(i18n, teams.kanban.columns.paused).to_string(),
        "completed" => t_string!(i18n, teams.kanban.columns.completed).to_string(),
        "skipped" => t_string!(i18n, teams.kanban.columns.skipped).to_string(),
        "failed" => t_string!(i18n, teams.kanban.columns.failed).to_string(),
        "cancelled" => t_string!(i18n, teams.kanban.columns.cancelled).to_string(),
        "unsatisfiable" => t_string!(i18n, teams.kanban.columns.unsatisfiable).to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{column_matches, BOARD_COLUMNS};

    #[test]
    fn blocked_column_absorbs_unsatisfiable() {
        assert!(column_matches("blocked", "blocked"));
        assert!(column_matches("unsatisfiable", "blocked"));
        // …and unsatisfiable does NOT also land in any other column.
        for col in BOARD_COLUMNS.iter().filter(|c| c.status != "blocked") {
            assert!(
                !column_matches("unsatisfiable", col.status),
                "unsatisfiable leaked into {}",
                col.status
            );
        }
    }

    #[test]
    fn every_stored_status_maps_to_exactly_one_column() {
        // The 9 statuses a task can actually be observed in (blocked +
        // unsatisfiable are both routed by the blocked column).
        let stored = [
            "pending",
            "blocked",
            "unsatisfiable",
            "in_progress",
            "waiting_review",
            "paused",
            "completed",
            "skipped",
            "failed",
            "cancelled",
        ];
        for s in stored {
            let hits = BOARD_COLUMNS
                .iter()
                .filter(|c| column_matches(s, c.status))
                .count();
            assert_eq!(hits, 1, "status {s} must map to exactly one column, got {hits}");
        }
    }
}
