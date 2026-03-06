//! Tool Registry Types
//!
//! Core data structure for the ToolRegistry.

use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use super::super::types::UnifiedTool;

/// Shared tool storage type
pub type ToolStorage = Arc<RwLock<HashMap<String, UnifiedTool>>>;

/// Result of resolving a user slash command
#[derive(Debug, Clone)]
pub struct ResolvedCommand {
    /// The matched tool
    pub tool: UnifiedTool,
    /// Parsed arguments (text after command name)
    pub arguments: Option<String>,
    /// Original user input
    pub raw_input: String,
}
