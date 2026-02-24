//! Skill System v2 — Domain-Driven Skill Management
//!
//! This module provides the runtime infrastructure for skill registration,
//! eligibility evaluation, SKILL.md parsing, prompt injection, and a
//! unified `SkillSystem` facade for the rest of the application.

pub mod commands;
pub mod eligibility;
pub mod installer;
pub mod manifest;
pub mod prompt;
pub mod registry;
pub mod snapshot;
pub mod status;

pub use commands::{list_available_commands, resolve_command, SkillCommandSpec};
pub use eligibility::{EligibilityResult, EligibilityService, IneligibilityReason};
pub use installer::{build_install_command, filter_install_specs_for_current_os};
pub use manifest::{parse_skill_content, parse_skill_file, SkillParseError};
pub use prompt::build_skills_prompt_xml;
pub use registry::SkillRegistry;
pub use snapshot::SkillSnapshot;
pub use status::SkillStatusReport;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::domain::skill::{SkillId, SkillManifest, SkillSource};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur in the skill system.
#[derive(Debug)]
pub enum SkillSystemError {
    /// Error parsing a skill file.
    Parse(SkillParseError),
    /// I/O error.
    Io(std::io::Error),
}

impl std::fmt::Display for SkillSystemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "skill parse error: {}", e),
            Self::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for SkillSystemError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(e) => Some(e),
            Self::Io(e) => Some(e),
        }
    }
}

impl From<SkillParseError> for SkillSystemError {
    fn from(e: SkillParseError) -> Self {
        Self::Parse(e)
    }
}

impl From<std::io::Error> for SkillSystemError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// ---------------------------------------------------------------------------
// SkillSystem facade
// ---------------------------------------------------------------------------

/// The main entry point for the skill system.
///
/// `SkillSystem` is cheaply cloneable (via `Arc`) and provides async-safe
/// access to the skill registry, eligibility evaluation, snapshots, and
/// slash command resolution.
#[derive(Clone)]
pub struct SkillSystem {
    inner: Arc<Inner>,
}

struct Inner {
    registry: RwLock<SkillRegistry>,
    snapshot: RwLock<SkillSnapshot>,
    skill_dirs: RwLock<Vec<PathBuf>>,
    version_counter: RwLock<u64>,
    eligibility: EligibilityService,
}

