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

use crate::config::types::agents_def::{AgentIdentity, AgentModelRef, SubagentPolicy};

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

/// Serde helper for tri-state optional fields: absent JSON key → `None`,
/// explicit `null` → `Some(None)`, any value → `Some(Some(value))`.
fn deserialize_double_option<'de, D>(
    deserializer: D,
) -> Result<Option<Option<AgentModelRef>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<AgentModelRef>::deserialize(deserializer)?))
}

/// Partial update for an agent definition
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentPatch {
    pub name: Option<String>,
    pub identity: Option<AgentIdentity>,
    pub skills: Option<Vec<String>>,
    pub skills_blacklist: Option<Vec<String>>,
    pub subagents: Option<SubagentPolicy>,
    pub allowed_links: Option<Vec<String>>,
    /// Model update tri-state: absent (`None`) = unchanged; `Some(None)` = clear → inherit system default; `Some(Some(ref))` = set to this model.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub model: Option<Option<AgentModelRef>>,
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
