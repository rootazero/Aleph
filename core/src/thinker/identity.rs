//! Identity Resolver - Layered identity resolution for AI embodiment
//!
//! This module provides the IdentityResolver that resolves identity from
//! multiple sources with configurable priority.
//!
//! # Priority Order (highest to lowest)
//!
//! 1. Session override - Runtime identity set programmatically
//! 2. Project identity - `~/.aleph/projects/<id>/identity.{md,json,toml}`
//! 3. Global soul - `~/.aleph/soul.md`
//! 4. Default - Empty SoulManifest
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    IdentityResolver                          │
//! │  ┌─────────────────────────────────────────────────────┐    │
//! │  │ Priority Stack                                       │    │
//! │  │   ┌─────────────┐                                    │    │
//! │  │   │  Session    │ ← Runtime override (highest)       │    │
//! │  │   ├─────────────┤                                    │    │
//! │  │   │  Project    │ ← ~/.aleph/projects/<id>/identity  │    │
//! │  │   ├─────────────┤                                    │    │
//! │  │   │  Global     │ ← ~/.aleph/soul.md                 │    │
//! │  │   ├─────────────┤                                    │    │
//! │  │   │  Default    │ ← Empty manifest (lowest)          │    │
//! │  │   └─────────────┘                                    │    │
//! │  └─────────────────────────────────────────────────────┘    │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::soul::SoulManifest;
use crate::utils::paths::get_project_dir;

/// Resolves identity from layered sources
///
/// Priority (highest to lowest):
/// 1. Session override
/// 2. Project identity (`~/.aleph/projects/<id>/identity.{md,json,toml}`)
/// 3. Global soul (~/.aleph/soul.md)
/// 4. Default (empty SoulManifest)
pub struct IdentityResolver {
    /// Global soul path (~/.aleph/soul.md)
    global_path: PathBuf,
    /// Project names to search for identity files in ~/.aleph/projects/<id>/
    project_ids: Vec<String>,
    /// Current session override
    session_override: Option<SoulManifest>,
    /// Override base directory for project resolution (testing only)
    projects_base_dir: Option<PathBuf>,
}

impl IdentityResolver {
    /// Create a new resolver with global path
    pub fn new(global_path: PathBuf) -> Self {
        Self {
            global_path,
            project_ids: Vec::new(),
            session_override: None,
            projects_base_dir: None,
        }
    }

    /// Create resolver with default paths
    pub fn with_defaults() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self::new(home.join(".aleph").join("soul.md"))
    }

    /// Add a project by name to search for identity files
    ///
    /// Identity files are looked up at `~/.aleph/projects/<project_id>/identity.{md,json,toml}`
    pub fn add_project(&mut self, project_id: &str) {
        self.project_ids.push(project_id.to_string());
    }

    /// Set session-level identity override
    pub fn set_session_override(&mut self, soul: SoulManifest) {
        self.session_override = Some(soul);
    }

    /// Clear session-level override
    pub fn clear_session_override(&mut self) {
        self.session_override = None;
    }

    /// Check if session has an override
    pub fn has_session_override(&self) -> bool {
        self.session_override.is_some()
    }

    /// Get the global soul path
    pub fn global_path(&self) -> &PathBuf {
        &self.global_path
    }

    /// Get the project IDs
    pub fn project_ids(&self) -> &[String] {
        &self.project_ids
    }

    /// Resolve the effective SoulManifest for current context
    pub fn resolve(&self) -> SoulManifest {
        // Priority: Session > Project > Global > Default
        if let Some(ref override_soul) = self.session_override {
            return override_soul.clone();
        }

        let project_soul = self.load_project_soul();
        let global_soul = self.load_global_soul();

        match (project_soul, global_soul) {
            (Some(project), Some(global)) => project.merge_with(&global),
            (Some(project), None) => project,
            (None, Some(global)) => global,
            (None, None) => SoulManifest::default(),
        }
    }

    /// Resolve project directory for a given project ID
    fn resolve_project_dir(&self, project_id: &str) -> Option<PathBuf> {
        if let Some(base) = &self.projects_base_dir {
            Some(base.join(project_id))
        } else {
            get_project_dir(project_id).ok()
        }
    }

    /// Load soul from project directory (~/.aleph/projects/<id>/)
    fn load_project_soul(&self) -> Option<SoulManifest> {
        for project_id in &self.project_ids {
            let project_dir = match self.resolve_project_dir(project_id) {
                Some(dir) => dir,
                None => continue,
            };

            let paths = [
                project_dir.join("identity.md"),
                project_dir.join("identity.json"),
                project_dir.join("identity.toml"),
            ];
            for path in paths {
                if path.exists() {
                    if let Ok(manifest) = SoulManifest::from_file(&path) {
                        return Some(manifest);
                    }
                }
            }
        }
        None
    }

    /// Load global soul
    fn load_global_soul(&self) -> Option<SoulManifest> {
        if self.global_path.exists() {
            SoulManifest::from_file(&self.global_path).ok()
        } else {
            // Also try .json and .toml variants
            let stem = self.global_path.file_stem().and_then(|s| s.to_str());
            let parent = self.global_path.parent();

            if let (Some(stem), Some(parent)) = (stem, parent) {
                for ext in ["json", "toml"] {
                    let alt_path = parent.join(format!("{}.{}", stem, ext));
                    if alt_path.exists() {
                        if let Ok(manifest) = SoulManifest::from_file(&alt_path) {
                            return Some(manifest);
                        }
                    }
                }
            }
            None
        }
    }

    /// List all available identity sources
    pub fn list_sources(&self) -> Vec<IdentitySource> {
        let mut sources = Vec::new();

        // Check global
        if self.global_path.exists() {
            sources.push(IdentitySource {
                source_type: IdentitySourceType::Global,
                path: self.global_path.clone(),
                loaded: self.load_global_soul().is_some(),
            });
        }

        // Check projects
        for project_id in &self.project_ids {
            let project_dir = match self.resolve_project_dir(project_id) {
                Some(dir) => dir,
                None => continue,
            };

            let paths = [
                project_dir.join("identity.md"),
                project_dir.join("identity.json"),
                project_dir.join("identity.toml"),
            ];
            for path in paths {
                if path.exists() {
                    sources.push(IdentitySource {
                        source_type: IdentitySourceType::Project,
                        path: path.clone(),
                        loaded: SoulManifest::from_file(&path).is_ok(),
                    });
                }
            }
        }

        // Session override
        if self.session_override.is_some() {
            sources.push(IdentitySource {
                source_type: IdentitySourceType::Session,
                path: PathBuf::from("<session>"),
                loaded: true,
            });
        }

        sources
    }
}

