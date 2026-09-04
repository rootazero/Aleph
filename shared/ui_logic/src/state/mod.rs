//! Shared, framework-agnostic UI state helpers.
//!
//! Pure logic that every UI surface (panel, TUI, mobile) can reuse without
//! pulling in Leptos or `web_sys`. Side-effectful signal wiring stays in the
//! individual surfaces; only the *decisions* live here so they stay testable.

pub mod agent_panel;
#[cfg(test)]
mod agent_panel_parity;
pub mod chat_scroll;
pub mod composer_dials;
pub mod composer_queue;
pub mod team_chat;

pub use agent_panel::{
    attention_rank, quiet_age, sort_entries, state_glyph, AgentPanelState, QuietAge, QuietUnit,
    MAX_SPLIT_RATIO, MIN_SPLIT_RATIO,
};
pub use chat_scroll::{scroll_action, ListCursor, ScrollAction};
pub use composer_dials::{session_dials_for_send, SendDials, SessionKnobs};
pub use composer_queue::{
    merge_recalled_draft, should_auto_drain_on_settle, should_flush_on_turn_boundary,
    should_recall_on_bare_arrow_up, was_busy_across_switch,
};
pub use team_chat::remember_own_message_id;
