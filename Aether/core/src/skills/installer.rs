//! Skills Installer - downloads and installs skills from GitHub/ZIP.
//!
//! Supports multiple installation methods:
//! - Official skills from anthropics/skills repository
//! - Third-party skills from any GitHub repository
//! - Local ZIP file upload

use crate::error::{AetherError, Result};
use crate::skills::Skill;
use std::io::Read;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Skills installer for downloading and managing skills
pub struct SkillsInstaller {
    /// Skills directory path
    skills_dir: PathBuf,
}

impl SkillsInstaller {
    /// Create a new skills installer
    ///
    /// # Arguments
    ///
    /// * `skills_dir` - Path to the skills directory
    pub fn new(skills_dir: PathBuf) -> Self {
        Self { skills_dir }
    }

    /// Install official skills from anthropics/skills repository
    ///
    /// Downloads the official skills repository ZIP and extracts valid skills.
    pub async fn install_official_skills(&self) -> Result<Vec<String>> {
        info!("Installing official skills from anthropics/skills");
        let url = "https://github.com/anthropics/skills/archive/refs/heads/main.zip";
        self.install_from_github_zip(url).await
    }

    /// Install skill from GitHub repository URL
    ///
    /// Supports formats:
    /// - `https://github.com/user/repo`
    /// - `github.com/user/repo`
    /// - `user/repo` (assumes GitHub)
    pub async fn install_from_github(&self, url: &str) -> Result<Vec<String>> {
        let normalized = self.normalize_github_url(url)?;
        info!(url = %normalized, "Installing skills from GitHub");

        let zip_url = format!("{}/archive/refs/heads/main.zip", normalized);
        self.install_from_github_zip(&zip_url).await
    }

    /// Install from ZIP file (local path)
    ///
    /// Extracts SKILL.md files from the ZIP and installs valid skills.
    pub async fn install_from_zip(&self, zip_path: &PathBuf) -> Result<Vec<String>> {
        info!(path = %zip_path.display(), "Installing skills from ZIP file");

        let file = std::fs::File::open(zip_path).map_err(|e| {
            AetherError::config(format!(
                "Failed to open ZIP file {}: {}",
                zip_path.display(),
                e
            ))
        })?;

        self.extract_and_install_from_zip(file)
    }

    /// Delete a skill
    ///
    /// Removes the skill directory entirely.
    pub fn delete_skill(&self, id: &str) -> Result<()> {
        let skill_dir = self.skills_dir.join(id);

        if !skill_dir.exists() {
            return Err(AetherError::invalid_config(format!("Skill '{}' not found", id)));
        }

        info!(skill_id = %id, "Deleting skill");
        std::fs::remove_dir_all(&skill_dir).map_err(|e| {
            AetherError::config(format!("Failed to delete skill '{}': {}", id, e))
        })?;

        Ok(())
    }

