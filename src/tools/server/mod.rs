//! Tool Server with Hot-Reload Support
//!
//! Provides a thread-safe tool registry that supports runtime
//! addition and removal of tools.

mod ops;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::sync_primitives::{Arc, Mutex};
use crate::tools::traits::AlephToolDyn;
use crate::tools::types::ToolUpdateInfo;

use ops::{list_tools_arc_impl, replace_tool_arc_impl};

// =============================================================================
// Shared type alias
// =============================================================================

/// The shared, lock-protected tool registry used by the server.
///
/// Uses `crate::sync_primitives::Mutex` so the production hot-reload paths
/// (`replace_tool`) can register tools without blocking the async runtime.
type ToolMap = Arc<Mutex<HashMap<String, Arc<dyn AlephToolDyn>>>>;

// =============================================================================
// AlephToolServer
// =============================================================================

/// Thread-safe tool server with hot-reload support.
///
/// This server manages a collection of tools that can be replaced at
/// runtime. The live surface is intentionally narrow:
/// - [`new`](Self::new) / [`Default`] for construction
/// - [`replace_tool`](Self::replace_tool) for the live markdown-skill reload path
/// - [`list_tools_arc`](Self::list_tools_arc) for the agent loop factory
pub struct AlephToolServer {
    tools: ToolMap,
}

impl AlephToolServer {
    /// Create a new empty tool server.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Replace or add a tool with explicit update semantics.
    ///
    /// Returns [`ToolUpdateInfo`] describing whether an existing tool was
    /// replaced.
    pub async fn replace_tool(&self, tool: impl AlephToolDyn + 'static) -> ToolUpdateInfo {
        replace_tool_arc_impl(&self.tools, Arc::new(tool)).await
    }

    /// List all registered tools as `Arc<dyn AlephToolDyn>`.
    ///
    /// Used by the minimal agent loop factory to wrap tools via adapters.
    pub async fn list_tools_arc(&self) -> Vec<Arc<dyn AlephToolDyn>> {
        list_tools_arc_impl(&self.tools).await
    }
}

impl Default for AlephToolServer {
    fn default() -> Self {
        Self::new()
    }
}
