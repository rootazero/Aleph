//! Skill Graveyard — archive for retired/demoted skills.
//!
//! Failed patterns become negative constraints for future skill generation,
//! enabling the system to "learn from failure" rather than repeating mistakes.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info, warn};

/// A single entry in the skill graveyard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraveyardEntry {
    pub skill_id: String,
    pub skill_md: String,
    pub failure_traces: Vec<String>,
    pub reason: String,
    pub retired_at: i64,
    pub vitality_at_death: f32,
}

/// Maximum entries before FIFO eviction.
const MAX_ENTRIES: usize = 100;

/// Manages the skill graveyard — a FIFO archive of failed/retired skills.
pub struct SkillGraveyard {
    entries: Vec<GraveyardEntry>,
    storage_path: PathBuf,
}

impl SkillGraveyard {
    /// Create or load a graveyard at the given directory.
    pub async fn open(graveyard_dir: &Path) -> anyhow::Result<Self> {
        let storage_path = graveyard_dir.join("graveyard.json");

        let entries = if storage_path.exists() {
            let data = fs::read_to_string(&storage_path).await?;
            serde_json::from_str(&data).unwrap_or_else(|e| {
                warn!("Failed to parse graveyard.json: {}, starting fresh", e);
                Vec::new()
            })
        } else {
            Vec::new()
        };

        Ok(Self {
            entries,
            storage_path,
        })
    }

    /// Create an in-memory graveyard (for testing).
    pub fn in_memory() -> Self {
        Self {
            entries: Vec::new(),
            storage_path: PathBuf::from("/dev/null"),
        }
    }

    /// Archive a retired skill. FIFO eviction when full.
    pub async fn archive(&mut self, entry: GraveyardEntry) -> anyhow::Result<()> {
        info!(skill_id = %entry.skill_id, reason = %entry.reason, "Archiving skill to graveyard");

        // FIFO eviction
        while self.entries.len() >= MAX_ENTRIES {
            let evicted = self.entries.remove(0);
            debug!(skill_id = %evicted.skill_id, "Evicted oldest graveyard entry");
        }

        self.entries.push(entry);
        self.persist().await
    }

    /// Get entries similar to a description (simple keyword overlap for now).
    /// Returns entries whose skill_md contains any of the given keywords.
    pub fn query_similar(&self, keywords: &[&str]) -> Vec<&GraveyardEntry> {
        self.entries
            .iter()
            .filter(|e| {
                let lower = e.skill_md.to_lowercase();
                keywords.iter().any(|kw| lower.contains(&kw.to_lowercase()))
            })
            .collect()
    }

    /// Get all entries (for inspection/testing).
    pub fn entries(&self) -> &[GraveyardEntry] {
        &self.entries
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the graveyard is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    async fn persist(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let json = serde_json::to_string_pretty(&self.entries)?;
        fs::write(&self.storage_path, json).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str, reason: &str) -> GraveyardEntry {
        GraveyardEntry {
            skill_id: id.to_string(),
            skill_md: format!("# Skill {}\nDoes something with files and code.", id),
            failure_traces: vec!["trace-1".to_string()],
            reason: reason.to_string(),
            retired_at: 1000,
            vitality_at_death: 0.05,
        }
    }

    #[tokio::test]
    async fn archive_and_query() {
        let mut graveyard = SkillGraveyard::in_memory();
        assert!(graveyard.is_empty());

        graveyard.archive(make_entry("skill-1", "too slow")).await.unwrap();
        graveyard.archive(make_entry("skill-2", "wrong output")).await.unwrap();

        assert_eq!(graveyard.len(), 2);

        let similar = graveyard.query_similar(&["files"]);
        assert_eq!(similar.len(), 2);

        let similar = graveyard.query_similar(&["nonexistent"]);
        assert!(similar.is_empty());
    }

    #[tokio::test]
    async fn fifo_eviction() {
        let mut graveyard = SkillGraveyard::in_memory();

        for i in 0..105 {
            graveyard
                .archive(make_entry(&format!("skill-{}", i), "eviction test"))
                .await
                .unwrap();
        }

        assert_eq!(graveyard.len(), 100);
        // Oldest entries (0-4) should be evicted
        assert_eq!(graveyard.entries()[0].skill_id, "skill-5");
    }

    #[tokio::test]
    async fn persistence_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let graveyard_dir = tmp.path().join(".graveyard");

        {
            let mut gy = SkillGraveyard::open(&graveyard_dir).await.unwrap();
            gy.archive(make_entry("s1", "test")).await.unwrap();
            assert_eq!(gy.len(), 1);
        }

        // Re-open and verify
        let gy = SkillGraveyard::open(&graveyard_dir).await.unwrap();
        assert_eq!(gy.len(), 1);
        assert_eq!(gy.entries()[0].skill_id, "s1");
    }
}
