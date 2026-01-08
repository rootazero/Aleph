//! Unified Skills Data Model (refactor-skills-ui-architecture)
//!
//! This module provides a unified data model that covers all capability extensions:
//! - BuiltinMcp: Builtin MCP services (Rust native implementation)
//! - ExternalMcp: External MCP servers (user installed, subprocess)
//! - PromptTemplate: Prompt template skills (SKILL.md based)
//!
//! # Design Principle: Config as Code, UI as Convenience
//!
//! The underlying configuration is stored in TOML format.
//! The UI is just a friendly renderer of this configuration.

use crate::mcp::{McpEnvVar, McpServerConfig, McpServerPermissions, McpServerStatus, McpServerType};
use crate::skills::SkillInfo;

/// Unified skill type - covers all capability extensions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnifiedSkillType {
    /// Builtin MCP service (Rust native implementation)
    BuiltinMcp,
    /// External MCP server (user installed, subprocess)
    ExternalMcp,
    /// Prompt template skill (SKILL.md based)
    PromptTemplate,
}

impl From<McpServerType> for UnifiedSkillType {
    fn from(mcp_type: McpServerType) -> Self {
        match mcp_type {
            McpServerType::Builtin => UnifiedSkillType::BuiltinMcp,
            McpServerType::External => UnifiedSkillType::ExternalMcp,
        }
    }
}

/// Unified skill status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnifiedSkillStatus {
    /// Skill is stopped
    Stopped,
    /// Skill is starting
    Starting,
    /// Skill is running
    Running,
    /// Skill has an error
    Error,
}

impl From<McpServerStatus> for UnifiedSkillStatus {
    fn from(status: McpServerStatus) -> Self {
        match status {
            McpServerStatus::Stopped => UnifiedSkillStatus::Stopped,
            McpServerStatus::Starting => UnifiedSkillStatus::Starting,
            McpServerStatus::Running => UnifiedSkillStatus::Running,
            McpServerStatus::Error => UnifiedSkillStatus::Error,
        }
    }
}

/// MCP transport protocol type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    /// Standard input/output (subprocess)
    Stdio,
    /// Server-Sent Events (remote server)
    Sse,
}

impl Default for McpTransport {
    fn default() -> Self {
        McpTransport::Stdio
    }
}

/// Environment variable key-value pair (unified version)
#[derive(Debug, Clone)]
pub struct UnifiedEnvVar {
    pub key: String,
    pub value: String,
}

impl From<McpEnvVar> for UnifiedEnvVar {
    fn from(env: McpEnvVar) -> Self {
        Self {
            key: env.key,
            value: env.value,
        }
    }
}

impl From<UnifiedEnvVar> for McpEnvVar {
    fn from(env: UnifiedEnvVar) -> Self {
        Self {
            key: env.key,
            value: env.value,
        }
    }
}

/// Unified skill permissions
#[derive(Debug, Clone, Default)]
pub struct UnifiedSkillPermissions {
    /// Tool calls require user confirmation
    pub requires_confirmation: bool,
    /// Allowed file paths
    pub allowed_paths: Vec<String>,
    /// Allowed shell commands
    pub allowed_commands: Vec<String>,
}

impl From<McpServerPermissions> for UnifiedSkillPermissions {
    fn from(perms: McpServerPermissions) -> Self {
        Self {
            requires_confirmation: perms.requires_confirmation,
            allowed_paths: perms.allowed_paths,
            allowed_commands: perms.allowed_commands,
        }
    }
}

impl From<UnifiedSkillPermissions> for McpServerPermissions {
    fn from(perms: UnifiedSkillPermissions) -> Self {
        Self {
            requires_confirmation: perms.requires_confirmation,
            allowed_paths: perms.allowed_paths,
            allowed_commands: perms.allowed_commands,
        }
    }
}

/// Unified skill configuration (covers MCP servers and prompt templates)
#[derive(Debug, Clone)]
pub struct UnifiedSkillConfig {
    // Basic information
    /// Unique identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Description
    pub description: String,
    /// Type of skill
    pub skill_type: UnifiedSkillType,
    /// Whether enabled
    pub enabled: bool,
    /// SF Symbol icon name
    pub icon: String,
    /// Theme color (hex)
    pub color: String,

    // Optional trigger command (e.g., /git, /fs)
    pub trigger_command: Option<String>,

