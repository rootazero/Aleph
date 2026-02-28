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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SkillType {
    /// Command (from commands/ directory) - user-triggered via /command
    Command,
    /// Skill (from skills/ directory) - can be auto-invoked by LLM
    Skill,
}

/// Extension skill definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionSkill {
    /// Skill name (from directory name or frontmatter)
    pub name: String,

    /// Plugin name (if from a plugin)
    pub plugin_name: Option<String>,

    /// Skill type
    pub skill_type: SkillType,

    /// Description (from frontmatter)
    pub description: String,

    /// Skill content (markdown body after frontmatter)
    pub content: String,

    /// Whether to disable automatic model invocation
    pub disable_model_invocation: bool,

    /// V2: Prompt injection scope
    #[serde(default)]
    pub scope: PromptScope,

    /// V2: Bound tool name (for Tool scope)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_tool: Option<String>,

    /// Source path
    pub source_path: PathBuf,

    /// Discovery source
    pub source: DiscoverySource,
}

impl ExtensionSkill {
    /// Get the fully qualified name (plugin:skill or just skill)
    pub fn qualified_name(&self) -> String {
        match &self.plugin_name {
            Some(plugin) => format!("{}:{}", plugin, self.name),
            None => self.name.clone(),
        }
    }

    /// Check if this skill can be auto-invoked by the model
    pub fn is_auto_invocable(&self) -> bool {
        !self.disable_model_invocation && self.skill_type == SkillType::Skill
    }

    /// Substitute $ARGUMENTS placeholder
    pub fn with_arguments(&self, arguments: &str) -> String {
        self.content.replace("$ARGUMENTS", arguments)
    }

    /// Convert to SkillInfo for compatibility with ToolRegistry
    ///
    /// This allows ExtensionSkill to be registered with the existing
    /// tool registration system.
    pub fn to_skill_info(&self) -> crate::skills::SkillInfo {
        crate::skills::SkillInfo {
            id: self.qualified_name(),
            name: self.name.clone(),
            description: self.description.clone(),
            triggers: Vec::new(), // ExtensionSkill doesn't track triggers
            allowed_tools: Vec::new(), // ExtensionSkill doesn't track allowed tools
            ecosystem: "aleph".to_string(),
        }
    }

    /// Get the base directory for this skill (for file references)
    pub fn base_dir(&self) -> PathBuf {
        self.source_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

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
