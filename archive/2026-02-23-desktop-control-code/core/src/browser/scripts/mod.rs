//! JavaScript Injection Scripts
//!
//! This module contains JavaScript code that is injected into browser pages
//! for various automation tasks.

pub mod accessibility;
pub mod freeze;
pub mod resume;

pub use accessibility::get_accessibility_tree_script;
pub use freeze::get_freeze_context_script;
pub use resume::get_resume_context_script;
