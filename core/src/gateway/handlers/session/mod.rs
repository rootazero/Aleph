//! Session Handlers
//!
//! RPC handlers for session management: list, history, reset, send.
//!
//! Provides two sets of handlers:
//! - In-memory handlers using SessionStore (for development/testing)
//! - Database-backed handlers using SessionManager (for production)

mod store;
mod db_handlers;

// Re-export shared types
pub use store::{SessionInfo, HistoryMessage, SessionStore};
pub use store::{handle_list, handle_history, handle_reset, handle_delete, handle_compact};
pub use db_handlers::{
    handle_list_db, handle_history_db, handle_reset_db, handle_delete_db,
    handle_usage_db, handle_create_db, handle_new_session_db, handle_compact_db,
    handle_set_topic_db,
};
