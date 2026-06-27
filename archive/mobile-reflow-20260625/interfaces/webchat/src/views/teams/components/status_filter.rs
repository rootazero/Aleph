//! `StatusFilter` — mobile Kanban single-status selector (§11 P-④).
//!
//! Drives `TeamsTabState::task_status_filter`. `None` ⇒ show every status
//! (mobile renders the full single-column board); `Some(s)` ⇒ show only that
//! status' column. Status is a raw wire string (no `TaskStatus` enum exists in
//! this crate — `CoordTaskDto.status` is `String`).

use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;

/// The six derived Kanban statuses, in board column order. `unsatisfiable`
/// is intentionally absent: board groups it under `blocked`, and the filter
/// mirrors that by folding it into the `blocked` option (see `status_matches`).
pub const STATUS_OPTIONS: &[&str] = &[
    "pending",
    "blocked",
    "in_progress",
    "completed",
    "failed",
    "cancelled",
];

/// Pure predicate: does a task with `task_status` pass the active `filter`?
/// `None` filter passes everything. The `blocked` filter also matches
/// `unsatisfiable` (board.rs groups them in one column).
pub fn status_matches(task_status: &str, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some("blocked") => task_status == "blocked" || task_status == "unsatisfiable",
        Some(s) => task_status == s,
    }
}

/// Mobile-only single-status selector. Reads/writes the shared
/// `task_status_filter` signal; an empty value ⇒ `None` (all statuses).
#[component]
#[must_use]
pub fn StatusFilter(value: RwSignal<Option<String>>) -> impl IntoView {
    let i18n = use_i18n();
    let all_label = move || t_string!(i18n, teams.kanban.filter.all).to_string();

    view! {
        <div class="max-sm:block hidden px-3 pb-2">
            <select
                class="w-full px-2 py-1.5 rounded bg-surface-sunken border border-border text-sm text-text-primary focus:outline-none focus:border-border-strong"
                on:change=move |ev| {
                    let v = event_target_value(&ev);
                    value.set(if v.is_empty() { None } else { Some(v) });
                }
                prop:value=move || value.get().unwrap_or_default()
            >
                <option value="">{all_label}</option>
                {STATUS_OPTIONS.iter().map(|s| {
                    let s = (*s).to_string();
                    let label = s.replace('_', " ");
                    view! { <option value=s.clone()>{label}</option> }
                }).collect_view()}
            </select>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_filter_passes_every_status() {
        for s in STATUS_OPTIONS {
            assert!(status_matches(s, None), "{s} should pass under None");
        }
        assert!(status_matches("unsatisfiable", None));
    }

    #[test]
    fn some_filter_matches_only_that_status() {
        assert!(status_matches("completed", Some("completed")));
        assert!(!status_matches("pending", Some("completed")));
        assert!(!status_matches("failed", Some("completed")));
    }

    #[test]
    fn blocked_filter_also_matches_unsatisfiable() {
        assert!(status_matches("blocked", Some("blocked")));
        assert!(status_matches("unsatisfiable", Some("blocked")));
        assert!(!status_matches("pending", Some("blocked")));
    }

    #[test]
    fn unsatisfiable_does_not_leak_into_failed() {
        // unsatisfiable is grouped with blocked, NOT failed — guard the boundary.
        assert!(!status_matches("unsatisfiable", Some("failed")));
    }
}
