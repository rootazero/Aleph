//! Capability definitions for the Three-Layer Control architecture
//!
//! Capabilities follow the principle of least privilege - each Skill declares
//! what capabilities it needs, and the ToolRouter enforces these restrictions.

use std::fmt;

/// Capability that a Skill can request
///
/// Each capability represents a specific type of operation that may be
/// restricted based on security policies.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Capability {
    // ===== File System =====
    /// Read files (safe by default)
    FileRead,
    /// List directories (safe by default)
    FileList,
    /// Write files (requires confirmation)
    FileWrite,
    /// Delete files (dangerous, blocked by default)
    FileDelete,

    // ===== Network =====
    /// Web search (safe by default)
    WebSearch,
    /// Fetch URL content (safe by default)
    WebFetch,

    // ===== MCP =====
    /// Access specific MCP server
    Mcp { server: String },

    // ===== LLM =====
    /// Call LLM (safe by default, but has token cost)
    LlmCall,

    // ===== System =====
    /// Execute shell commands (dangerous, blocked by default)
    ShellExec,
    /// Spawn processes (dangerous, blocked by default)
    ProcessSpawn,
}

/// Security level for a capability
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityLevel {
    /// No confirmation needed
    Safe,
    /// Requires user confirmation before execution
    Confirmation,
    /// Blocked by default, requires explicit override
    Blocked,
}

impl Capability {
    /// Get the default security level for this capability
    pub fn default_level(&self) -> CapabilityLevel {
        match self {
            // Safe operations
            Capability::FileRead => CapabilityLevel::Safe,
            Capability::FileList => CapabilityLevel::Safe,
            Capability::WebSearch => CapabilityLevel::Safe,
            Capability::WebFetch => CapabilityLevel::Safe,
            Capability::LlmCall => CapabilityLevel::Safe,
            Capability::Mcp { .. } => CapabilityLevel::Safe,

            // Requires confirmation
            Capability::FileWrite => CapabilityLevel::Confirmation,

            // Blocked by default
            Capability::FileDelete => CapabilityLevel::Blocked,
            Capability::ShellExec => CapabilityLevel::Blocked,
            Capability::ProcessSpawn => CapabilityLevel::Blocked,
        }
    }

    /// Check if this capability is dangerous (Confirmation or Blocked)
    pub fn is_dangerous(&self) -> bool {
        !matches!(self.default_level(), CapabilityLevel::Safe)
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Capability::FileRead => write!(f, "file:read"),
            Capability::FileList => write!(f, "file:list"),
            Capability::FileWrite => write!(f, "file:write"),
            Capability::FileDelete => write!(f, "file:delete"),
            Capability::WebSearch => write!(f, "web:search"),
            Capability::WebFetch => write!(f, "web:fetch"),
            Capability::Mcp { server } => write!(f, "mcp:{}", server),
            Capability::LlmCall => write!(f, "llm:call"),
            Capability::ShellExec => write!(f, "shell:exec"),
            Capability::ProcessSpawn => write!(f, "process:spawn"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_equality() {
        assert_eq!(Capability::FileRead, Capability::FileRead);
        assert_ne!(Capability::FileRead, Capability::FileWrite);
    }

    #[test]
    fn test_capability_mcp_with_server() {
        let cap1 = Capability::Mcp { server: "github".to_string() };
        let cap2 = Capability::Mcp { server: "github".to_string() };
        let cap3 = Capability::Mcp { server: "slack".to_string() };

        assert_eq!(cap1, cap2);
        assert_ne!(cap1, cap3);
    }

    #[test]
    fn test_capability_level_default() {
        assert_eq!(Capability::FileRead.default_level(), CapabilityLevel::Safe);
        assert_eq!(Capability::FileWrite.default_level(), CapabilityLevel::Confirmation);
        assert_eq!(Capability::ShellExec.default_level(), CapabilityLevel::Blocked);
    }

    #[test]
    fn test_capability_display() {
        assert_eq!(format!("{}", Capability::FileRead), "file:read");
        assert_eq!(format!("{}", Capability::Mcp { server: "github".to_string() }), "mcp:github");
    }
}