    // MCP-specific fields (skill_type == BuiltinMcp || ExternalMcp)
    /// Transport protocol
    pub transport: Option<McpTransport>,
    /// Command path (external MCP)
    pub command: Option<String>,
    /// Command arguments
    pub args: Vec<String>,
    /// Environment variables
    pub env: Vec<UnifiedEnvVar>,
    /// Working directory
    pub working_directory: Option<String>,

    // Permissions (all types)
    pub permissions: UnifiedSkillPermissions,

    // PromptTemplate-specific fields (skill_type == PromptTemplate)
    /// Path to SKILL.md file
    pub skill_md_path: Option<String>,
    /// Allowed MCP tools
    pub allowed_tools: Vec<String>,
}

impl UnifiedSkillConfig {
    /// Create a new unified skill config from MCP server config
    pub fn from_mcp_server(server: McpServerConfig) -> Self {
        Self {
            id: server.id,
            name: server.name,
            description: String::new(), // MCP servers don't have descriptions in current model
            skill_type: server.server_type.into(),
            enabled: server.enabled,
            icon: server.icon,
            color: server.color,
            trigger_command: server.trigger_command,
            transport: Some(McpTransport::Stdio), // Default to Stdio
            command: server.command,
            args: server.args,
            env: server.env.into_iter().map(Into::into).collect(),
            working_directory: server.working_directory,
            permissions: server.permissions.into(),
            skill_md_path: None,
            allowed_tools: Vec::new(),
        }
    }

    /// Create a new unified skill config from prompt template skill info
    pub fn from_skill_info(skill: SkillInfo, skills_dir: &str) -> Self {
        Self {
            id: format!("skill:{}", skill.id),
            name: skill.name,
            description: skill.description,
            skill_type: UnifiedSkillType::PromptTemplate,
            enabled: true, // Prompt templates are always "enabled"
            icon: "text.book.closed".to_string(),
            color: "#8E8E93".to_string(), // System gray
            trigger_command: Some(format!("/{}", skill.id)),
            transport: None,
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            working_directory: None,
            permissions: UnifiedSkillPermissions::default(),
            skill_md_path: Some(format!("{}/{}/SKILL.md", skills_dir, skill.id)),
            allowed_tools: skill.allowed_tools,
        }
    }

    /// Convert to MCP server config (for MCP-type skills)
    pub fn to_mcp_server(&self) -> Option<McpServerConfig> {
        match self.skill_type {
            UnifiedSkillType::BuiltinMcp | UnifiedSkillType::ExternalMcp => {
                Some(McpServerConfig {
                    id: self.id.clone(),
                    name: self.name.clone(),
                    server_type: match self.skill_type {
                        UnifiedSkillType::BuiltinMcp => McpServerType::Builtin,
                        UnifiedSkillType::ExternalMcp => McpServerType::External,
                        _ => McpServerType::External,
                    },
                    enabled: self.enabled,
                    command: self.command.clone(),
                    args: self.args.clone(),
                    env: self.env.iter().cloned().map(Into::into).collect(),
                    working_directory: self.working_directory.clone(),
                    trigger_command: self.trigger_command.clone(),
                    permissions: self.permissions.clone().into(),
                    icon: self.icon.clone(),
                    color: self.color.clone(),
                })
            }
            UnifiedSkillType::PromptTemplate => None,
        }
    }

    /// Check if this is an MCP skill (builtin or external)
    pub fn is_mcp(&self) -> bool {
        matches!(
            self.skill_type,
            UnifiedSkillType::BuiltinMcp | UnifiedSkillType::ExternalMcp
        )
    }

    /// Check if this is a prompt template skill
    pub fn is_prompt_template(&self) -> bool {
        matches!(self.skill_type, UnifiedSkillType::PromptTemplate)
    }
}

/// Unified skill status info (for UI display)
#[derive(Debug, Clone)]
pub struct UnifiedSkillStatusInfo {
    /// Current status
    pub status: UnifiedSkillStatus,
    /// Status message
    pub message: Option<String>,
    /// Last error message (if any)
    pub last_error: Option<String>,
    /// Process ID (external MCP only)
    pub pid: Option<u64>,
}

impl Default for UnifiedSkillStatusInfo {
    fn default() -> Self {
        Self {
            status: UnifiedSkillStatus::Stopped,
            message: None,
            last_error: None,
            pid: None,
        }
    }
}

