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
///
/// Resolves the workspace root through [`crate::utils::paths::get_config_dir`]
/// so it honors `ALEPH_HOME` — `<aleph_home>/workspaces/main` — instead of
/// collapsing to the real `~/.aleph/workspaces/main` on any machine that
/// picked a different home. Hand-rolling the path off `dirs::home_dir()` (the
/// pre-fix behaviour) was a home-isolation hole: two instances with different
/// `ALEPH_HOME` values would both write into the real `~/.aleph` and the
/// isolation knob silently stopped covering the tool-output path.
///
/// Falls back to `<temp>/.aleph/workspaces/main` only when `get_config_dir`
/// itself fails (no home directory at all). Tool output is best-effort there
/// — every existing fail-closed path retains its old behaviour.
#[must_use]
pub fn new_tool_context_handle() -> ToolContextHandle {
    let default_workspace = crate::utils::paths::get_config_dir()
        .map(|c| c.join("workspaces").join("main"))
        .unwrap_or_else(|_| {
            PathBuf::from("/tmp")
                .join(".aleph")
                .join("workspaces")
                .join("main")
        });
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