impl SkillSystem {
    /// Create a new, empty skill system.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                registry: RwLock::new(SkillRegistry::new()),
                snapshot: RwLock::new(SkillSnapshot::empty()),
                skill_dirs: RwLock::new(Vec::new()),
                version_counter: RwLock::new(0),
                eligibility: EligibilityService::new(),
            }),
        }
    }

    /// Initialize the skill system by scanning the given directories.
    ///
    /// Each directory is scanned for SKILL.md files. The source is guessed
    /// from the path. After scanning, a snapshot is built.
    pub async fn init(&self, dirs: Vec<PathBuf>) -> Result<(), SkillSystemError> {
        {
            let mut skill_dirs = self.inner.skill_dirs.write().await;
            *skill_dirs = dirs.clone();
        }

        let mut registry = self.inner.registry.write().await;
        registry.clear();

        for dir in &dirs {
            if dir.exists() {
                let source = guess_source(dir);
                let manifests = scan_directory(dir, source);
                registry.register_all(manifests);
            }
        }

        drop(registry);
        self.rebuild_snapshot().await;

        Ok(())
    }

    /// Rebuild the snapshot from the current registry state.
    ///
    /// Increments the version counter and builds a new snapshot.
    pub async fn rebuild(&self) -> Result<(), SkillSystemError> {
        // Re-scan directories
        let dirs = self.inner.skill_dirs.read().await.clone();

        let mut registry = self.inner.registry.write().await;
        registry.clear();

        for dir in &dirs {
            if dir.exists() {
                let source = guess_source(dir);
                let manifests = scan_directory(dir, source);
                registry.register_all(manifests);
            }
        }

        drop(registry);
        self.rebuild_snapshot().await;

        Ok(())
    }

    /// Reload a single skill file into the registry and rebuild the snapshot.
    pub async fn reload_file(&self, path: impl AsRef<Path>) -> Result<(), SkillSystemError> {
        let path = path.as_ref();
        let source = guess_source(path);
        let manifest = parse_skill_file(path, source)?;

        let mut registry = self.inner.registry.write().await;
        registry.register(manifest);
        drop(registry);

        self.rebuild_snapshot().await;

        Ok(())
    }

    /// Get a clone of the current snapshot.
    pub async fn current_snapshot(&self) -> SkillSnapshot {
        self.inner.snapshot.read().await.clone()
    }

    /// Get a skill manifest by ID.
    pub async fn get_skill(&self, id: &SkillId) -> Option<SkillManifest> {
        self.inner.registry.read().await.get(id).cloned()
    }

    /// List all registered skill manifests.
    pub async fn list_skills(&self) -> Vec<SkillManifest> {
        self.inner
            .registry
            .read()
            .await
            .list_all()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Build status reports for all registered skills.
    pub async fn skill_status(&self) -> Vec<SkillStatusReport> {
        let registry = self.inner.registry.read().await;
        registry
            .list_all()
            .into_iter()
            .map(|m| {
                let result = self.inner.eligibility.evaluate(m);
                SkillStatusReport::from_manifest(m, result)
            })
            .collect()
    }

    /// Resolve a slash command name to a skill command spec.
    pub async fn resolve_command(&self, name: &str) -> Option<SkillCommandSpec> {
        let registry = self.inner.registry.read().await;
        commands::resolve_command(name, &registry)
    }

    // --- Private helpers ---

    /// Increment the version counter and build a new snapshot.
    async fn rebuild_snapshot(&self) {
        let mut version = self.inner.version_counter.write().await;
        *version += 1;
        let current_version = *version;
        drop(version);

        let registry = self.inner.registry.read().await;
        let new_snapshot =
            SkillSnapshot::build(&registry, &self.inner.eligibility, current_version);
        drop(registry);

        let mut snapshot = self.inner.snapshot.write().await;
        *snapshot = new_snapshot;
    }
}

impl Default for SkillSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SkillSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillSystem")
            .field("arc_strong_count", &Arc::strong_count(&self.inner))
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Scan a directory for SKILL.md files and parse them.
///
/// Non-parseable files are silently skipped.
fn scan_directory(dir: &Path, source: SkillSource) -> Vec<SkillManifest> {
    let mut manifests = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return manifests,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_file() && is_skill_file(&path) {
            match parse_skill_file(&path, source.clone()) {
                Ok(manifest) => manifests.push(manifest),
                Err(e) => {
                    tracing::warn!("failed to parse skill file {:?}: {}", path, e);
                }
            }
        }

        // Recurse into subdirectories
        if path.is_dir() {
            let sub = scan_directory(&path, source.clone());
            manifests.extend(sub);
        }
    }

    manifests
}

/// Check if a file looks like a SKILL.md file.
fn is_skill_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case("SKILL.md"))
        .unwrap_or(false)
}

