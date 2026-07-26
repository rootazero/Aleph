//! Tool execution context — workspace-scoped paths for tool output.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::sync_primitives::Arc;

/// Runtime context providing workspace-scoped output paths to tools.
///
/// Injected via shared handle on `BuiltinToolRegistry`.
/// Tools that need output paths read from the handle; others ignore it.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Workspace output directory (e.g. ~/.`aleph/workspaces/{agent_id}/output`/)
    pub output_dir: PathBuf,
}

impl ToolContext {
    /// Build from a resolved workspace path, creating directories if needed.
    pub fn from_workspace(workspace_path: &Path) -> Result<Self> {
        let output_dir = workspace_path.join("output");

        fs::create_dir_all(&output_dir).map_err(|e| {
            crate::error::AlephError::config(format!(
                "Failed to create output directory {}: {}",
                output_dir.display(),
                e
            ))
        })?;

        Ok(Self { output_dir })
    }
}

/// Type alias for the shared handle, matching existing handle patterns.
pub type ToolContextHandle = Arc<tokio::sync::RwLock<ToolContext>>;

/// Create a new `ToolContext` handle with default paths (main workspace).
#[must_use]
pub fn new_tool_context_handle() -> ToolContextHandle {
    let default_workspace = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".aleph")
        .join("workspaces")
        .join("main");
    let ctx = ToolContext::from_workspace(&default_workspace).unwrap_or_else(|e| {
        tracing::warn!(
            error = %e,
            path = %default_workspace.display(),
            "Failed to create default workspace output dir; using fallback"
        );
        let output_dir = default_workspace.join("output");
        if let Err(e) = std::fs::create_dir_all(&output_dir) {
            tracing::error!(
                error = %e,
                path = %output_dir.display(),
                "Failed to create fallback output dir; tools may fail"
            );
        }
        ToolContext { output_dir }
    });
    Arc::new(tokio::sync::RwLock::new(ctx))
}
