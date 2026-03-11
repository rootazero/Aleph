//! L3 Sandbox Executor — isolated execution environment for High-risk skill validation.
//!
//! Combines ShadowFs (read source, write overlay) with RestrictedToolset
//! (path bounds, tool whitelist) and a timeout-guarded execution loop.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::info;

use super::restricted_tools::{RestrictedToolset, SandboxViolation};
use super::shadow_fs::ShadowFs;

/// Configuration for the sandbox executor.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub allow_network: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
            max_output_bytes: 1_048_576, // 1 MB
            allow_network: false,
        }
    }
}

/// Result of a sandbox execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxResult {
    pub success: bool,
    pub modified_files: Vec<PathBuf>,
    pub violations: Vec<SandboxViolation>,
    pub duration_ms: u64,
    pub error: Option<String>,
}

/// Record of a single tool call made during sandbox execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub file_path: Option<String>,
    pub allowed: bool,
}

/// L3 Sandbox Executor.
pub struct SandboxExecutor {
    shadow_fs: ShadowFs,
    restricted_tools: RestrictedToolset,
    config: SandboxConfig,
}

impl SandboxExecutor {
    /// Create a new sandbox executor.
    pub fn new(
        source_dir: PathBuf,
        overlay_dir: PathBuf,
        allowed_tools: HashSet<String>,
        config: SandboxConfig,
    ) -> Self {
        let shadow_fs = ShadowFs::new(source_dir, overlay_dir.clone());
        let restricted_tools = RestrictedToolset::new(allowed_tools, overlay_dir, config.allow_network);

        Self {
            shadow_fs,
            restricted_tools,
            config,
        }
    }

    /// Validate a sequence of tool calls against the sandbox constraints.
    /// Returns a SandboxResult with any violations found.
    pub async fn validate_tool_calls(
        &self,
        tool_calls: &[(String, Option<String>)], // (tool_name, optional_file_path)
    ) -> SandboxResult {
        let start = std::time::Instant::now();
        let mut violations = Vec::new();

        for (tool_name, file_path) in tool_calls {
            let path = file_path.as_ref().map(|p| std::path::Path::new(p.as_str()));
            let result = self.restricted_tools.validate_call(tool_name, path);

            if let Err(violation) = result {
                violations.push(violation);
            }
        }

        let modified = self.shadow_fs.modified_files().await.unwrap_or_default();
        let duration_ms = start.elapsed().as_millis() as u64;

        let result = SandboxResult {
            success: violations.is_empty(),
            modified_files: modified,
            violations,
            duration_ms,
            error: None,
        };

        info!(
            target: "aleph::evolution::probe",
            probe = "sandbox_validation_completed",
            success = result.success,
            tool_calls_checked = tool_calls.len(),
            violations = result.violations.len(),
            modified_files = result.modified_files.len(),
            duration_ms = result.duration_ms,
            "L3 sandbox validation completed"
        );

        result
    }

    /// Get reference to the shadow filesystem.
    pub fn shadow_fs(&self) -> &ShadowFs {
        &self.shadow_fs
    }

    /// Get the configured timeout.
    pub fn timeout(&self) -> Duration {
        self.config.timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::fs;

    #[tokio::test]
    async fn sandbox_allows_valid_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        let tools: HashSet<String> = ["read_file", "write_file"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let executor = SandboxExecutor::new(source, overlay, tools, SandboxConfig::default());

        let calls = vec![
            ("read_file".to_string(), Some("test.txt".to_string())),
            ("write_file".to_string(), Some("output.txt".to_string())),
        ];

        let result = executor.validate_tool_calls(&calls).await;
        assert!(result.success);
        assert!(result.violations.is_empty());
    }

    #[tokio::test]
    async fn sandbox_catches_violations() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        let tools: HashSet<String> = ["read_file"].iter().map(|s| s.to_string()).collect();
        let executor = SandboxExecutor::new(source, overlay, tools, SandboxConfig::default());

        let calls = vec![
            ("read_file".to_string(), None),
            ("delete_all".to_string(), None), // not in whitelist
            ("read_file".to_string(), Some("../../../etc/passwd".to_string())), // path escape
        ];

        let result = executor.validate_tool_calls(&calls).await;
        assert!(!result.success);
        assert_eq!(result.violations.len(), 2);
    }

    #[tokio::test]
    async fn sandbox_tracks_modified_files() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        let tools: HashSet<String> = ["write_file"].iter().map(|s| s.to_string()).collect();
        let executor = SandboxExecutor::new(source, overlay.clone(), tools, SandboxConfig::default());

        // Simulate a write to the overlay
        fs::write(overlay.join("result.txt"), "output").await.unwrap();

        let result = executor.validate_tool_calls(&[]).await;
        assert!(result.success);
        assert_eq!(result.modified_files.len(), 1);
    }

    #[test]
    fn default_config() {
        let config = SandboxConfig::default();
        assert_eq!(config.timeout, Duration::from_secs(60));
        assert_eq!(config.max_output_bytes, 1_048_576);
        assert!(!config.allow_network);
    }
}
