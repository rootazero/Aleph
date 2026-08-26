//! Shared, framework-agnostic UI state helpers.
//!
//! Pure logic that every UI surface (panel, TUI, mobile) can reuse without
//! pulling in Leptos or `web_sys`. Side-effectful signal wiring stays in the
//! individual surfaces; only the *decisions* live here so they stay testable.

pub mod chat_scroll;
pub mod composer_dials;
pub mod composer_queue;
pub mod team_chat;

pub use chat_scroll::{scroll_action, ListCursor, ScrollAction};
pub use composer_dials::{session_dials_for_send, SendDials, SessionKnobs};
pub use composer_queue::{
    merge_recalled_draft, should_auto_drain_on_settle, should_flush_on_turn_boundary,
    should_recall_on_bare_arrow_up, was_busy_across_switch,
};
pub use team_chat::remember_own_message_id;
