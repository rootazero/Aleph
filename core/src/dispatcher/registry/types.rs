//! Tool Registry Types
//!
//! Core data structure for the ToolRegistry.

use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use super::super::types::UnifiedTool;

/// Shared tool storage type
pub type ToolStorage = Arc<RwLock<HashMap<String, UnifiedTool>>>;
