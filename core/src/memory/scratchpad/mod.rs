// core/src/memory/scratchpad/mod.rs

//! Session Scratchpad Module
//!
//! Provides working memory for active tasks, stored as project-local
//! Markdown files that are immune to compression.
//!
//! ## Architecture
//!
//! - **scratchpad.md**: Current active task state
//! - **session_history.log**: Archive of completed tasks
//!
//! ## Usage
//!
//! ```rust,ignore
//! let manager = ScratchpadManager::new(project_root, "session-id");
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
