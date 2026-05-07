//! Skills manifest — tracks installed skills, their sources, and bundled version.
//!
//! Location: `~/.aleph/skills/manifest.json`

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Version of the last successfully extracted bundled content.
    pub bundled_version: String,
    /// Per-skill metadata keyed by skill directory name.
    pub skills: BTreeMap<String, SkillEntry>,
}

/// Where a skill was installed from.
///
/// Named `SkillOrigin` to avoid collision with `domain::skill::SkillSource`
/// and `skills::registry::SkillSource`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SkillOrigin {
    /// Bundled with the binary, extracted on startup.
    Official,
    /// Installed from a GitHub URL.
    Github,
    /// Manually placed in the skills directory.
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    /// Where this skill came from.
    pub source: SkillOrigin,
    /// Version when installed (for official skills, matches bundled_version).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Source URL (for github-installed skills).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// ISO date when installed (for non-official skills).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
}

impl SkillManifest {
    /// Load manifest from disk. Returns None if file doesn't exist or is corrupt.
    pub fn load(skills_dir: &Path) -> Option<Self> {
        let path = skills_dir.join("manifest.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(manifest) => Some(manifest),
                Err(e) => {
                    warn!(error = %e, path = %path.display(), "Corrupt manifest.json, will recreate");
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                warn!(error = %e, path = %path.display(), "Failed to read manifest.json");
                None
            }
        }
    }

    /// Save manifest to disk.
    pub fn save(&self, skills_dir: &Path) -> std::io::Result<()> {
        let path = skills_dir.join("manifest.json");
        let content = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&path, content)?;
        debug!(path = %path.display(), "Saved skills manifest");
        Ok(())
    }

    /// Create an empty manifest with the given version.
    pub fn new(version: &str) -> Self {
        Self {
            bundled_version: version.to_string(),
            skills: BTreeMap::new(),
        }
    }

    /// Reconcile manifest with actual directory contents.
    /// - Directories not in manifest → add as Local
    /// - Manifest entries without directories → remove
    ///
    /// Returns `Err` if the skills directory could not be read (caller should
    /// avoid saving a potentially stale manifest).
    pub fn reconcile(&mut self, skills_dir: &Path) -> std::io::Result<()> {
        // Find directories on disk
        let entries = std::fs::read_dir(skills_dir)?;
        let on_disk: HashSet<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                // Use symlink_metadata so we don't follow symlinks — a symlink
                // pointing outside the skills dir should not be treated as a skill.
                e.path()
                    .symlink_metadata()
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
            })
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        // Add missing directories as Local
        for name in &on_disk {
            if !self.skills.contains_key(name) {
                debug!(skill = %name, "Discovered untracked skill, marking as local");
                self.skills.insert(
                    name.clone(),
                    SkillEntry {
                        source: SkillOrigin::Local,
                        version: None,
                        url: None,
                        installed_at: None,
                    },
                );
            }
        }

        // Remove manifest entries for deleted directories
        self.skills.retain(|name, _| on_disk.contains(name));
        Ok(())
    }

    /// Check if a skill is official.
    pub fn is_official(&self, name: &str) -> bool {
        self.skills
            .get(name)
            .map(|e| e.source == SkillOrigin::Official)
            .unwrap_or(false)
    }
}
