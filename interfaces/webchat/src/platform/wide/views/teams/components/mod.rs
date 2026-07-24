//! Kanban sub-components for the Teams tab.

pub mod board;
pub mod board_columns;
pub mod column;
pub mod create_form;
pub mod lifecycle;
pub mod task_card;
pub mod task_drawer;
pub mod team_selector;

/// Format a unix-epoch (seconds) timestamp as a coarse human delta
/// (e.g. "just now", "5 min ago", "3 h ago", "2 d ago"). Shared by the
/// task card and the detail drawer.
#[must_use]
pub fn format_relative_time(epoch_secs: u64) -> String {
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let delta = now.saturating_sub(epoch_secs);
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{} min ago", delta / 60)
    } else if delta < 86_400 {
        format!("{} h ago", delta / 3600)
    } else {
        format!("{} d ago", delta / 86_400)
    }
}