    /// Check if a skill name is valid
    ///
    /// Valid names: lowercase letters, numbers, hyphens
    pub fn is_valid_skill_name(&self, name: &str) -> bool {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit())
            && !name.starts_with('-')
            && !name.ends_with('-')
    }

    /// Normalize GitHub URL to standard format
    ///
    /// Converts various formats to `https://github.com/user/repo`
    fn normalize_github_url(&self, url: &str) -> Result<String> {
        let url = url.trim();

        // Handle short format: user/repo
        if !url.contains("://") && !url.starts_with("github.com") {
            if url.matches('/').count() == 1 && !url.contains(' ') {
                return Ok(format!("https://github.com/{}", url));
            }
        }

        // Handle github.com/user/repo
        if url.starts_with("github.com/") {
            return Ok(format!("https://{}", url));
        }

        // Already full URL
        if url.starts_with("https://github.com/") || url.starts_with("http://github.com/") {
            // Ensure https
            let normalized = url.replace("http://", "https://");
            // Remove trailing slashes and .git suffix
            let normalized = normalized.trim_end_matches('/');
            let normalized = normalized.trim_end_matches(".git");
            return Ok(normalized.to_string());
        }

        Err(AetherError::invalid_config(format!(
            "Invalid GitHub URL format: {}. Expected: user/repo, github.com/user/repo, or https://github.com/user/repo",
            url
        )))
    }

    /// Extract skill directory name from ZIP path
    ///
    /// Path format: `repo-main/skill-name/SKILL.md` -> `skill-name`
    fn extract_skill_dir_name(&self, path: &str) -> Option<String> {
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            let parent = parts[parts.len() - 2];
            // Skip if parent is the repo root or "skills" directory
            if !parent.contains("-main")
                && !parent.contains("-master")
                && parent != "skills"
                && !parent.is_empty()
            {
                return Some(parent.to_string());
            }
        }
        None
    }

    /// Download ZIP from URL and install skills
    async fn install_from_github_zip(&self, url: &str) -> Result<Vec<String>> {
        debug!(url = %url, "Downloading ZIP from GitHub");

        let response = reqwest::get(url).await.map_err(|e| {
            AetherError::network(format!("Failed to download from {}: {}", url, e))
        })?;

        if !response.status().is_success() {
            return Err(AetherError::network(format!(
                "Failed to download from {}: HTTP {}",
                url,
                response.status()
            )));
        }

        let bytes = response.bytes().await.map_err(|e| {
            AetherError::network(format!("Failed to read response body: {}", e))
        })?;

        // Save to temp file
        let temp_dir = std::env::temp_dir();
        let temp_zip = temp_dir.join(format!("aether-skill-{}.zip", uuid::Uuid::new_v4()));

        std::fs::write(&temp_zip, &bytes).map_err(|e| {
            AetherError::config(format!("Failed to write temp ZIP: {}", e))
        })?;

        // Extract and install
        let file = std::fs::File::open(&temp_zip).map_err(|e| {
            AetherError::config(format!("Failed to open temp ZIP: {}", e))
        })?;

        let result = self.extract_and_install_from_zip(file);

        // Cleanup temp file
        let _ = std::fs::remove_file(&temp_zip);

        result
    }

    /// Extract skills from ZIP archive and install them
    fn extract_and_install_from_zip<R: Read + std::io::Seek>(
        &self,
        reader: R,
    ) -> Result<Vec<String>> {
        let mut archive = zip::ZipArchive::new(reader).map_err(|e| {
            AetherError::config(format!("Failed to read ZIP archive: {}", e))
        })?;

        let mut installed = Vec::new();

        // First pass: find all SKILL.md files
        let mut skill_files: Vec<(String, String)> = Vec::new();

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| {
                AetherError::config(format!("Failed to read ZIP entry: {}", e))
            })?;

            let name = file.name().to_string();

            // Look for SKILL.md files
            if name.ends_with("SKILL.md") {
                let mut content = String::new();
                file.read_to_string(&mut content).map_err(|e| {
                    AetherError::config(format!("Failed to read SKILL.md content: {}", e))
                })?;

                skill_files.push((name, content));
            }
        }

        // Second pass: install valid skills
        for (path, content) in skill_files {
            let skill_dir_name = match self.extract_skill_dir_name(&path) {
                Some(name) => name,
                None => {
                    debug!(path = %path, "Could not extract skill name from path, skipping");
                    continue;
                }
            };

            // Validate SKILL.md format first (before touching filesystem)
            let target_dir = self.skills_dir.join(&skill_dir_name);
            match Skill::parse(&skill_dir_name, &content) {
                Ok(skill) => {
                    // Try to create skill directory atomically (TOCTOU fix)
                    // Using create_dir instead of exists() + create_dir_all
                    // to avoid race condition between check and creation
                    match std::fs::create_dir(&target_dir) {
                        Ok(()) => {
                            // Successfully created, continue with installation
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            // Skill already exists, skip it
                            info!(
                                skill = %skill_dir_name,
                                "Skill already exists, skipping"
                            );
                            continue;
                        }
                        Err(_) => {
                            // Parent directory might not exist, try create_dir_all
                            std::fs::create_dir_all(&target_dir).map_err(|e| {
                                AetherError::config(format!(
                                    "Failed to create skill directory {}: {}",
                                    target_dir.display(),
                                    e
                                ))
                            })?;
                        }
                    }

                    // Write SKILL.md
                    let skill_md_path = target_dir.join("SKILL.md");
                    std::fs::write(&skill_md_path, &content).map_err(|e| {
                        AetherError::config(format!(
                            "Failed to write SKILL.md: {}",
                            e
                        ))
                    })?;

                    info!(
                        skill_id = %skill_dir_name,
                        name = %skill.name(),
                        "Installed skill"
                    );
                    installed.push(skill_dir_name);
                }
                Err(e) => {
                    warn!(
                        path = %path,
                        error = %e,
                        "Invalid SKILL.md format, skipping"
                    );
                }
            }
        }

        info!(count = installed.len(), "Skills installation complete");
        Ok(installed)
    }

    /// Get the skills directory path
    pub fn skills_dir(&self) -> &PathBuf {
        &self.skills_dir
    }

    /// Install skill from GitHub URL (synchronous wrapper)
    ///
    /// Blocks the current thread to perform the async download.
    /// Returns the first successfully installed skill.
    pub fn install_from_url_sync(&self, url: &str) -> Result<Skill> {
        let runtime = tokio::runtime::Runtime::new().map_err(|e| {
            AetherError::config(format!("Failed to create async runtime: {}", e))
        })?;

        let installed = runtime.block_on(self.install_from_github(url))?;

        if installed.is_empty() {
            return Err(AetherError::invalid_config(format!(
                "No valid skills found at {}",
                url
            )));
        }

        // Load and return the first installed skill
        let skill_id = &installed[0];
        let skill_dir = self.skills_dir.join(skill_id);
        let skill_md = skill_dir.join("SKILL.md");
        let content = std::fs::read_to_string(&skill_md).map_err(|e| {
            AetherError::config(format!("Failed to read installed skill: {}", e))
        })?;

        Skill::parse(skill_id, &content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_normalize_github_url_short_format() {
        let installer = SkillsInstaller::new(PathBuf::from("/tmp"));

        assert_eq!(
            installer.normalize_github_url("user/repo").unwrap(),
            "https://github.com/user/repo"
        );
    }

    #[test]
    fn test_normalize_github_url_domain_format() {
        let installer = SkillsInstaller::new(PathBuf::from("/tmp"));

        assert_eq!(
            installer.normalize_github_url("github.com/user/repo").unwrap(),
            "https://github.com/user/repo"
        );
    }

    #[test]
    fn test_normalize_github_url_full_format() {
        let installer = SkillsInstaller::new(PathBuf::from("/tmp"));

        assert_eq!(
            installer
                .normalize_github_url("https://github.com/user/repo")
                .unwrap(),
            "https://github.com/user/repo"
        );

        // With trailing slash
        assert_eq!(
            installer
                .normalize_github_url("https://github.com/user/repo/")
                .unwrap(),
            "https://github.com/user/repo"
        );

        // With .git suffix
        assert_eq!(
            installer
                .normalize_github_url("https://github.com/user/repo.git")
                .unwrap(),
            "https://github.com/user/repo"
        );
    }

    #[test]
    fn test_normalize_github_url_invalid() {
        let installer = SkillsInstaller::new(PathBuf::from("/tmp"));

        assert!(installer.normalize_github_url("invalid").is_err());
        assert!(installer.normalize_github_url("not a url at all").is_err());
    }

    #[test]
    fn test_is_valid_skill_name() {
        let installer = SkillsInstaller::new(PathBuf::from("/tmp"));

        assert!(installer.is_valid_skill_name("refine-text"));
        assert!(installer.is_valid_skill_name("skill123"));
        assert!(installer.is_valid_skill_name("my-skill-2"));

        assert!(!installer.is_valid_skill_name("")); // Empty
        assert!(!installer.is_valid_skill_name("-invalid")); // Starts with hyphen
        assert!(!installer.is_valid_skill_name("invalid-")); // Ends with hyphen
        assert!(!installer.is_valid_skill_name("Invalid")); // Uppercase
        assert!(!installer.is_valid_skill_name("has space")); // Space
    }

    #[test]
    fn test_extract_skill_dir_name() {
        let installer = SkillsInstaller::new(PathBuf::from("/tmp"));

        // Standard path
        assert_eq!(
            installer.extract_skill_dir_name("skills-main/refine-text/SKILL.md"),
            Some("refine-text".to_string())
        );

        // Nested path
        assert_eq!(
            installer.extract_skill_dir_name("repo-main/skills/translate/SKILL.md"),
            Some("translate".to_string())
        );

        // Skip repo root
        assert_eq!(
            installer.extract_skill_dir_name("repo-main/SKILL.md"),
            None
        );
    }

    #[test]
    fn test_delete_skill() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        // Create a skill
        let skill_dir = skills_dir.join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: test\ndescription: test\n---\n").unwrap();

        let installer = SkillsInstaller::new(skills_dir);

        // Delete should succeed
        installer.delete_skill("test-skill").unwrap();
        assert!(!skill_dir.exists());

        // Delete non-existent should fail
        assert!(installer.delete_skill("nonexistent").is_err());
    }

    #[test]
    fn test_extract_and_install_from_zip() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        // Create a test ZIP in memory
        let mut zip_buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_buffer));

            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            // Add a valid SKILL.md
            let skill_content = r#"---
name: test-skill
description: A test skill
---

# Test Skill

Instructions here.
"#;

            zip.start_file("repo-main/test-skill/SKILL.md", options)
                .unwrap();
            zip.write_all(skill_content.as_bytes()).unwrap();

            zip.finish().unwrap();
        }

        let installer = SkillsInstaller::new(skills_dir.clone());

        let cursor = std::io::Cursor::new(zip_buffer);
        let installed = installer.extract_and_install_from_zip(cursor).unwrap();

        assert_eq!(installed, vec!["test-skill"]);
        assert!(skills_dir.join("test-skill/SKILL.md").exists());
    }
}
