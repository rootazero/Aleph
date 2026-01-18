//! Skills Registry - manages loaded skills from the skills directory.
//!
//! The registry scans the skills directory for SKILL.md files, parses them,
//! and provides lookup functionality.

use crate::error::{AetherError, Result};
use crate::skills::Skill;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use tracing::{debug, info, warn};

/// Skills Registry manages loaded skills
pub struct SkillsRegistry {
    /// Skills directory path
    skills_dir: PathBuf,

    /// Loaded skills indexed by ID
    skills: RwLock<HashMap<String, Skill>>,
}

impl SkillsRegistry {
    /// Create a new skills registry
    ///
    /// # Arguments
    ///
    /// * `skills_dir` - Path to the skills directory
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dir,
            skills: RwLock::new(HashMap::new()),
        }
    }

    /// Load all skills from the skills directory
    ///
    /// Scans subdirectories for SKILL.md files and parses them.
    /// Invalid skills are logged and skipped.
    pub fn load_all(&self) -> Result<()> {
        let mut skills = self
            .skills
            .write()
            .map_err(|_| AetherError::config("Failed to acquire write lock on skills registry"))?;

        skills.clear();

        if !self.skills_dir.exists() {
            info!(path = %self.skills_dir.display(), "Skills directory does not exist");
            return Ok(());
        }

        let entries = std::fs::read_dir(&self.skills_dir).map_err(|e| {
            AetherError::config(format!(
                "Failed to read skills directory {}: {}",
                self.skills_dir.display(),
                e
            ))
        })?;

        for entry in entries.flatten() {
            let path = entry.path();

            // Only process directories
            if !path.is_dir() {
                continue;
            }

            let skill_id = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();

            // Skip hidden directories
            if skill_id.starts_with('.') {
                continue;
            }

            let skill_md_path = path.join("SKILL.md");

            if !skill_md_path.exists() {
                debug!(skill_id = %skill_id, "No SKILL.md found, skipping");
                continue;
            }

            match self.load_skill(&skill_id, &skill_md_path) {
                Ok(skill) => {
                    info!(
                        skill_id = %skill_id,
                        name = %skill.frontmatter.name,
                        "Loaded skill"
                    );
                    skills.insert(skill_id, skill);
                }
                Err(e) => {
                    warn!(
                        skill_id = %skill_id,
                        error = %e,
                        "Failed to load skill, skipping"
                    );
                }
            }
        }

        info!(count = skills.len(), "Skills registry loaded");
        Ok(())
    }

    /// Load a single skill from its SKILL.md file
    fn load_skill(&self, skill_id: &str, path: &PathBuf) -> Result<Skill> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            AetherError::config(format!(
                "Failed to read SKILL.md at {}: {}",
                path.display(),
                e
            ))
        })?;

        Skill::parse(skill_id, &content)
    }

    /// Get a skill by ID
    ///
    /// # Arguments
    ///
    /// * `id` - The skill ID (directory name)
    ///
    /// # Returns
    ///
    /// The skill if found, None otherwise
    pub fn get_skill(&self, id: &str) -> Option<Skill> {
        let skills = self.skills.read().ok()?;
        skills.get(id).cloned()
    }

    /// List all loaded skills
    pub fn list_skills(&self) -> Vec<Skill> {
        let skills = match self.skills.read() {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        skills.values().cloned().collect()
    }

    /// Find a skill matching the user input (keyword matching)
    ///
    /// This is used for auto-matching when enabled. It checks if any
    /// skill's description contains keywords from the user input.
    ///
    /// # Arguments
    ///
    /// * `input` - User input to match against
    ///
    /// # Returns
    ///
    /// The best matching skill, if any
    pub fn find_matching(&self, input: &str) -> Option<Skill> {
        let skills = self.skills.read().ok()?;
        let input_lower = input.to_lowercase();

        // Simple keyword matching based on description
        // Look for skills whose description keywords appear in input
        for skill in skills.values() {
            let description_lower = skill.frontmatter.description.to_lowercase();

            // Extract key action words from description
            let keywords: Vec<&str> = description_lower
                .split_whitespace()
                .filter(|w| w.len() > 3) // Skip short words
                .filter(|w| {
                    // Keep action words
                    matches!(
                        *w,
                        "improve"
                            | "polish"
                            | "refine"
                            | "translate"
                            | "summarize"
                            | "summary"
                            | "enhance"
                            | "fix"
                            | "correct"
                    )
                })
                .collect();

            // Check if any keyword appears in input
            for keyword in keywords {
                if input_lower.contains(keyword) {
                    debug!(
                        skill_id = %skill.id,
                        keyword = %keyword,
                        "Found matching skill via keyword"
                    );
                    return Some(skill.clone());
                }
            }
        }

        None
    }

    /// Reload all skills (hot reload)
    pub fn reload(&self) -> Result<()> {
        info!("Reloading skills registry");
        self.load_all()
    }

    /// Get the skills directory path
    pub fn skills_dir(&self) -> &PathBuf {
        &self.skills_dir
    }

    /// Check if a skill exists
    pub fn has_skill(&self, id: &str) -> bool {
        let skills = match self.skills.read() {
            Ok(s) => s,
            Err(_) => return false,
        };
        skills.contains_key(id)
    }

    /// Get the number of loaded skills
    pub fn count(&self) -> usize {
        let skills = match self.skills.read() {
            Ok(s) => s,
            Err(_) => return 0,
        };
        skills.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_skill(dir: &PathBuf, name: &str, description: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();

        let content = format!(
            r#"---
name: {}
description: {}
---

# {}

Some instructions here.
"#,
            name, description, name
        );

        fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    #[test]
    fn test_registry_load_all() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(&skills_dir, "refine-text", "Improve and polish writing");
        create_test_skill(&skills_dir, "translate", "Translate text between languages");

        let registry = SkillsRegistry::new(skills_dir);
        registry.load_all().unwrap();

        assert_eq!(registry.count(), 2);
        assert!(registry.has_skill("refine-text"));
        assert!(registry.has_skill("translate"));
    }

    #[test]
    fn test_registry_get_skill() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(&skills_dir, "refine-text", "Improve and polish writing");

        let registry = SkillsRegistry::new(skills_dir);
        registry.load_all().unwrap();

        let skill = registry.get_skill("refine-text").unwrap();
        assert_eq!(skill.name(), "refine-text");

        assert!(registry.get_skill("nonexistent").is_none());
    }

    #[test]
    fn test_registry_list_skills() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(&skills_dir, "skill-a", "Description A");
        create_test_skill(&skills_dir, "skill-b", "Description B");

        let registry = SkillsRegistry::new(skills_dir);
        registry.load_all().unwrap();

        let skills = registry.list_skills();
        assert_eq!(skills.len(), 2);
    }

    #[test]
    fn test_registry_find_matching() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(&skills_dir, "refine-text", "Improve and polish writing");
        create_test_skill(&skills_dir, "translate", "Translate text between languages");

        let registry = SkillsRegistry::new(skills_dir);
        registry.load_all().unwrap();

        // Should match refine-text
        let matched = registry.find_matching("please improve this text");
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().id, "refine-text");

        // Should match translate
        let matched = registry.find_matching("translate this to French");
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().id, "translate");

        // No match
        let matched = registry.find_matching("what is the weather?");
        assert!(matched.is_none());
    }

    #[test]
    fn test_registry_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        let registry = SkillsRegistry::new(skills_dir);
        registry.load_all().unwrap();

        assert_eq!(registry.count(), 0);
        assert!(registry.list_skills().is_empty());
    }

    #[test]
    fn test_registry_nonexistent_directory() {
        let registry = SkillsRegistry::new(PathBuf::from("/nonexistent/path"));
        registry.load_all().unwrap(); // Should not error

        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_registry_skips_invalid_skills() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        // Create a valid skill
        create_test_skill(&skills_dir, "valid-skill", "A valid skill");

        // Create an invalid skill (missing description)
        let invalid_dir = skills_dir.join("invalid-skill");
        fs::create_dir_all(&invalid_dir).unwrap();
        fs::write(
            invalid_dir.join("SKILL.md"),
            "---\nname: invalid\n---\nNo description",
        )
        .unwrap();

        let registry = SkillsRegistry::new(skills_dir);
        registry.load_all().unwrap();

        // Only valid skill should be loaded
        assert_eq!(registry.count(), 1);
        assert!(registry.has_skill("valid-skill"));
        assert!(!registry.has_skill("invalid-skill"));
    }

    #[test]
    fn test_registry_reload() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(&skills_dir, "skill-1", "First skill");

        let registry = SkillsRegistry::new(skills_dir.clone());
        registry.load_all().unwrap();
        assert_eq!(registry.count(), 1);

        // Add another skill
        create_test_skill(&skills_dir, "skill-2", "Second skill");

        // Reload
        registry.reload().unwrap();
        assert_eq!(registry.count(), 2);
    }
}
