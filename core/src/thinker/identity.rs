//! Identity Resolver - Layered identity resolution for AI embodiment
//!
//! This module provides the IdentityResolver that resolves identity from
//! multiple sources with configurable priority.
//!
//! # Priority Order (highest to lowest)
//!
//! 1. Session override - Runtime identity set programmatically
//! 2. Project identity - `.soul/identity.md` or `.aleph/identity.md`
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
//! │  │   │  Project    │ ← .soul/identity.md                │    │
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

/// Resolves identity from layered sources
///
/// Priority (highest to lowest):
/// 1. Session override
/// 2. Project identity (.soul/identity.md or .aleph/identity.md)
/// 3. Global soul (~/.aleph/soul.md)
/// 4. Default (empty SoulManifest)
pub struct IdentityResolver {
    /// Global soul path (~/.aleph/soul.md)
    global_path: PathBuf,
    /// Project roots to search for .soul/identity.md
    project_roots: Vec<PathBuf>,
    /// Current session override
    session_override: Option<SoulManifest>,
}

impl IdentityResolver {
    /// Create a new resolver with global path
    pub fn new(global_path: PathBuf) -> Self {
        Self {
            global_path,
            project_roots: Vec::new(),
            session_override: None,
        }
    }

    /// Create resolver with default paths
    pub fn with_defaults() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self::new(home.join(".aleph").join("soul.md"))
    }

    /// Add a project root to search
    pub fn add_project_root(&mut self, path: PathBuf) {
        self.project_roots.push(path);
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

    /// Get the project roots
    pub fn project_roots(&self) -> &[PathBuf] {
        &self.project_roots
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

    /// Load soul from project directory
    fn load_project_soul(&self) -> Option<SoulManifest> {
        for root in &self.project_roots {
            // Check multiple possible paths
            let paths = [
                root.join(".soul").join("identity.md"),
                root.join(".soul").join("identity.json"),
                root.join(".soul").join("identity.toml"),
                root.join(".aleph").join("identity.md"),
                root.join(".aleph").join("identity.json"),
                root.join(".aleph").join("identity.toml"),
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
        for root in &self.project_roots {
            let paths = [
                root.join(".soul").join("identity.md"),
                root.join(".soul").join("identity.json"),
                root.join(".soul").join("identity.toml"),
                root.join(".aleph").join("identity.md"),
                root.join(".aleph").join("identity.json"),
                root.join(".aleph").join("identity.toml"),
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

        // Create project soul
        let project_root = tmp.path().join("project");
        fs::create_dir_all(project_root.join(".soul")).unwrap();
        let project_soul = SoulManifest {
            identity: "Project identity".to_string(),
            ..Default::default()
        };
        fs::write(
            project_root.join(".soul").join("identity.json"),
            serde_json::to_string(&project_soul).unwrap(),
        )
        .unwrap();

        let mut resolver = IdentityResolver::new(global_path);
        resolver.add_project_root(project_root);

        let resolved = resolver.resolve();
        // Project identity overrides global
        assert_eq!(resolved.identity, "Project identity");
        // But inherits directives from global
        assert_eq!(resolved.directives, vec!["Be helpful".to_string()]);
    }

    #[test]
    fn test_aleph_directory_works() {
        let tmp = TempDir::new().unwrap();

        // Create project soul in .aleph directory
        let project_root = tmp.path().join("project");
        fs::create_dir_all(project_root.join(".aleph")).unwrap();
        let project_soul = SoulManifest {
            identity: "Aleph project identity".to_string(),
            ..Default::default()
        };
        fs::write(
            project_root.join(".aleph").join("identity.json"),
            serde_json::to_string(&project_soul).unwrap(),
        )
        .unwrap();

        let mut resolver = IdentityResolver::new(PathBuf::from("/nonexistent"));
        resolver.add_project_root(project_root);

        let resolved = resolver.resolve();
        assert_eq!(resolved.identity, "Aleph project identity");
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

        // Create project soul
        let project_root = tmp.path().join("project");
        fs::create_dir_all(project_root.join(".soul")).unwrap();
        let project_soul = SoulManifest {
            identity: "Project".to_string(),
            ..Default::default()
        };
        fs::write(
            project_root.join(".soul").join("identity.json"),
            serde_json::to_string(&project_soul).unwrap(),
        )
        .unwrap();

        let mut resolver = IdentityResolver::new(global_path);
        resolver.add_project_root(project_root);

        // Session override should take priority
        resolver.set_session_override(SoulManifest {
            identity: "Session".to_string(),
            ..Default::default()
        });

        let resolved = resolver.resolve();
        assert_eq!(resolved.identity, "Session");
    }
}
