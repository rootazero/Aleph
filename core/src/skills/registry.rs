//! Skills Registry - manages loaded skills from the skills directory.
//!
//! The registry scans the skills directory for SKILL.md files, parses them,
//! and provides lookup functionality.
//!
//! ## Multi-location Discovery
//!
//! Skills are discovered from multiple locations in priority order:
//! 1. Project level: `.aleph/skills/`, `.claude/skills/` (traverse up to git root)
//! 2. User level: `~/.aleph/skills`, `~/.claude/skills`
//!
//! ## Progressive Disclosure
//!
//! Skills support Progressive Disclosure for efficient context usage:
//! - **Level 1 (Metadata)**: Name, description, location - always available in system prompt
//! - **Level 2 (Instructions)**: Full SKILL.md content - loaded via read_skill tool
//! - **Level 3 (Resources)**: Additional files - loaded on-demand via file_name parameter

use crate::error::{AlephError, Result};
use crate::skills::events::SkillRegistryEvent;
use crate::skills::health::HealthChecker;
use crate::skills::types::SkillHealth;
use crate::skills::Skill;
use crate::utils::paths::get_all_skills_dirs;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

// ============================================================================
// Skill Metadata (Progressive Disclosure Level 1)
// ============================================================================

/// Skill metadata for Progressive Disclosure Level 1
///
/// Contains only the essential information needed for the system prompt.
/// Full content is loaded on-demand via the read_skill tool.
#[derive(Debug, Clone, Serialize)]
pub struct SkillMetadata {
    /// Skill ID (directory name)
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Brief description for tool selection
    pub description: String,

    /// Full path to the skill directory
    pub location: PathBuf,

    /// Source of this skill (project or global)
    pub source: SkillSource,
}

/// Where a skill was discovered from
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    /// Project-level skill (.aleph/skills or .claude/skills)
    Project,
    /// Global user-level skill (~/.aleph/skills or ~/.claude/skills)
    Global,
}

// ============================================================================
// Skill with Health
// ============================================================================

/// Skill with its health status
#[derive(Debug, Clone)]
pub struct SkillWithHealth {
    pub skill: Skill,
    pub health: SkillHealth,
}

// ============================================================================
// Skills Registry
// ============================================================================

/// Default capacity for the event broadcast channel
const EVENT_CHANNEL_CAPACITY: usize = 64;

/// Skills Registry manages loaded skills
pub struct SkillsRegistry {
    /// Primary skills directory path (for backwards compatibility)
    skills_dir: PathBuf,

    /// All skills directories (for multi-location discovery)
    skills_dirs: Vec<PathBuf>,

    /// Loaded skills indexed by ID
    skills: RwLock<HashMap<String, Skill>>,

    /// Skill metadata indexed by ID (for Progressive Disclosure)
    metadata: RwLock<HashMap<String, SkillMetadata>>,

    /// Event broadcast sender for skill lifecycle events
    event_tx: broadcast::Sender<SkillRegistryEvent>,
}

