//! Agent Manager — TOML CRUD for agent definitions
//!
//! Provides create, read, update, delete operations on the `[[agents.list]]`
//! section of the config file, plus workspace file management for each agent.
//!
//! Uses `toml_edit` for format-preserving edits and atomic file saves.

mod agent_files;
mod crud;
mod toml_ops;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::types::agents_def::{
    AgentIdentity, AgentModelConfig, AgentParams, SubagentPolicy,
};

// =============================================================================
// Constants
// =============================================================================

/// Identity files recognized in agent directories.
///
/// Note: `MEMORY.md` lives alongside but is owned by the curated memory
/// module — it is not flagged as a bootstrap file here, matching the
/// `IDENTITY_FILE_NAMES` list in `src/thinker/identity_files.rs`.
pub(super) const BOOTSTRAP_FILES: &[&str] = &[
    "SOUL.md",
    "IDENTITY.md",
    "AGENTS.md",
    "TOOLS.md",
    "HEARTBEAT.md",
];

/// Maximum length for agent IDs
pub(super) const MAX_ID_LENGTH: usize = 32;

// =============================================================================
// AgentPatch
// =============================================================================

/// Partial update for an agent definition
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentPatch {
    pub name: Option<String>,
    pub identity: Option<AgentIdentity>,
    pub model_config: Option<AgentModelConfig>,
    pub params: Option<AgentParams>,
    pub skills: Option<Vec<String>>,
    pub skills_blacklist: Option<Vec<String>>,
    pub subagents: Option<SubagentPolicy>,
    pub allowed_links: Option<Vec<String>>,
}

// =============================================================================
// WorkspaceFile
// =============================================================================

/// Metadata for a file in an agent's workspace directory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceFile {
    pub filename: String,
    pub size_bytes: u64,
    pub modified_at: i64,
    pub is_bootstrap: bool,
}

// =============================================================================
// AgentManager
// =============================================================================

/// Manages agent definitions in the TOML config and their workspace directories
pub struct AgentManager {
    pub(super) config_path: PathBuf,
    pub workspace_root: PathBuf,
    pub agents_root: PathBuf,
    pub(super) trash_root: PathBuf,
}
