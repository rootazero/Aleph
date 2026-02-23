//! Skill System v2
//!
//! Provides a snapshot-based skill management system that integrates with
//! the ExtensionManager. Skills are scanned from directories, parsed into
//! a SkillSnapshot containing prompt XML for LLM injection.
//!
//! # Architecture
//!
//! ```text
//! SkillSystem
//!   └── Arc<RwLock<SkillSnapshot>>  (current cached snapshot)
//!       ├── prompt_xml: String       (XML for system prompt)
//!       └── skills: Vec<SkillEntry>  (parsed skill entries)
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A snapshot of available skills at a point in time
#[derive(Debug, Clone)]
pub struct SkillSnapshot {
    /// XML-formatted prompt text describing available skills
    /// for injection into the LLM system prompt
    pub prompt_xml: String,

    /// Individual skill entries
    pub skills: Vec<SkillEntry>,

    /// When this snapshot was built
    pub built_at: chrono::DateTime<chrono::Utc>,
}

impl Default for SkillSnapshot {
    fn default() -> Self {
        Self {
            prompt_xml: String::new(),
            skills: Vec::new(),
            built_at: chrono::Utc::now(),
        }
    }
}

/// A single skill entry in the snapshot
#[derive(Debug, Clone)]
pub struct SkillEntry {
    /// Skill name
    pub name: String,
    /// Skill description
    pub description: String,
    /// Qualified name (plugin:skill or skill)
    pub qualified_name: String,
}

/// SkillSystem v2 - manages skill scanning, loading, and snapshot generation
///
/// This is Clone-able via Arc<Inner> pattern, allowing cheap sharing
/// across multiple components (Gateway handlers, execution engine, etc.)
#[derive(Clone)]
pub struct SkillSystem {
    inner: Arc<Inner>,
}

struct Inner {
    /// Current skill snapshot (cached, updated on reload)
    snapshot: RwLock<SkillSnapshot>,
    /// Directories to scan for skills
    skill_dirs: RwLock<Vec<PathBuf>>,
}

impl SkillSystem {
    /// Create a new empty SkillSystem
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                snapshot: RwLock::new(SkillSnapshot::default()),
                skill_dirs: RwLock::new(Vec::new()),
            }),
        }
    }

    /// Get the current skill snapshot
    ///
    /// This is a fast read from the cached RwLock value.
    pub async fn current_snapshot(&self) -> SkillSnapshot {
        self.inner.snapshot.read().await.clone()
    }

    /// Add a directory to scan for skills
    pub async fn add_skill_dir(&self, dir: PathBuf) {
        self.inner.skill_dirs.write().await.push(dir);
    }

    /// Scan all registered directories and rebuild the snapshot
    pub async fn rebuild_snapshot(&self) {
        let dirs = self.inner.skill_dirs.read().await.clone();
        let mut entries = Vec::new();

        for dir in &dirs {
            if !dir.exists() {
                continue;
            }

            // Scan for SKILL.md files in subdirectories
            if let Ok(read_dir) = std::fs::read_dir(dir) {
                for entry in read_dir.flatten() {
                    let skill_file = entry.path().join("SKILL.md");
                    if skill_file.exists() {
                        if let Ok(content) = std::fs::read_to_string(&skill_file) {
                            let name = entry
                                .file_name()
                                .to_string_lossy()
                                .to_string();
                            let description = content
                                .lines()
                                .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
                                .unwrap_or("No description")
                                .trim()
                                .to_string();

                            entries.push(SkillEntry {
                                qualified_name: name.clone(),
                                name,
                                description,
                            });
                        }
                    }
                }
            }
        }

        // Build prompt XML
        let prompt_xml = build_prompt_xml(&entries);

        let snapshot = SkillSnapshot {
            prompt_xml,
            skills: entries,
            built_at: chrono::Utc::now(),
        };

        *self.inner.snapshot.write().await = snapshot;
    }

    /// Initialize by scanning default directories and building initial snapshot
    pub async fn init_from_defaults(&self) {
        // Add default skill directories
        if let Some(home) = dirs::home_dir() {
            let global_skills = home.join(".aleph/skills");
            if global_skills.exists() {
                self.add_skill_dir(global_skills).await;
            }
        }

        // Add project-local skills (from current directory)
        let local_skills = PathBuf::from(".aleph/skills");
        if local_skills.exists() {
            self.add_skill_dir(local_skills).await;
        }

        // Build initial snapshot
        self.rebuild_snapshot().await;
    }
}

impl Default for SkillSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// Build XML prompt text from skill entries
fn build_prompt_xml(entries: &[SkillEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }

    let mut xml = String::from("<available_skills>\n");
    for entry in entries {
        xml.push_str(&format!(
            "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n  </skill>\n",
            entry.qualified_name, entry.description
        ));
    }
    xml.push_str("</available_skills>");
    xml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_prompt_xml_empty() {
        assert_eq!(build_prompt_xml(&[]), "");
    }

    #[test]
    fn test_build_prompt_xml_with_entries() {
        let entries = vec![SkillEntry {
            name: "test".to_string(),
            description: "A test skill".to_string(),
            qualified_name: "test".to_string(),
        }];
        let xml = build_prompt_xml(&entries);
        assert!(xml.contains("<available_skills>"));
        assert!(xml.contains("<name>test</name>"));
        assert!(xml.contains("<description>A test skill</description>"));
    }

    #[tokio::test]
    async fn test_skill_system_clone() {
        let sys = SkillSystem::new();
        let sys2 = sys.clone();

        // Both should see the same snapshot
        let snap1 = sys.current_snapshot().await;
        let snap2 = sys2.current_snapshot().await;
        assert_eq!(snap1.skills.len(), snap2.skills.len());
    }

    #[tokio::test]
    async fn test_skill_system_default_snapshot() {
        let sys = SkillSystem::new();
        let snapshot = sys.current_snapshot().await;
        assert!(snapshot.prompt_xml.is_empty());
        assert!(snapshot.skills.is_empty());
    }
}
