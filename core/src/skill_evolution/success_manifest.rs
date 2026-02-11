//! Success Manifest: Semantic constraints (soft constraints) for Skills
//!
//! The SuccessManifest defines what a Skill should do, what it's allowed to do,
//! and what it's prohibited from doing. It serves as the "semantic contract"
//! that guides LLM behavior within a Skill context.
//!
//! This is the "soft constraint" layer that complements the "hard constraint"
//! layer (Capabilities + Sandbox).
//!
//! # Example
//!
//! ```rust
//! use alephcore::skill_evolution::success_manifest::SuccessManifest;
//!
//! // Create a new manifest for a file processing skill
//! let manifest = SuccessManifest::new(
//!     "file_processor",
//!     "Process CSV files and generate reports"
//! );
//!
//! // Check if network access is prohibited
//! assert!(manifest.prohibits_network());
//!
//! // Check if a path is allowed for reading
//! assert!(manifest.allows_read_from("/data/input.csv"));
//! ```
//!
//! # Architecture
//!
//! The SuccessManifest is part of the dual-layer constraint system:
//!
//! - **Soft Layer (SuccessManifest)**: Semantic, human-readable constraints
//! - **Hard Layer (Capabilities)**: Enforced by sandbox at runtime
//!
//! The ConstraintValidator ensures these layers are consistent.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Success Manifest: Semantic constraints for a Skill
///
/// This structure represents the human-readable, semantic constraints that
/// define what a Skill is allowed and prohibited from doing. It's designed
/// to be serialized to/from Markdown format (SUCCESS_MANIFEST.md).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SuccessManifest {
    /// Metadata about the skill
    pub metadata: SkillMetadata,

    /// Goal: What this skill is designed to accomplish
    pub goal: String,

    /// Allowed operations
    pub allowed_operations: AllowedOperations,

    /// Prohibited operations
    pub prohibited_operations: ProhibitedOperations,

    /// Recommended tool chain (ordered list of tools)
    pub recommended_tools: Vec<RecommendedTool>,

    /// Success criteria: How to determine if execution was successful
    pub success_criteria: Vec<String>,

    /// Failure handling: What to do when things go wrong
    pub failure_handling: Vec<String>,

    /// Security guarantees: Safety promises this skill makes
    pub security_guarantees: Vec<String>,
}

/// Metadata about a skill
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillMetadata {
    /// Unique skill identifier (e.g., "personal_finance_audit")
    pub skill_id: String,

    /// Semantic version (e.g., "1.0.0")
    pub version: String,

    /// Creation timestamp (Unix seconds, UTC)
    pub created_at: i64,

    /// Author (e.g., "llm-generated", "user-defined")
    pub author: String,
}

/// Allowed operations for a skill
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AllowedOperations {
    /// Filesystem operations
    pub filesystem: FileSystemOperations,

    /// Script execution permissions
    pub script_execution: ScriptExecution,

    /// Data processing capabilities
    pub data_processing: DataProcessing,
}

/// Filesystem operations
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileSystemOperations {
    /// Paths allowed for reading (glob patterns)
    pub read_paths: Vec<String>,

    /// Paths allowed for writing (glob patterns)
    pub write_paths: Vec<String>,

    /// Whether temporary workspace is allowed
    pub allow_temp_workspace: bool,
}

/// Script execution permissions
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScriptExecution {
    /// Allowed script languages (e.g., "python", "bash")
    pub languages: Vec<String>,

    /// Allowed libraries/packages
    pub libraries: Vec<String>,
}

/// Data processing capabilities
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DataProcessing {
    /// Allowed input formats (e.g., "pdf", "csv", "json")
    pub input_formats: Vec<String>,

    /// Allowed output formats (e.g., "xlsx", "json", "html")
    pub output_formats: Vec<String>,

    /// Allowed operations (e.g., "parse", "calculate", "generate")
    pub operations: Vec<String>,
}

/// Prohibited operations for a skill
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProhibitedOperations {
    /// Network restrictions
    pub network: NetworkRestrictions,

    /// Filesystem restrictions
    pub filesystem: FileSystemRestrictions,

    /// Process restrictions
    pub process: ProcessRestrictions,
}

/// Network restrictions
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetworkRestrictions {
    /// Whether all network access is prohibited
    pub prohibit_all: bool,

    /// Specific prohibited domains (if not prohibit_all)
    pub prohibited_domains: Vec<String>,

    /// Reason for network restrictions
    pub reason: String,
}

/// Filesystem restrictions
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileSystemRestrictions {
    /// Prohibited paths (glob patterns)
    pub prohibited_paths: Vec<String>,

    /// Whether modifying original files is prohibited
    pub prohibit_modify_originals: bool,

    /// Reason for filesystem restrictions
    pub reason: String,
}

/// Process restrictions
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessRestrictions {
    /// Whether fork/exec is prohibited
    pub prohibit_fork: bool,

    /// Prohibited system commands
    pub prohibited_commands: Vec<String>,

    /// Reason for process restrictions
    pub reason: String,
}

/// Recommended tool in the tool chain
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecommendedTool {
    /// Tool name
    pub name: String,

    /// Tool description
    pub description: String,

    /// Order in the tool chain (1-indexed)
    pub order: u32,
}

