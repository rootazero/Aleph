//! Skill-related types for the extension system
//!
//! This module contains types for skills, commands, and skill tool invocation.

use crate::discovery::DiscoverySource;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// Forward declarations for types from other modules
use super::{PermissionRule, PromptScope};

// =============================================================================
// Skill Tool Types
// =============================================================================

/// Result of skill tool invocation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillToolResult {
    /// Display title (e.g., "Loaded skill: my-skill")
    pub title: String,

    /// Rendered skill content with templates expanded
    pub content: String,

    /// Base directory for relative path references
    pub base_dir: PathBuf,

    /// Skill metadata
    pub metadata: SkillMetadata,
}

/// Metadata about an invoked skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Skill name
    pub name: String,

    /// Fully qualified name (plugin:skill or skill)
    pub qualified_name: String,

    /// Discovery source
    pub source: DiscoverySource,
}

/// Context for skill tool invocation (passed from agent loop)
#[derive(Debug, Clone, Default)]
pub struct SkillContext {
    /// Session identifier
    pub session_id: String,

    /// Agent-level permission rules (if any)
    pub agent_permissions: Option<HashMap<String, PermissionRule>>,
}

/// Direct command execution result
///
/// Used by commands that execute immediately without LLM involvement
/// (e.g., `/status`, `/clear`, `/version`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectCommandResult {
    /// Command output to display to user
    pub content: String,
    /// Optional structured data
    pub data: Option<serde_json::Value>,
    /// Whether command was successful
    pub success: bool,
}

impl DirectCommandResult {
    /// Create a successful result with content only
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            data: None,
            success: true,
        }
    }

    /// Create a successful result with content and structured data
    pub fn with_data(content: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            content: content.into(),
            data: Some(data),
            success: true,
        }
    }

    /// Create an error result
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            data: None,
            success: false,
        }
    }
}

// =============================================================================
// Skill Types
// =============================================================================

/// Skill type (command vs skill)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SkillType {
    /// Command (from commands/ directory) - user-triggered via /command
    Command,
    /// Skill (from skills/ directory) - can be auto-invoked by LLM
    #[default]
    Skill,
}

/// Extension skill definition (unified with SkillRegistration).
///
/// This is now a type alias for `SkillRegistration`, which contains all fields
/// needed for both plugin-registered and filesystem-discovered skills.
pub type ExtensionSkill = crate::extension::SkillRegistration;

// =============================================================================
// Command Types (alias for user-triggered skills)
// =============================================================================

/// Extension command (user-triggered skill)
pub type ExtensionCommand = ExtensionSkill;

// =============================================================================
// Frontmatter Types
// =============================================================================

/// Skill/Command frontmatter
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(rename = "disable-model-invocation", default)]
    pub disable_model_invocation: bool,

    /// V2: Prompt injection scope
    #[serde(default)]
    pub scope: Option<PromptScope>,

    /// V2: Bound tool name (for Tool scope)
    #[serde(rename = "bound-tool", default)]
    pub bound_tool: Option<String>,
}
