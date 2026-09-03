//! Session Handlers
//!
//! RPC handlers for session management using the SQLite-backed `SessionManager`.

mod db_handlers;

pub use db_handlers::{
    handle_branch_checkpoint_db, handle_compact_db, handle_create_db, handle_delete_db,
    handle_delete_db_with_capture, handle_history_db, handle_list_checkpoints_db, handle_list_db,
    handle_new_session_db, handle_patch_db, handle_preview_db, handle_reset_db,
    handle_restore_checkpoint_db, handle_set_project_root_db, handle_set_topic_db,
    handle_truncate_db, handle_usage_db, HistoryMessage,
};