impl SuccessManifest {
    /// Create a new SuccessManifest with default values
    pub fn new(skill_id: impl Into<String>, goal: impl Into<String>) -> Self {
        Self {
            metadata: SkillMetadata {
                skill_id: skill_id.into(),
                version: "1.0.0".to_string(),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
                author: "llm-generated".to_string(),
            },
            goal: goal.into(),
            allowed_operations: AllowedOperations {
                filesystem: FileSystemOperations {
                    read_paths: vec![],
                    write_paths: vec![],
                    allow_temp_workspace: true,
                },
                script_execution: ScriptExecution {
                    languages: vec![],
                    libraries: vec![],
                },
                data_processing: DataProcessing {
                    input_formats: vec![],
                    output_formats: vec![],
                    operations: vec![],
                },
            },
            prohibited_operations: ProhibitedOperations {
                network: NetworkRestrictions {
                    prohibit_all: true,
                    prohibited_domains: vec![],
                    reason: "Default: prohibit all network access for security".to_string(),
                },
                filesystem: FileSystemRestrictions {
                    prohibited_paths: vec![
                        "/System/**".to_string(),
                        "/usr/**".to_string(),
                        "/etc/**".to_string(),
                    ],
                    prohibit_modify_originals: true,
                    reason: "Protect system files and original data".to_string(),
                },
                process: ProcessRestrictions {
                    prohibit_fork: true,
                    prohibited_commands: vec![],
                    reason: "Prevent spawning uncontrolled processes".to_string(),
                },
            },
            recommended_tools: vec![],
            success_criteria: vec![],
            failure_handling: vec![],
            security_guarantees: vec![],
        }
    }

    /// Check if network access is prohibited
    pub fn prohibits_network(&self) -> bool {
        self.prohibited_operations.network.prohibit_all
    }

    /// Get allowed read paths
    pub fn allowed_read_paths(&self) -> &[String] {
        &self.allowed_operations.filesystem.read_paths
    }

    /// Get allowed write paths
    pub fn allowed_write_paths(&self) -> &[String] {
        &self.allowed_operations.filesystem.write_paths
    }

    /// Check if writing to a specific path is allowed
    pub fn allows_write_to(&self, path: &PathBuf) -> bool {
        let path_str = path.to_string_lossy();
        self.allowed_operations
            .filesystem
            .write_paths
            .iter()
            .any(|pattern| {
                // Simple glob matching (can be enhanced with glob crate)
                if pattern.ends_with("/**") {
                    // Recursive match: /data/** matches /data/file.txt and /data/subdir/file.txt
                    let prefix = &pattern[..pattern.len() - 3];
                    path_str.starts_with(prefix) && (path_str.len() > prefix.len())
                } else if pattern.ends_with("/*") {
                    // Single-level match: /tmp/* matches /tmp/file.txt but not /tmp/subdir/file.txt
                    let prefix = &pattern[..pattern.len() - 1]; // Keep the trailing /
                    if let Some(rest) = path_str.strip_prefix(prefix) {
                        // Check that there's no additional '/' in the rest
                        !rest.is_empty() && !rest.contains('/')
                    } else {
                        false
                    }
                } else {
                    // Exact match
                    path_str.as_ref() == pattern.as_str()
                }
            })
    }

    /// Check if reading from a specific path is allowed
    pub fn allows_read_from(&self, path: &PathBuf) -> bool {
        let path_str = path.to_string_lossy();
        self.allowed_operations
            .filesystem
            .read_paths
            .iter()
            .any(|pattern| {
                if pattern.ends_with("/**") {
                    // Recursive match
                    let prefix = &pattern[..pattern.len() - 3];
                    path_str.starts_with(prefix) && (path_str.len() > prefix.len())
                } else if pattern.ends_with("/*") {
                    // Single-level match
                    let prefix = &pattern[..pattern.len() - 1]; // Keep the trailing /
                    if let Some(rest) = path_str.strip_prefix(prefix) {
                        !rest.is_empty() && !rest.contains('/')
                    } else {
                        false
                    }
                } else {
                    // Exact match
                    path_str.as_ref() == pattern.as_str()
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_success_manifest_creation() {
        let manifest = SuccessManifest::new(
            "test_skill",
            "Test skill for unit testing",
        );

        assert_eq!(manifest.metadata.skill_id, "test_skill");
        assert_eq!(manifest.goal, "Test skill for unit testing");
        assert_eq!(manifest.metadata.version, "1.0.0");
        assert!(manifest.prohibits_network());
    }

    #[test]
    fn test_allows_write_to() {
        let mut manifest = SuccessManifest::new("test", "test");
        manifest.allowed_operations.filesystem.write_paths = vec![
            "/data/output/**".to_string(),
            "/tmp/*".to_string(),
        ];

        assert!(manifest.allows_write_to(&PathBuf::from("/data/output/file.txt")));
        assert!(manifest.allows_write_to(&PathBuf::from("/data/output/subdir/file.txt")));
        assert!(manifest.allows_write_to(&PathBuf::from("/tmp/file.txt")));
        assert!(!manifest.allows_write_to(&PathBuf::from("/tmp/subdir/file.txt")));
        assert!(!manifest.allows_write_to(&PathBuf::from("/etc/passwd")));
    }

    #[test]
    fn test_allows_read_from() {
        let mut manifest = SuccessManifest::new("test", "test");
        manifest.allowed_operations.filesystem.read_paths = vec![
            "/data/input/**".to_string(),
        ];

        assert!(manifest.allows_read_from(&PathBuf::from("/data/input/file.pdf")));
        assert!(manifest.allows_read_from(&PathBuf::from("/data/input/subdir/file.csv")));
        assert!(!manifest.allows_read_from(&PathBuf::from("/etc/passwd")));
    }
}