impl SkillsRegistry {
    /// Create a new skills registry with a single skills directory
    ///
    /// # Arguments
    ///
    /// * `skills_dir` - Path to the skills directory
    pub fn new(skills_dir: PathBuf) -> Self {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            skills_dirs: vec![skills_dir.clone()],
            skills_dir,
            skills: RwLock::new(HashMap::new()),
            metadata: RwLock::new(HashMap::new()),
            event_tx,
        }
    }

    /// Create a skills registry with multi-location discovery
    ///
    /// Automatically discovers skills from:
    /// - Project level: .aleph/skills/, .claude/skills/
    /// - Global level: ~/.aleph/skills, ~/.claude/skills
    ///
    /// # Arguments
    ///
    /// * `project_dir` - Optional project directory to start discovery from
    pub fn with_auto_discover(project_dir: Option<&std::path::Path>) -> Result<Self> {
        let skills_dirs = get_all_skills_dirs(project_dir)?;

        // Use first directory as primary (for backwards compatibility)
        let skills_dir = skills_dirs.first().cloned().unwrap_or_else(|| {
            crate::utils::paths::get_skills_dir().unwrap_or_else(|_| PathBuf::from("~/.aleph/skills"))
        });

        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Ok(Self {
            skills_dirs,
            skills_dir,
            skills: RwLock::new(HashMap::new()),
            metadata: RwLock::new(HashMap::new()),
            event_tx,
        })
    }

    /// Create a skills registry with specific directories
    ///
    /// # Arguments
    ///
    /// * `skills_dirs` - List of skills directories in priority order
    pub fn with_directories(skills_dirs: Vec<PathBuf>) -> Self {
        let skills_dir = skills_dirs.first().cloned().unwrap_or_else(|| PathBuf::from("."));
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);

        Self {
            skills_dirs,
            skills_dir,
            skills: RwLock::new(HashMap::new()),
            metadata: RwLock::new(HashMap::new()),
            event_tx,
        }
    }

    /// Subscribe to skill registry events
    ///
    /// Returns a receiver that will receive events for skill lifecycle changes.
    /// Events include SkillLoaded, SkillRemoved, and AllReloaded.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut events = registry.subscribe();
    /// tokio::spawn(async move {
    ///     while let Ok(event) = events.recv().await {
    ///         match event {
    ///             SkillRegistryEvent::AllReloaded { count, .. } => {
    ///                 println!("Reloaded {} skills", count);
    ///             }
    ///             _ => {}
    ///         }
    ///     }
    /// });
    /// ```
    pub fn subscribe(&self) -> broadcast::Receiver<SkillRegistryEvent> {
        self.event_tx.subscribe()
    }

    /// Emit an event to all subscribers
    fn emit_event(&self, event: SkillRegistryEvent) {
        // Ignore send errors (no subscribers is fine)
        let _ = self.event_tx.send(event);
    }

    /// Load all skills from all configured skills directories
    ///
    /// Scans subdirectories for SKILL.md files and parses them.
    /// Invalid skills are logged and skipped.
    /// Earlier directories take priority (first occurrence wins).
    pub fn load_all(&self) -> Result<()> {
        // Debug: Log all directories to be scanned
        info!(
            dirs = ?self.skills_dirs,
            count = self.skills_dirs.len(),
            "Loading skills from multiple directories"
        );

        let mut skills = self
            .skills
            .write()
            .map_err(|_| AlephError::config("Failed to acquire write lock on skills registry"))?;

        let mut metadata = self
            .metadata
            .write()
            .map_err(|_| AlephError::config("Failed to acquire write lock on skills metadata"))?;

        skills.clear();
        metadata.clear();

        // Get home directory for determining source
        let home_dir = crate::utils::paths::get_home_dir().ok();

        // Scan all directories
        for skills_dir in &self.skills_dirs {
            if !skills_dir.exists() {
                debug!(path = %skills_dir.display(), "Skills directory does not exist");
                continue;
            }

            let entries = match std::fs::read_dir(skills_dir) {
                Ok(e) => e,
                Err(e) => {
                    warn!(
                        path = %skills_dir.display(),
                        error = %e,
                        "Failed to read skills directory"
                    );
                    continue;
                }
            };

            // Determine source (project vs global)
            let source = if let Some(ref home) = home_dir {
                if skills_dir.starts_with(home) {
                    SkillSource::Global
                } else {
                    SkillSource::Project
                }
            } else {
                SkillSource::Project
            };

            // Collect directories to scan (supports nested category folders)
            let mut dirs_to_scan: Vec<PathBuf> = Vec::new();

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let dir_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();

                // Skip hidden directories
                if dir_name.starts_with('.') {
                    continue;
                }

                let skill_md_path = path.join("SKILL.md");
                if skill_md_path.exists() {
                    // Direct skill directory: skills/<name>/SKILL.md
                    dirs_to_scan.push(path);
                } else {
                    // Category directory: skills/<category>/<name>/SKILL.md
                    // Recurse one level into subdirectories
                    if let Ok(sub_entries) = std::fs::read_dir(&path) {
                        for sub_entry in sub_entries.flatten() {
                            let sub_path = sub_entry.path();
                            if sub_path.is_dir() {
                                let sub_name = sub_path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or_default();
                                if !sub_name.starts_with('.') && sub_path.join("SKILL.md").exists() {
                                    dirs_to_scan.push(sub_path);
                                }
                            }
                        }
                    }
                }
            }

            for path in dirs_to_scan {
                let skill_id = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();

                // Skip if already loaded (first occurrence wins)
                if skills.contains_key(&skill_id) {
                    debug!(
                        skill_id = %skill_id,
                        path = %path.display(),
                        "Skill already loaded from earlier directory, skipping duplicate"
                    );
                    continue;
                }

                let skill_md_path = path.join("SKILL.md");

                match self.load_skill(&skill_id, &skill_md_path) {
                    Ok(skill) => {
                        info!(
                            skill_id = %skill_id,
                            name = %skill.frontmatter.name,
                            source = ?source,
                            "Loaded skill"
                        );

                        // Create metadata for Progressive Disclosure
                        let meta = SkillMetadata {
                            id: skill_id.clone(),
                            name: skill.frontmatter.name.clone(),
                            description: skill.frontmatter.description.clone(),
                            location: path.clone(),
                            source,
                        };
                        metadata.insert(skill_id.clone(), meta);

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
        }

        let skill_count = skills.len();
        let skill_ids: Vec<String> = skills.keys().cloned().collect();

        info!(count = skill_count, "Skills registry loaded");

        // Drop locks before emitting event to avoid deadlock
        drop(skills);
        drop(metadata);

        // Emit AllReloaded event
        self.emit_event(SkillRegistryEvent::all_reloaded(skill_count, skill_ids));

        Ok(())
    }

    /// Load a single skill from its SKILL.md file
    fn load_skill(&self, skill_id: &str, path: &PathBuf) -> Result<Skill> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            AlephError::config(format!(
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

    /// List skill metadata for Progressive Disclosure Level 1
    ///
    /// Returns only the essential metadata (id, name, description, location)
    /// without loading full skill content. This is suitable for system prompts.
    pub fn list_skill_metadata(&self) -> Vec<SkillMetadata> {
        let metadata = match self.metadata.read() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };

        let mut result: Vec<_> = metadata.values().cloned().collect();
        // Sort by source (Project first), then by name
        result.sort_by(|a, b| {
            match (a.source, b.source) {
                (SkillSource::Project, SkillSource::Global) => std::cmp::Ordering::Less,
                (SkillSource::Global, SkillSource::Project) => std::cmp::Ordering::Greater,
                _ => a.name.cmp(&b.name),
            }
        });
        result
    }

    /// Get skill metadata by ID
    pub fn get_skill_metadata(&self, id: &str) -> Option<SkillMetadata> {
        let metadata = self.metadata.read().ok()?;
        metadata.get(id).cloned()
    }

    /// Get the full content of a skill file
    ///
    /// This loads the skill content on-demand (Progressive Disclosure Level 2).
    ///
    /// # Arguments
    ///
    /// * `id` - The skill ID
    /// * `file_name` - Optional file name within the skill directory (defaults to "SKILL.md")
    ///
    /// # Returns
    ///
    /// The file content if found
    pub fn get_skill_content(&self, id: &str, file_name: Option<&str>) -> Result<String> {
        let metadata = self.metadata.read()
            .map_err(|_| AlephError::config("Failed to acquire read lock on skills metadata"))?;

        let meta = metadata.get(id)
            .ok_or_else(|| AlephError::config(format!("Skill '{}' not found", id)))?;

        let file = file_name.unwrap_or("SKILL.md");
        let file_path = meta.location.join(file);

        if !file_path.exists() {
            return Err(AlephError::config(format!(
                "File '{}' not found in skill '{}'",
                file, id
            )));
        }

        std::fs::read_to_string(&file_path)
            .map_err(|e| AlephError::config(format!("Failed to read skill file: {}", e)))
    }

    /// List files available in a skill directory
    pub fn list_skill_files(&self, id: &str) -> Result<Vec<String>> {
        let metadata = self.metadata.read()
            .map_err(|_| AlephError::config("Failed to acquire read lock on skills metadata"))?;

        let meta = metadata.get(id)
            .ok_or_else(|| AlephError::config(format!("Skill '{}' not found", id)))?;

        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&meta.location) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_file() {
                        if let Some(name) = entry.file_name().to_str() {
                            if !name.starts_with('.') {
                                files.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }

        files.sort();
        Ok(files)
    }

    /// Get all configured skills directories
    pub fn skills_dirs(&self) -> &[PathBuf] {
        &self.skills_dirs
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

    /// Load all skills and check their health
    pub fn load_all_with_health(&self) -> Vec<SkillWithHealth> {
        let skills = self.list_skills();
        skills
            .into_iter()
            .map(|skill| {
                let health = HealthChecker::check_skill(&skill);
                SkillWithHealth { skill, health }
            })
            .collect()
    }

    /// List only healthy skills
    pub fn list_healthy_skills(&self) -> Vec<Skill> {
        self.load_all_with_health()
            .into_iter()
            .filter(|s| s.health == SkillHealth::Healthy)
            .map(|s| s.skill)
            .collect()
    }

    /// List degraded skills with their missing dependencies
    pub fn list_degraded_skills(&self) -> Vec<(Skill, Vec<String>)> {
        self.load_all_with_health()
            .into_iter()
            .filter_map(|s| match s.health {
                SkillHealth::Degraded { missing } => Some((s.skill, missing)),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn create_test_skill(dir: &Path, name: &str, description: &str) {
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

    #[test]
    fn test_load_all_with_health() {
        use crate::skills::types::SkillHealth;

        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        // Create a skill with requirements for 'ls' (should exist)
        let skill_dir = skills_dir.join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: test-skill
description: Test skill
requirements:
  binaries:
    - ls
---
Instructions
"#,
        )
        .unwrap();

        let registry = SkillsRegistry::new(skills_dir);
        registry.load_all().unwrap();

        let with_health = registry.load_all_with_health();
        assert_eq!(with_health.len(), 1);
        assert_eq!(with_health[0].health, SkillHealth::Healthy);
    }

    #[test]
    fn test_list_degraded_skills() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        // Create a skill with nonexistent binary
        let skill_dir = skills_dir.join("broken-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: broken-skill
description: Broken skill
requirements:
  binaries:
    - nonexistent_binary_12345
---
Instructions
"#,
        )
        .unwrap();

        let registry = SkillsRegistry::new(skills_dir);
        registry.load_all().unwrap();

        let degraded = registry.list_degraded_skills();
        assert_eq!(degraded.len(), 1);
        assert_eq!(degraded[0].0.id, "broken-skill");
        assert_eq!(degraded[0].1, vec!["nonexistent_binary_12345"]);
    }

    #[test]
    fn test_registry_nested_category_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        // Create nested category structure: skills/<category>/<skill>/SKILL.md
        let foundation = skills_dir.join("foundation");
        create_test_skill(&foundation, "test", "Run tests");
        create_test_skill(&foundation, "debug", "Debug code");

        let automation = skills_dir.join("automation");
        create_test_skill(&automation, "ssh", "SSH operations");

        // Also create a flat skill alongside categories
        create_test_skill(&skills_dir, "custom-skill", "A custom skill");

        let registry = SkillsRegistry::new(skills_dir);
        registry.load_all().unwrap();

        // Should find all 4 skills (2 nested + 1 nested + 1 flat)
        assert_eq!(registry.count(), 4);
        assert!(registry.has_skill("test"));
        assert!(registry.has_skill("debug"));
        assert!(registry.has_skill("ssh"));
        assert!(registry.has_skill("custom-skill"));
    }
}
