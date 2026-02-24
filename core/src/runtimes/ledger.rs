//! Capability Ledger — lightweight state tracker for runtime capabilities
//!
//! The CapabilityLedger replaces the heavy RuntimeRegistry by tracking only
//! the *state* of each capability (Missing, Probing, Bootstrapping, Ready, Stale).
//! It never downloads or installs anything — that responsibility belongs to
//! the Prober and Bootstrapper (separate modules).
//!
//! Persistence format: JSON file at a user-specified path.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Lifecycle status of a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityStatus {
    /// Not known / not yet probed.
    Missing,
    /// A probe is in progress (version check running).
    Probing,
    /// Download / bootstrap is in progress.
    Bootstrapping,
    /// Executable is available and verified.
    Ready,
    /// Was Ready, but a periodic re-probe is needed (version drift, etc.).
    Stale,
}

/// How the capability was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilitySource {
    /// Found on the system PATH (user-installed).
    System,
    /// Installed and managed by Aleph under `~/.aleph/runtimes/`.
    AlephManaged,
}

/// A single capability entry in the ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityEntry {
    /// Unique name (e.g. "uv", "fnm", "ffmpeg", "yt-dlp").
    pub name: String,
    /// Absolute path to the executable binary.
    pub bin_path: PathBuf,
    /// Detected version string (may be empty during Probing).
    pub version: String,
    /// Current lifecycle status.
    pub status: CapabilityStatus,
    /// Where the binary came from.
    pub source: CapabilitySource,
    /// Unix timestamp (seconds) of the last successful probe.
    pub last_probed: u64,
}

impl CapabilityEntry {
    /// Convenience constructor for a capability that is immediately ready.
    pub fn new_ready(name: impl Into<String>, bin_path: impl Into<PathBuf>, source: CapabilitySource) -> Self {
        Self {
            name: name.into(),
            bin_path: bin_path.into(),
            version: String::new(),
            status: CapabilityStatus::Ready,
            source,
            last_probed: now_secs(),
        }
    }
}

// ---------------------------------------------------------------------------
// CapabilityLedger
// ---------------------------------------------------------------------------

/// Persisted ledger that maps capability names to their current state.
///
/// The ledger is the single source of truth for "what can Aleph use right now?"
/// It is intentionally simple — a `HashMap` plus a JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityLedger {
    /// Capability entries keyed by name.
    pub entries: HashMap<String, CapabilityEntry>,
    /// Path to the JSON persistence file.
    #[serde(skip)]
    persist_path: PathBuf,
}

impl CapabilityLedger {
    /// Create an empty ledger (in-memory only until `persist()` is called).
    pub fn new(persist_path: impl Into<PathBuf>) -> Self {
        Self {
            entries: HashMap::new(),
            persist_path: persist_path.into(),
        }
    }

    /// Load an existing ledger from disk, or create a fresh one.
    ///
    /// If the file is missing or contains invalid JSON, a fresh empty ledger
    /// is returned (corrupted data is never propagated).
    pub fn load_or_create(persist_path: impl Into<PathBuf>) -> Self {
        let persist_path = persist_path.into();

        if persist_path.exists() {
            match std::fs::read_to_string(&persist_path) {
                Ok(content) => match serde_json::from_str::<CapabilityLedger>(&content) {
                    Ok(mut ledger) => {
                        ledger.persist_path = persist_path;
                        debug!("Loaded capability ledger ({} entries)", ledger.entries.len());
                        return ledger;
                    }
                    Err(e) => {
                        warn!("Corrupted ledger JSON, creating fresh: {}", e);
                    }
                },
                Err(e) => {
                    warn!("Failed to read ledger file, creating fresh: {}", e);
                }
            }
        }

        Self::new(persist_path)
    }

    /// Return the status of a capability by name.
    ///
    /// Returns `CapabilityStatus::Missing` if the name is unknown.
    pub fn status(&self, name: &str) -> CapabilityStatus {
        self.entries
            .get(name)
            .map(|e| e.status)
            .unwrap_or(CapabilityStatus::Missing)
    }

    /// Return the executable path *only* if the capability is `Ready`.
    pub fn executable(&self, name: &str) -> Option<&Path> {
        self.entries.get(name).and_then(|e| {
            if e.status == CapabilityStatus::Ready {
                Some(e.bin_path.as_path())
            } else {
                None
            }
        })
    }

    /// Insert or update a capability entry.
    pub fn update(&mut self, entry: CapabilityEntry) {
        self.entries.insert(entry.name.clone(), entry);
    }

    /// Update only the status of an existing entry. No-op if unknown.
    pub fn update_status(&mut self, name: &str, status: CapabilityStatus) {
        if let Some(entry) = self.entries.get_mut(name) {
            entry.status = status;
        }
    }