impl UnifiedSkillStatusInfo {
    /// Create a running status
    pub fn running() -> Self {
        Self {
            status: UnifiedSkillStatus::Running,
            message: Some("Running".to_string()),
            last_error: None,
            pid: None,
        }
    }

    /// Create a stopped status
    pub fn stopped() -> Self {
        Self {
            status: UnifiedSkillStatus::Stopped,
            message: Some("Stopped".to_string()),
            last_error: None,
            pid: None,
        }
    }

    /// Create an error status
    pub fn error(message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            status: UnifiedSkillStatus::Error,
            message: Some("Error".to_string()),
            last_error: Some(msg),
            pid: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_skill_from_mcp_server() {
        let mcp_server = McpServerConfig {
            id: "fs".to_string(),
            name: "File System".to_string(),
            server_type: McpServerType::Builtin,
            enabled: true,
            command: None,
            args: vec![],
            env: vec![],
            working_directory: None,
            trigger_command: Some("/fs".to_string()),
            permissions: McpServerPermissions::default(),
            icon: "folder".to_string(),
            color: "#007AFF".to_string(),
        };

        let unified = UnifiedSkillConfig::from_mcp_server(mcp_server);

        assert_eq!(unified.id, "fs");
        assert_eq!(unified.name, "File System");
        assert_eq!(unified.skill_type, UnifiedSkillType::BuiltinMcp);
        assert!(unified.enabled);
        assert!(unified.is_mcp());
        assert!(!unified.is_prompt_template());
    }

    #[test]
    fn test_unified_skill_from_skill_info() {
        let skill_info = SkillInfo {
            id: "refine-text".to_string(),
            name: "Refine Text".to_string(),
            description: "Improve and polish writing".to_string(),
            allowed_tools: vec!["Read".to_string(), "Edit".to_string()],
        };

        let unified = UnifiedSkillConfig::from_skill_info(skill_info, "~/.config/aether/skills");

        assert_eq!(unified.id, "skill:refine-text");
        assert_eq!(unified.name, "Refine Text");
        assert_eq!(unified.skill_type, UnifiedSkillType::PromptTemplate);
        assert!(unified.is_prompt_template());
        assert!(!unified.is_mcp());
        assert_eq!(unified.allowed_tools.len(), 2);
    }

    #[test]
    fn test_unified_skill_to_mcp_server() {
        let unified = UnifiedSkillConfig {
            id: "git".to_string(),
            name: "Git".to_string(),
            description: "Git operations".to_string(),
            skill_type: UnifiedSkillType::ExternalMcp,
            enabled: true,
            icon: "arrow.triangle.branch".to_string(),
            color: "#F05033".to_string(),
            trigger_command: Some("/git".to_string()),
            transport: Some(McpTransport::Stdio),
            command: Some("~/.cargo/bin/mcp-git".to_string()),
            args: vec!["--path".to_string(), "/Users/test".to_string()],
            env: vec![],
            working_directory: None,
            permissions: UnifiedSkillPermissions::default(),
            skill_md_path: None,
            allowed_tools: vec![],
        };

        let mcp_server = unified.to_mcp_server().unwrap();

        assert_eq!(mcp_server.id, "git");
        assert_eq!(mcp_server.server_type, McpServerType::External);
        assert_eq!(mcp_server.command, Some("~/.cargo/bin/mcp-git".to_string()));
    }

    #[test]
    fn test_prompt_template_no_mcp_conversion() {
        let unified = UnifiedSkillConfig {
            id: "skill:refine-text".to_string(),
            name: "Refine Text".to_string(),
            description: "Improve writing".to_string(),
            skill_type: UnifiedSkillType::PromptTemplate,
            enabled: true,
            icon: "text.book.closed".to_string(),
            color: "#8E8E93".to_string(),
            trigger_command: Some("/refine-text".to_string()),
            transport: None,
            command: None,
            args: vec![],
            env: vec![],
            working_directory: None,
            permissions: UnifiedSkillPermissions::default(),
            skill_md_path: Some("~/.config/aether/skills/refine-text/SKILL.md".to_string()),
            allowed_tools: vec!["Read".to_string()],
        };

        assert!(unified.to_mcp_server().is_none());
    }
}
