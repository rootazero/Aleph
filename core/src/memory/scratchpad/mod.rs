// core/src/memory/scratchpad/mod.rs

//! Session Scratchpad Module
//!
//! Provides working memory for active tasks, stored as project-local
//! Markdown files that are immune to compression.

pub mod template;
mod manager;
mod history;

pub use manager::ScratchpadManager;
pub use history::SessionHistory;
pub use template::{DEFAULT_TEMPLATE, generate_scratchpad};
