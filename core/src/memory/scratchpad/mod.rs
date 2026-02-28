// core/src/memory/scratchpad/mod.rs

//! Session Scratchpad Module
//!
//! Provides working memory for active tasks, stored as Markdown files
//! under `~/.aleph/projects/<project_name>/` that are immune to compression.
//!
//! Aleph is conversation-driven — projects are generated artifacts managed
//! by Aleph, not user-created directories entered via CLI. All project
//! working memory lives in the unified `~/.aleph/` workspace.
//!
//! ## Architecture
//!
//! - **scratchpad.md**: Current active task state
//! - **session_history.log**: Archive of completed tasks
//!
//! ## Usage
//!
//! ```rust,ignore
//! // Production: uses ~/.aleph/projects/<name>/
//! let manager = ScratchpadManager::new("my-blog", "session-id");
//!
//! // Testing: uses an explicit directory
//! let manager = ScratchpadManager::with_dir(temp_dir, "session-id");
//!
//! manager.initialize(Some("Build auth module")).await?;
//! manager.set_plan(&["Design API", "Implement", "Test"]).await?;
//! manager.complete_item(0).await?;
//! ```

pub mod template;
mod manager;
mod history;

pub use manager::{ScratchpadManager, ScratchpadConfig};
pub use history::{SessionHistory, HistoryEntry};
pub use template::{DEFAULT_TEMPLATE, generate_scratchpad};
