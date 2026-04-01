//! Session Handlers
//!
//! RPC handlers for session management: list, history, reset, send.
//!
//! Provides two sets of handlers:
//! - In-memory handlers using SessionStore (for development/testing)
//! - Database-backed handlers using SessionManager (for production)

mod db_handlers;
mod store;

// Re-export shared types
pub use db_handlers::{
    handle_compact_db, handle_create_db, handle_delete_db, handle_history_db, handle_list_db,
    handle_new_session_db, handle_reset_db, handle_set_topic_db, handle_usage_db,
};
pub use store::{handle_compact, handle_delete, handle_history, handle_list, handle_reset};
pub use store::{HistoryMessage, SessionInfo, SessionStore};
