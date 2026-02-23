//! Skill Loader
//!
//! Batch loading of Markdown skills from directory.

use std::path::{Path, PathBuf};
use anyhow::Result;
use tokio::fs;
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

use super::parser::parse_skill_file;
use super::spec::{AlephSkillSpec, SandboxMode};
use super::tool_adapter::MarkdownCliTool;

/// Skill loader for scanning and loading Markdown skills
pub struct SkillLoader {
    /// Base directory to scan (e.g., "skills/")
    base_dir: PathBuf,

    /// Whether to scan recursively
    recursive: bool,
}

impl SkillLoader {
    /// Create a new skill loader
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            recursive: true,
        }
    }

    /// Load all skills from the base directory
    ///
    /// Returns (loaded_tools, errors) - partial failures are logged but don't abort
    pub async fn load_all(&self) -> (Vec<MarkdownCliTool>, Vec<(PathBuf, anyhow::Error)>) {
        let mut tools = Vec::new();
        let mut errors = Vec::new();

        info!(
            base_dir = %self.base_dir.display(),
            recursive = self.recursive,
            "Scanning for Markdown skills"
        );

        // Find all .md files
        let skill_files = match self.find_skill_files().await {
            Ok(files) => files,
            Err(e) => {
                error!(error = %e, "Failed to scan skill directory");
                return (tools, errors);
            }
        };

        info!(count = skill_files.len(), "Found skill files");

        // Load each file
        for path in skill_files {
            match self.load_skill_file(&path).await {
                Ok(tool) => {
                    info!(
                        skill = %tool.spec.name,
                        path = %path.display(),
                        "Loaded skill"
                    );
                    tools.push(tool);
                }
                Err(e) => {
                    warn!(
                        path = %path.display(),
                        error = %e,
                        "Failed to load skill file"
                    );
                    errors.push((path, e));
                }
            }
        }

        info!(
            loaded = tools.len(),
            failed = errors.len(),
            "Skill loading complete"
        );

        (tools, errors)
    }

    /// Find all SKILL.md files (using walkdir for safety)
    async fn find_skill_files(&self) -> Result<Vec<PathBuf>> {
        if !self.base_dir.exists() {
            info!(
                base_dir = %self.base_dir.display(),
                "Skill directory does not exist, skipping"
            );
            return Ok(Vec::new());
        }

        // Use walkdir (sync) in blocking task to avoid stack issues
        let base_dir = self.base_dir.clone();
        let recursive = self.recursive;

        let skill_files = tokio::task::spawn_blocking(move || {
            let walker = if recursive {
                WalkDir::new(&base_dir)
                    .follow_links(false) // Prevent symlink loops
                    .max_depth(10) // Reasonable limit
            } else {
                WalkDir::new(&base_dir).max_depth(1)
            };

            walker
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
                .map(|e| e.into_path())
                .filter(|p| Self::is_skill_file_static(p))
                .collect::<Vec<PathBuf>>()
        })
        .await?;

        Ok(skill_files)
    }

    /// Static version for use in spawn_blocking
    fn is_skill_file_static(path: &Path) -> bool {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            name.eq_ignore_ascii_case("SKILL.md") || name.to_lowercase().ends_with(".skill.md")
        } else {
            false
        }
    }

    /// Load a single skill file
    async fn load_skill_file(&self, path: &Path) -> Result<MarkdownCliTool> {
        // Read file
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read file: {}", e))?;

        // Parse spec
        let spec =
            parse_skill_file(&content).map_err(|e| anyhow::anyhow!("Parse error: {}", e))?;

        // Validate binary availability (optional warning)
        self.check_binary_availability(&spec);

        // Create tool
        Ok(MarkdownCliTool::new(spec))
    }

    /// Check if required binaries are available (only for host mode)
    fn check_binary_availability(&self, spec: &AlephSkillSpec) {
        // Only check when running on host
        let is_host_mode = spec
            .metadata
            .aleph
            .as_ref()
            .map(|a| matches!(a.security.sandbox, SandboxMode::Host))
            .unwrap_or(true); // Default: OpenClaw style (host execution)

        if !is_host_mode {
            // Docker/VirtualFs mode: binary is in container, not on host
            debug!(
                skill = %spec.name,
                "Skipping host binary check (sandbox mode)"
            );
            return;
        }

        for bin in &spec.metadata.requires.bins {
            match which::which(bin) {
                Ok(path) => {
                    debug!(
                        skill = %spec.name,
                        binary = %bin,
                        path = %path.display(),
                        "Binary found"
                    );
                }
                Err(_) => {
                    warn!(
                        skill = %spec.name,
                        binary = %bin,
                        "Required binary not found in PATH (skill will fail at runtime). \
                        Install it or switch to 'sandbox: docker' mode."
                    );
                }
            }
        }
    }
}

/// Helper function for convenient loading
pub async fn load_skills_from_dir(dir: impl Into<PathBuf>) -> Vec<MarkdownCliTool> {
    let loader = SkillLoader::new(dir);
    let (tools, errors) = loader.load_all().await;

    if !errors.is_empty() {
        warn!(
            failed_count = errors.len(),
            "Some skills failed to load (check logs for details)"
        );
    }

    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn test_load_valid_skill() {
        let temp_dir = tempfile::tempdir().unwrap();
        let skill_path = temp_dir.path().join("test-skill/SKILL.md");

        std::fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
        let mut file = std::fs::File::create(&skill_path).unwrap();

        writeln!(file, "---").unwrap();
        writeln!(file, "name: test-tool").unwrap();
        writeln!(file, "description: A test").unwrap();
        writeln!(file, "metadata:").unwrap();
        writeln!(file, "  requires:").unwrap();
        writeln!(file, "    bins: [\"echo\"]").unwrap();
        writeln!(file, "---").unwrap();
        writeln!(file, "Test content").unwrap();

        let loader = SkillLoader::new(temp_dir.path());
        let (tools, errors) = loader.load_all().await;

        assert_eq!(tools.len(), 1);
        assert_eq!(errors.len(), 0);
        assert_eq!(tools[0].spec.name, "test-tool");
    }

    #[tokio::test]
    async fn test_partial_failure_resilience() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Valid skill
        let valid = temp_dir.path().join("valid/SKILL.md");
        std::fs::create_dir_all(valid.parent().unwrap()).unwrap();
        std::fs::write(
            &valid,
            "---\nname: good\ndescription: ok\nmetadata:\n  requires:\n    bins: []\n---\nContent",
        )
        .unwrap();

        // Invalid skill (malformed YAML)
        let invalid = temp_dir.path().join("invalid/SKILL.md");
        std::fs::create_dir_all(invalid.parent().unwrap()).unwrap();
        std::fs::write(&invalid, "---\n{{{invalid yaml\n---\n").unwrap();

        let loader = SkillLoader::new(temp_dir.path());
        let (tools, errors) = loader.load_all().await;

        assert_eq!(tools.len(), 1); // Valid one loaded
        assert_eq!(errors.len(), 1); // Invalid one failed
    }
}
