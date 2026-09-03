//! Session handlers operating against the `SessionStore` trait.
//!
//! All `handle_*_db` functions operate against any `SessionStore` implementation
//! (`SQLite` or file backend) for production use.

mod checkpoint;
mod create;
mod modify;
mod query;
mod types;

pub use checkpoint::{
    handle_branch_checkpoint_db, handle_list_checkpoints_db, handle_restore_checkpoint_db,
};
pub use create::{handle_create_db, handle_new_session_db};
pub use modify::{
    handle_compact_db, handle_delete_db, handle_delete_db_with_capture, handle_patch_db,
    handle_reset_db, handle_set_project_root_db, handle_set_topic_db, handle_truncate_db,
};
pub use query::{handle_history_db, handle_list_db, handle_preview_db, handle_usage_db};
pub use types::HistoryMessage;
