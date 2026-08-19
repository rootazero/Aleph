//! Tool Registry Types
//!
//! Core data structure for the `ToolCatalog`.

use crate::sync_primitives::{Arc, AsyncRwLock};
use std::collections::HashMap;

use super::super::types::UnifiedTool;

/// Shared tool storage type
pub type ToolStorage = Arc<AsyncRwLock<HashMap<String, UnifiedTool>>>;

/// Result of resolving a user slash command
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ResolvedCommand {
    /// The matched tool
    pub tool: UnifiedTool,
    /// Parsed arguments (text after command name)
    pub arguments: Option<String>,
    /// Original user input
    pub raw_input: String,
}