    /// Build an enhanced PATH string with Ready entries' bin directories
    /// prepended to the system PATH.
    pub fn build_path(&self) -> String {
        let mut paths: Vec<PathBuf> = Vec::new();

        // Collect bin directories from Ready entries
        for entry in self.entries.values() {
            if entry.status == CapabilityStatus::Ready {
                if let Some(parent) = entry.bin_path.parent() {
                    // Avoid duplicates
                    if !paths.contains(&parent.to_path_buf()) {
                        paths.push(parent.to_path_buf());
                    }
                }
            }
        }

        // Append system PATH
        if let Ok(system_path) = std::env::var("PATH") {
            for p in std::env::split_paths(&system_path) {
                if !paths.contains(&p) {
                    paths.push(p);
                }
            }
        }

        std::env::join_paths(&paths)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// Return all entries that are currently `Ready`.
    pub fn list_ready(&self) -> Vec<&CapabilityEntry> {
        self.entries
            .values()
            .filter(|e| e.status == CapabilityStatus::Ready)
            .collect()
    }

    /// Persist the ledger to its JSON file.
    ///
    /// Creates parent directories if needed. Uses atomic write (write-to-temp
    /// then rename) to avoid corruption if the process crashes mid-write.
    pub fn persist(&self) -> std::io::Result<()> {
        if let Some(parent) = self.persist_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(self)
            .map_err(std::io::Error::other)?;

        // Atomic write: write to temp file then rename
        let tmp_path = self.persist_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, &self.persist_path)?;

        debug!("Persisted capability ledger to {:?}", self.persist_path);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------

/// Migrate from legacy manifest.json to ledger.json
///
/// If manifest.json exists but ledger.json doesn't, converts entries
/// to Stale status (need re-probe to verify paths still valid).
/// If ledger.json already exists, loads it directly.
pub fn migrate_from_legacy(runtimes_dir: &Path) -> std::io::Result<CapabilityLedger> {
    let legacy_path = runtimes_dir.join("manifest.json");
    let ledger_path = runtimes_dir.join("ledger.json");

    if legacy_path.exists() && !ledger_path.exists() {
        tracing::info!("Migrating from legacy manifest.json to ledger.json");
        let mut ledger = CapabilityLedger::new(ledger_path);

        if let Ok(content) = std::fs::read_to_string(&legacy_path) {
            // Parse legacy format: { "version": 1, "runtimes": { "id": { "version": "..." } } }
            if let Ok(legacy) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(runtimes) = legacy.get("runtimes").and_then(|r| r.as_object()) {
                    for (id, metadata) in runtimes {
                        let version = metadata
                            .get("version")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        ledger.update(CapabilityEntry {
                            name: id.clone(),
                            bin_path: PathBuf::new(), // Legacy didn't store paths
                            version,
                            status: CapabilityStatus::Stale, // Need re-probe
                            source: CapabilitySource::AlephManaged,
                            last_probed: 0,
                        });
                    }
                }
            }
        }

        ledger.persist()?;
        Ok(ledger)
    } else {
        Ok(CapabilityLedger::load_or_create(ledger_path))
    }
}

// ---------------------------------------------------------------------------
// Convenience functions
// ---------------------------------------------------------------------------

/// Build enhanced PATH from the persisted ledger on disk.
/// Convenience for callers that don't have a ledger instance in memory.
pub fn build_enhanced_path() -> std::io::Result<String> {
    let runtimes_dir = crate::runtimes::get_runtimes_dir()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let ledger_path = runtimes_dir.join("ledger.json");
    let ledger = CapabilityLedger::load_or_create(ledger_path);
    Ok(ledger.build_path())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Current time as Unix seconds (monotonic-safe fallback to 0).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests (TDD — written first)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -- helpers -----------------------------------------------------------

    fn tmp_ledger_path(dir: &TempDir) -> PathBuf {
        dir.path().join("ledger.json")
    }

    fn sample_entry(name: &str, status: CapabilityStatus) -> CapabilityEntry {
        CapabilityEntry {
            name: name.to_string(),
            bin_path: PathBuf::from(format!("/usr/local/bin/{}", name)),
            version: "1.0.0".to_string(),
            status,
            source: CapabilitySource::System,
            last_probed: 1_700_000_000,
        }
    }

    // -- tests -------------------------------------------------------------

    #[test]
    fn test_new_ledger_is_empty() {
        let dir = TempDir::new().unwrap();
        let ledger = CapabilityLedger::new(tmp_ledger_path(&dir));

        assert!(ledger.entries.is_empty());
        assert_eq!(ledger.status("uv"), CapabilityStatus::Missing);
        assert!(ledger.executable("uv").is_none());
        assert!(ledger.list_ready().is_empty());
    }

    #[test]
    fn test_update_and_query() {
        let dir = TempDir::new().unwrap();
        let mut ledger = CapabilityLedger::new(tmp_ledger_path(&dir));

        let entry = CapabilityEntry::new_ready("uv", "/home/user/.aleph/runtimes/uv/bin/uv", CapabilitySource::AlephManaged);
        ledger.update(entry);

        assert_eq!(ledger.status("uv"), CapabilityStatus::Ready);
        assert_eq!(
            ledger.executable("uv").unwrap(),
            Path::new("/home/user/.aleph/runtimes/uv/bin/uv")
        );
        assert_eq!(ledger.list_ready().len(), 1);
        assert_eq!(ledger.list_ready()[0].name, "uv");
    }

    #[test]
    fn test_persist_and_reload() {
        let dir = TempDir::new().unwrap();
        let path = tmp_ledger_path(&dir);

        // Create, populate, persist
        {
            let mut ledger = CapabilityLedger::new(path.clone());
            ledger.update(sample_entry("uv", CapabilityStatus::Ready));
            ledger.update(sample_entry("fnm", CapabilityStatus::Ready));
            ledger.persist().unwrap();
        }

        // Reload
        let ledger = CapabilityLedger::load_or_create(path);
        assert_eq!(ledger.entries.len(), 2);
        assert_eq!(ledger.status("uv"), CapabilityStatus::Ready);
        assert_eq!(ledger.status("fnm"), CapabilityStatus::Ready);
    }

    #[test]
    fn test_update_status() {
        let dir = TempDir::new().unwrap();
        let mut ledger = CapabilityLedger::new(tmp_ledger_path(&dir));

        ledger.update(sample_entry("ffmpeg", CapabilityStatus::Ready));
        assert_eq!(ledger.status("ffmpeg"), CapabilityStatus::Ready);

        ledger.update_status("ffmpeg", CapabilityStatus::Stale);
        assert_eq!(ledger.status("ffmpeg"), CapabilityStatus::Stale);

        // Updating unknown name is a no-op
        ledger.update_status("ghost", CapabilityStatus::Probing);
        assert_eq!(ledger.status("ghost"), CapabilityStatus::Missing);
    }

    #[test]
    fn test_build_path_includes_ready_entries() {
        let dir = TempDir::new().unwrap();
        let mut ledger = CapabilityLedger::new(tmp_ledger_path(&dir));

        ledger.update(CapabilityEntry {
            name: "uv".to_string(),
            bin_path: PathBuf::from("/custom/runtimes/uv/bin/uv"),
            version: "0.5.0".to_string(),
            status: CapabilityStatus::Ready,
            source: CapabilitySource::AlephManaged,
            last_probed: 0,
        });

        let path = ledger.build_path();

        // The parent directory of the bin_path must appear in the PATH
        assert!(
            path.contains("/custom/runtimes/uv/bin"),
            "PATH should contain the Ready entry's bin dir, got: {}",
            path
        );
    }

    #[test]
    fn test_stale_entry_not_in_executable() {
        let dir = TempDir::new().unwrap();
        let mut ledger = CapabilityLedger::new(tmp_ledger_path(&dir));

        ledger.update(sample_entry("yt-dlp", CapabilityStatus::Stale));

        // Stale entries must NOT be returned by executable()
        assert!(ledger.executable("yt-dlp").is_none());

        // But the entry still exists
        assert_eq!(ledger.status("yt-dlp"), CapabilityStatus::Stale);
    }

    #[test]
    fn test_corrupted_json_creates_fresh_ledger() {
        let dir = TempDir::new().unwrap();
        let path = tmp_ledger_path(&dir);

        // Write garbage
        std::fs::write(&path, "{{{{not valid json!!!!").unwrap();

        let ledger = CapabilityLedger::load_or_create(path);
        assert!(
            ledger.entries.is_empty(),
            "Corrupted JSON should yield an empty ledger"
        );
    }

    #[test]
    fn test_migrate_from_legacy_manifest() {
        let dir = TempDir::new().unwrap();
        let runtimes_dir = dir.path();

        // Write a legacy manifest.json
        let legacy = serde_json::json!({
            "version": 1,
            "runtimes": {
                "uv": {
                    "installed_at": { "secs_since_epoch": 1700000000, "nanos_since_epoch": 0 },
                    "version": "0.5.14",
                    "last_update_check": null,
                    "extra": {}
                },
                "ffmpeg": {
                    "installed_at": { "secs_since_epoch": 1700000000, "nanos_since_epoch": 0 },
                    "version": "6.1",
                    "last_update_check": null,
                    "extra": {}
                }
            }
        });
        std::fs::write(
            runtimes_dir.join("manifest.json"),
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let ledger = migrate_from_legacy(runtimes_dir).unwrap();

        // Migrated entries should be Stale (need re-probe)
        assert_eq!(ledger.status("uv"), CapabilityStatus::Stale);
        assert_eq!(ledger.status("ffmpeg"), CapabilityStatus::Stale);
        // ledger.json should exist
        assert!(runtimes_dir.join("ledger.json").exists());
    }

    #[test]
    fn test_migrate_skips_if_ledger_exists() {
        let dir = TempDir::new().unwrap();
        let runtimes_dir = dir.path();

        // Write both files
        std::fs::write(runtimes_dir.join("manifest.json"), "{}").unwrap();
        std::fs::write(runtimes_dir.join("ledger.json"), "{}").unwrap();

        // Should load ledger, not migrate
        let ledger = migrate_from_legacy(runtimes_dir).unwrap();
        assert!(ledger.list_ready().is_empty());
    }
}