/// Guess the `SkillSource` from a file path.
///
/// - Contains `.aleph/skills` and is under a workspace → Workspace
/// - Contains `.aleph/skills` at home dir level → Global
/// - Otherwise → Bundled
fn guess_source(path: &Path) -> SkillSource {
    let path_str = path.to_string_lossy();

    if path_str.contains(".aleph/skills") {
        if let Some(home) = dirs::home_dir() {
            let home_skills = home.join(".aleph").join("skills");
            if path.starts_with(&home_skills) {
                return SkillSource::Global;
            }
        } else {
            // Cannot determine home directory — fall back to Global for safety
            // so we don't accidentally give workspace-level priority
            tracing::warn!("dirs::home_dir() returned None, defaulting to Global source");
            return SkillSource::Global;
        }
        return SkillSource::Workspace;
    }

    SkillSource::Bundled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::SkillSource;

    #[test]
    fn clone_shares_state() {
        let sys1 = SkillSystem::new();
        let sys2 = sys1.clone();

        // Both point to the same Arc
        assert!(Arc::ptr_eq(&sys1.inner, &sys2.inner));
    }

    #[tokio::test]
    async fn init_with_temp_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let skill_file = dir.path().join("SKILL.md");

        let content = r#"---
name: Test Skill
description: A test skill for unit tests
---
You are a test expert."#;
        std::fs::write(&skill_file, content).unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let snapshot = system.current_snapshot().await;
        assert!(snapshot.version > 0);
        assert!(!snapshot.eligible.is_empty());

        let skills = system.list_skills().await;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name(), "Test Skill");
    }

    #[tokio::test]
    async fn rebuild_increments_version() {
        let dir = tempfile::TempDir::new().unwrap();
        let skill_file = dir.path().join("SKILL.md");

        let content = r#"---
name: Version Test
description: Tests version increments
---
Content."#;
        std::fs::write(&skill_file, content).unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let v1 = system.current_snapshot().await.version;

        system.rebuild().await.unwrap();
        let v2 = system.current_snapshot().await.version;

        system.rebuild().await.unwrap();
        let v3 = system.current_snapshot().await.version;

        assert!(v2 > v1);
        assert!(v3 > v2);
    }

    #[tokio::test]
    async fn list_skills() {
        let dir = tempfile::TempDir::new().unwrap();

        // Create two skill subdirectories with SKILL.md files
        let sub1 = dir.path().join("skill1");
        std::fs::create_dir(&sub1).unwrap();
        std::fs::write(
            sub1.join("SKILL.md"),
            r#"---
name: Skill One
description: First skill
---
Content one."#,
        )
        .unwrap();

        let sub2 = dir.path().join("skill2");
        std::fs::create_dir(&sub2).unwrap();
        std::fs::write(
            sub2.join("SKILL.md"),
            r#"---
name: Skill Two
description: Second skill
---
Content two."#,
        )
        .unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let skills = system.list_skills().await;
        assert_eq!(skills.len(), 2);

        let names: Vec<&str> = skills.iter().map(|s| s.name()).collect();
        assert!(names.contains(&"Skill One"));
        assert!(names.contains(&"Skill Two"));
    }

    #[tokio::test]
    async fn resolve_command_through_facade() {
        let dir = tempfile::TempDir::new().unwrap();
        let skill_file = dir.path().join("SKILL.md");

        std::fs::write(
            &skill_file,
            r#"---
name: Git Commit
description: Helps with git commits
---
Git expert."#,
        )
        .unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await.unwrap();

        // The ID will be "git-commit" (derived from name by parser)
        let result = system.resolve_command("git-commit").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "Git Commit");
    }

    #[tokio::test]
    async fn skill_status_reports() {
        let dir = tempfile::TempDir::new().unwrap();
        let skill_file = dir.path().join("SKILL.md");

        std::fs::write(
            &skill_file,
            r#"---
name: Status Test
description: Tests status reporting
---
Content."#,
        )
        .unwrap();

        let system = SkillSystem::new();
        system.init(vec![dir.path().to_path_buf()]).await.unwrap();

        let statuses = system.skill_status().await;
        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].is_eligible());
    }

    #[test]
    fn guess_source_workspace() {
        let path = PathBuf::from("/some/project/.aleph/skills/git/SKILL.md");
        assert_eq!(guess_source(&path), SkillSource::Workspace);
    }

    #[test]
    fn guess_source_bundled_fallback() {
        let path = PathBuf::from("/usr/share/aleph/skills/git/SKILL.md");
        assert_eq!(guess_source(&path), SkillSource::Bundled);
    }

    #[test]
    fn is_skill_file_detection() {
        assert!(is_skill_file(Path::new("/some/dir/SKILL.md")));
        assert!(is_skill_file(Path::new("/some/dir/skill.md")));
        assert!(!is_skill_file(Path::new("/some/dir/README.md")));
        assert!(!is_skill_file(Path::new("/some/dir/")));
    }
}