/// Information about an identity source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentitySource {
    /// Type of identity source
    pub source_type: IdentitySourceType,
    /// Path to the source file (or "<session>" for session override)
    pub path: PathBuf,
    /// Whether the source was successfully loaded
    pub loaded: bool,
}

/// Type of identity source
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IdentitySourceType {
    /// Global soul file (~/.aleph/soul.md)
    Global,
    /// Project-specific identity file
    Project,
    /// Runtime session override
    Session,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper to create a resolver with a temp-based projects directory
    fn resolver_with_temp_projects(tmp: &TempDir) -> IdentityResolver {
        let mut resolver = IdentityResolver::new(PathBuf::from("/nonexistent"));
        resolver.projects_base_dir = Some(tmp.path().join("projects"));
        resolver
    }

    #[test]
    fn test_resolver_default_empty() {
        let resolver = IdentityResolver::new(PathBuf::from("/nonexistent"));
        let soul = resolver.resolve();
        assert!(soul.is_empty());
    }

    #[test]
    fn test_session_override_takes_priority() {
        let mut resolver = IdentityResolver::new(PathBuf::from("/nonexistent"));

        let override_soul = SoulManifest {
            identity: "I am session override".to_string(),
            ..Default::default()
        };
        resolver.set_session_override(override_soul.clone());

        let resolved = resolver.resolve();
        assert_eq!(resolved.identity, "I am session override");
    }

    #[test]
    fn test_clear_session_override() {
        let mut resolver = IdentityResolver::new(PathBuf::from("/nonexistent"));

        resolver.set_session_override(SoulManifest {
            identity: "Override".to_string(),
            ..Default::default()
        });
        assert!(resolver.has_session_override());

        resolver.clear_session_override();
        assert!(!resolver.has_session_override());
    }

    #[test]
    fn test_load_global_soul() {
        let tmp = TempDir::new().unwrap();
        let global_path = tmp.path().join("soul.json");

        let soul = SoulManifest {
            identity: "Global soul".to_string(),
            ..Default::default()
        };
        fs::write(&global_path, serde_json::to_string(&soul).unwrap()).unwrap();

        let resolver = IdentityResolver::new(global_path);
        let resolved = resolver.resolve();

        assert_eq!(resolved.identity, "Global soul");
    }

    #[test]
    fn test_project_overrides_global() {
        let tmp = TempDir::new().unwrap();

        // Create global soul
        let global_path = tmp.path().join("global.json");
        let global_soul = SoulManifest {
            identity: "Global identity".to_string(),
            directives: vec!["Be helpful".to_string()],
            ..Default::default()
        };
        fs::write(&global_path, serde_json::to_string(&global_soul).unwrap()).unwrap();

        // Create project identity at projects/<id>/identity.json
        let project_dir = tmp.path().join("projects").join("my-project");
        fs::create_dir_all(&project_dir).unwrap();
        let project_soul = SoulManifest {
            identity: "Project identity".to_string(),
            ..Default::default()
        };
        fs::write(
            project_dir.join("identity.json"),
            serde_json::to_string(&project_soul).unwrap(),
        )
        .unwrap();

        let mut resolver = IdentityResolver::new(global_path);
        resolver.projects_base_dir = Some(tmp.path().join("projects"));
        resolver.add_project("my-project");

        let resolved = resolver.resolve();
        // Project identity overrides global
        assert_eq!(resolved.identity, "Project identity");
        // But inherits directives from global
        assert_eq!(resolved.directives, vec!["Be helpful".to_string()]);
    }

    #[test]
    fn test_project_identity_json() {
        let tmp = TempDir::new().unwrap();

        // Create project identity at projects/<id>/identity.json
        let project_dir = tmp.path().join("projects").join("test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let project_soul = SoulManifest {
            identity: "Project identity via json".to_string(),
            ..Default::default()
        };
        fs::write(
            project_dir.join("identity.json"),
            serde_json::to_string(&project_soul).unwrap(),
        )
        .unwrap();

        let mut resolver = resolver_with_temp_projects(&tmp);
        resolver.add_project("test-project");

        let resolved = resolver.resolve();
        assert_eq!(resolved.identity, "Project identity via json");
    }

    #[test]
    fn test_project_identity_toml() {
        let tmp = TempDir::new().unwrap();

        // Create project identity at projects/<id>/identity.toml
        let project_dir = tmp.path().join("projects").join("toml-project");
        fs::create_dir_all(&project_dir).unwrap();
        let toml_content = r#"
identity = "Project identity via toml"
directives = ["Be concise"]
"#;
        fs::write(project_dir.join("identity.toml"), toml_content).unwrap();

        let mut resolver = resolver_with_temp_projects(&tmp);
        resolver.add_project("toml-project");

        let resolved = resolver.resolve();
        assert_eq!(resolved.identity, "Project identity via toml");
        assert_eq!(resolved.directives, vec!["Be concise".to_string()]);
    }

    #[test]
    fn test_toml_format_works() {
        let tmp = TempDir::new().unwrap();
        let global_path = tmp.path().join("soul.toml");

        let toml_content = r#"
identity = "TOML soul"
directives = ["Be concise"]
"#;
        fs::write(&global_path, toml_content).unwrap();

        let resolver = IdentityResolver::new(global_path);
        let resolved = resolver.resolve();

        assert_eq!(resolved.identity, "TOML soul");
        assert_eq!(resolved.directives, vec!["Be concise".to_string()]);
    }

    #[test]
    fn test_list_sources() {
        let tmp = TempDir::new().unwrap();
        let global_path = tmp.path().join("soul.json");

        let soul = SoulManifest::default();
        fs::write(&global_path, serde_json::to_string(&soul).unwrap()).unwrap();

        let resolver = IdentityResolver::new(global_path);
        let sources = resolver.list_sources();

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_type, IdentitySourceType::Global);
    }

    #[test]
    fn test_list_sources_with_session() {
        let mut resolver = IdentityResolver::new(PathBuf::from("/nonexistent"));

        resolver.set_session_override(SoulManifest {
            identity: "Session".to_string(),
            ..Default::default()
        });

        let sources = resolver.list_sources();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_type, IdentitySourceType::Session);
        assert_eq!(sources[0].path, PathBuf::from("<session>"));
    }

    #[test]
    fn test_with_defaults() {
        let resolver = IdentityResolver::with_defaults();
        // Should point to ~/.aleph/soul.md
        let expected_suffix = std::path::Path::new(".aleph").join("soul.md");
        assert!(resolver.global_path().ends_with(&expected_suffix));
    }

    #[test]
    fn test_identity_source_serde() {
        let source = IdentitySource {
            source_type: IdentitySourceType::Project,
            path: PathBuf::from("/test/path"),
            loaded: true,
        };

        let json = serde_json::to_string(&source).unwrap();
        let parsed: IdentitySource = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.source_type, IdentitySourceType::Project);
        assert_eq!(parsed.path, PathBuf::from("/test/path"));
        assert!(parsed.loaded);
    }

    #[test]
    fn test_session_takes_priority_over_all() {
        let tmp = TempDir::new().unwrap();

        // Create global soul
        let global_path = tmp.path().join("soul.json");
        let global_soul = SoulManifest {
            identity: "Global".to_string(),
            ..Default::default()
        };
        fs::write(&global_path, serde_json::to_string(&global_soul).unwrap()).unwrap();

        // Create project identity
        let project_dir = tmp.path().join("projects").join("my-project");
        fs::create_dir_all(&project_dir).unwrap();
        let project_soul = SoulManifest {
            identity: "Project".to_string(),
            ..Default::default()
        };
        fs::write(
            project_dir.join("identity.json"),
            serde_json::to_string(&project_soul).unwrap(),
        )
        .unwrap();

        let mut resolver = IdentityResolver::new(global_path);
        resolver.projects_base_dir = Some(tmp.path().join("projects"));
        resolver.add_project("my-project");

        // Session override should take priority
        resolver.set_session_override(SoulManifest {
            identity: "Session".to_string(),
            ..Default::default()
        });

        let resolved = resolver.resolve();
        assert_eq!(resolved.identity, "Session");
    }

    #[test]
    fn test_add_project_stores_id() {
        let mut resolver = IdentityResolver::new(PathBuf::from("/nonexistent"));
        resolver.add_project("project-a");
        resolver.add_project("project-b");
        assert_eq!(resolver.project_ids(), &["project-a", "project-b"]);
    }
}
