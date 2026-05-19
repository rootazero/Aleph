//! Per-skill usage tracking — a `.usage.json` sidecar in the skills dir.
//!
//! Best-effort: every record/load failure degrades to a warn log and never
//! propagates. Mirrors hermes-agent's `.usage.json` sidecar pattern.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageStats {
    #[serde(default)]
    pub use_count: u64,
    #[serde(default)]
    pub view_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_viewed_at: Option<String>,
}

/// Tracks skill usage in `<dir>/.usage.json`.
pub struct UsageStore {
    path: PathBuf,
}

impl UsageStore {
    /// Create a store backed by `<skills_dir>/.usage.json`.
    pub fn new(skills_dir: impl AsRef<Path>) -> Self {
        Self {
            path: skills_dir.as_ref().join(".usage.json"),
        }
    }

    fn load_map(&self) -> HashMap<String, UsageStats> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    fn save_map(&self, map: &HashMap<String, UsageStats>) {
        match serde_json::to_vec_pretty(map) {
            Ok(bytes) => {
                if let Err(e) = crate::utils::atomic_io::write_atomic(&self.path, &bytes) {
                    tracing::warn!(error = %e, "skill usage: atomic write failed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "skill usage: serialize failed"),
        }
    }

    /// Read stats for one skill, if any.
    pub fn get(&self, skill: &str) -> Option<UsageStats> {
        self.load_map().get(skill).cloned()
    }

    /// Increment the view counter for `skill`. Best-effort.
    pub fn record_view(&self, skill: &str) {
        let mut map = self.load_map();
        let entry = map.entry(skill.to_string()).or_default();
        entry.view_count += 1;
        entry.last_viewed_at = Some(chrono::Utc::now().to_rfc3339());
        self.save_map(&map);
    }

    /// Increment the use counter for `skill`. Best-effort.
    pub fn record_use(&self, skill: &str) {
        let mut map = self.load_map();
        let entry = map.entry(skill.to_string()).or_default();
        entry.use_count += 1;
        entry.last_used_at = Some(chrono::Utc::now().to_rfc3339());
        self.save_map(&map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_and_reload_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = UsageStore::new(tmp.path());
        store.record_view("git");
        store.record_view("git");
        store.record_use("git");
        let reloaded = UsageStore::new(tmp.path());
        let stats = reloaded.get("git").unwrap();
        assert_eq!(stats.view_count, 2);
        assert_eq!(stats.use_count, 1);
    }

    #[test]
    fn unknown_skill_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let store = UsageStore::new(tmp.path());
        assert!(store.get("never").is_none());
    }
}
