# Runtime On-Demand Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace ~2500 LOC of heavy Rust runtime managers with a lightweight Capability Ledger + Shell-driven bootstrap protocol, enabling zero-overhead startup and on-demand runtime provisioning.

**Architecture:** New `CapabilityLedger` (JSON-backed state tracking) replaces `RuntimeRegistry`. System probing discovers existing tools. Shell scripts handle installation. The Ledger integrates with exec (PATH injection) and prompt (capability awareness). Heavy download/extract code is deleted.

**Tech Stack:** Rust (serde_json, tokio, regex), Shell scripts (embedded `&'static str`)

**Design doc:** `docs/plans/2026-02-24-runtime-on-demand-design.md`

---

## Phase 1: Decouple — Capability Ledger + Init Reform

### Task 1: Create CapabilityLedger data structures

**Files:**
- Create: `core/src/runtimes/ledger.rs`
- Modify: `core/src/runtimes/mod.rs` (add `mod ledger` and re-export)

**Step 1: Write the failing test**

Create `core/src/runtimes/ledger.rs` with test module first:

```rust
//! Capability Ledger — lightweight runtime state tracking
//!
//! Replaces the heavy RuntimeRegistry with a minimal JSON-backed ledger
//! that only tracks "who is where" and "can it be used", not "how to install".

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::Result;

/// Capability status state machine: Missing → Probing → Bootstrapping → Ready → Stale
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CapabilityStatus {
    /// Never probed, status unknown
    Missing,
    /// Currently probing system PATH
    Probing,
    /// Shell bootstrap in progress
    Bootstrapping,
    /// Available, path verified
    Ready,
    /// Was available but path no longer valid
    Stale,
}

/// Where this capability came from
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CapabilitySource {
    /// Found on system PATH (e.g., /usr/local/bin/ffmpeg)
    System,
    /// Installed by Aleph to managed directory (e.g., ~/.aleph/runtimes/)
    AlephManaged,
}

/// A single capability record
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityEntry {
    pub name: String,
    pub bin_path: Option<PathBuf>,
    pub version: Option<String>,
    pub status: CapabilityStatus,
    pub source: CapabilitySource,
    pub last_probed: Option<SystemTime>,
}

/// The Capability Ledger — tracks runtime state, persists to JSON
pub struct CapabilityLedger {
    entries: HashMap<String, CapabilityEntry>,
    persist_path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_new_ledger_is_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ledger.json");
        let ledger = CapabilityLedger::load_or_create(path).unwrap();
        assert_eq!(ledger.status("python"), CapabilityStatus::Missing);
        assert!(ledger.executable("python").is_none());
        assert!(ledger.list_ready().is_empty());
    }

    #[test]
    fn test_update_and_query() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ledger.json");
        let mut ledger = CapabilityLedger::load_or_create(path).unwrap();

        ledger.update(CapabilityEntry {
            name: "python".into(),
            bin_path: Some(PathBuf::from("/usr/bin/python3")),
            version: Some("3.12.0".into()),
            status: CapabilityStatus::Ready,
            source: CapabilitySource::System,
            last_probed: Some(SystemTime::now()),
        }).unwrap();

        assert_eq!(ledger.status("python"), CapabilityStatus::Ready);
        assert_eq!(ledger.executable("python"), Some(Path::new("/usr/bin/python3")));
        assert_eq!(ledger.list_ready().len(), 1);
    }

    #[test]
    fn test_persist_and_reload() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ledger.json");

        {
            let mut ledger = CapabilityLedger::load_or_create(path.clone()).unwrap();
            ledger.update(CapabilityEntry {
                name: "ffmpeg".into(),
                bin_path: Some(PathBuf::from("/opt/homebrew/bin/ffmpeg")),
                version: Some("6.1".into()),
                status: CapabilityStatus::Ready,
                source: CapabilitySource::System,
                last_probed: Some(SystemTime::now()),
            }).unwrap();
            ledger.persist().unwrap();
        }

        let reloaded = CapabilityLedger::load_or_create(path).unwrap();
        assert_eq!(reloaded.status("ffmpeg"), CapabilityStatus::Ready);
    }

    #[test]
    fn test_update_status() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ledger.json");
        let mut ledger = CapabilityLedger::load_or_create(path).unwrap();

        ledger.update(CapabilityEntry {
            name: "uv".into(),
            bin_path: None,
            version: None,
            status: CapabilityStatus::Missing,
            source: CapabilitySource::AlephManaged,
            last_probed: None,
        }).unwrap();

        ledger.update_status("uv", CapabilityStatus::Probing).unwrap();
        assert_eq!(ledger.status("uv"), CapabilityStatus::Probing);
    }

    #[test]
    fn test_build_path_includes_ready_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ledger.json");
        let mut ledger = CapabilityLedger::load_or_create(path).unwrap();

        ledger.update(CapabilityEntry {
            name: "python".into(),
            bin_path: Some(PathBuf::from("/home/user/.aleph/runtimes/python/bin/python3")),
            version: Some("3.12.0".into()),
            status: CapabilityStatus::Ready,
            source: CapabilitySource::AlephManaged,
            last_probed: Some(SystemTime::now()),
        }).unwrap();

        let built_path = ledger.build_path();
        assert!(built_path.contains("/home/user/.aleph/runtimes/python/bin"));
    }

    #[test]
    fn test_stale_entry_not_in_executable() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ledger.json");
        let mut ledger = CapabilityLedger::load_or_create(path).unwrap();

        ledger.update(CapabilityEntry {
            name: "node".into(),
            bin_path: Some(PathBuf::from("/usr/bin/node")),
            version: Some("18.0.0".into()),
            status: CapabilityStatus::Stale,
            source: CapabilitySource::System,
            last_probed: None,
        }).unwrap();

        assert!(ledger.executable("node").is_none());
    }

    #[test]
    fn test_corrupted_json_creates_fresh_ledger() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ledger.json");
        std::fs::write(&path, "not valid json{{{").unwrap();

        let ledger = CapabilityLedger::load_or_create(path).unwrap();
        assert!(ledger.list_ready().is_empty());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test runtimes::ledger --no-default-features -- --test-threads=1 2>&1 | head -30`
Expected: FAIL — `CapabilityLedger` methods not implemented yet

**Step 3: Implement CapabilityLedger**

Add the implementation above the tests in the same file:

```rust
impl CapabilityEntry {
    /// Create a ready entry from a successful probe or bootstrap
    pub fn new_ready(name: &str, bin_path: PathBuf, source: CapabilitySource) -> Self {
        Self {
            name: name.to_string(),
            bin_path: Some(bin_path),
            version: None,
            status: CapabilityStatus::Ready,
            source,
            last_probed: Some(SystemTime::now()),
        }
    }
}

impl CapabilityLedger {
    /// Create a new empty ledger
    pub fn new(persist_path: PathBuf) -> Self {
        Self {
            entries: HashMap::new(),
            persist_path,
        }
    }

    /// Load from disk or create fresh. Corrupted JSON creates a fresh ledger.
    pub fn load_or_create(persist_path: PathBuf) -> Result<Self> {
        if persist_path.exists() {
            match std::fs::read_to_string(&persist_path) {
                Ok(content) => {
                    match serde_json::from_str::<HashMap<String, CapabilityEntry>>(&content) {
                        Ok(entries) => Ok(Self { entries, persist_path }),
                        Err(e) => {
                            tracing::warn!("Corrupted ledger.json, creating fresh: {}", e);
                            Ok(Self::new(persist_path))
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to read ledger.json, creating fresh: {}", e);
                    Ok(Self::new(persist_path))
                }
            }
        } else {
            Ok(Self::new(persist_path))
        }
    }

    /// Query capability status (returns Missing if unknown)
    pub fn status(&self, name: &str) -> CapabilityStatus {
        self.entries
            .get(name)
            .map(|e| e.status.clone())
            .unwrap_or(CapabilityStatus::Missing)
    }

    /// Get executable path (only if Ready)
    pub fn executable(&self, name: &str) -> Option<&Path> {
        self.entries.get(name).and_then(|e| {
            if e.status == CapabilityStatus::Ready {
                e.bin_path.as_deref()
            } else {
                None
            }
        })
    }

    /// Update a capability entry
    pub fn update(&mut self, entry: CapabilityEntry) -> Result<()> {
        self.entries.insert(entry.name.clone(), entry);
        Ok(())
    }

    /// Update only the status of an existing entry
    pub fn update_status(&mut self, name: &str, status: CapabilityStatus) -> Result<()> {
        if let Some(entry) = self.entries.get_mut(name) {
            entry.status = status;
        } else {
            self.entries.insert(name.to_string(), CapabilityEntry {
                name: name.to_string(),
                bin_path: None,
                version: None,
                status,
                source: CapabilitySource::AlephManaged,
                last_probed: None,
            });
        }
        Ok(())
    }

    /// Build enhanced PATH string with Ready capabilities' bin dirs prepended
    pub fn build_path(&self) -> String {
        let mut paths: Vec<PathBuf> = Vec::new();

        // Add bin directories for all Ready capabilities
        for entry in self.entries.values() {
            if entry.status == CapabilityStatus::Ready {
                if let Some(ref bin_path) = entry.bin_path {
                    if let Some(parent) = bin_path.parent() {
                        if !paths.contains(&parent.to_path_buf()) {
                            paths.push(parent.to_path_buf());
                        }
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

    /// List all Ready capabilities
    pub fn list_ready(&self) -> Vec<&CapabilityEntry> {
        self.entries
            .values()
            .filter(|e| e.status == CapabilityStatus::Ready)
            .collect()
    }

    /// Persist ledger to disk
    pub fn persist(&self) -> Result<()> {
        if let Some(parent) = self.persist_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.entries)?;
        std::fs::write(&self.persist_path, json)?;
        Ok(())
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test runtimes::ledger --no-default-features -- --test-threads=1`
Expected: All 7 tests PASS

**Step 5: Wire into mod.rs**

Modify `core/src/runtimes/mod.rs`:
- Add `pub mod ledger;` to the module declarations (around line 30)
- Add re-export: `pub use ledger::{CapabilityLedger, CapabilityEntry, CapabilityStatus, CapabilitySource};` (around line 46)

**Step 6: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check 2>&1 | tail -5`
Expected: Compiles without errors

**Step 7: Commit**

```bash
git add core/src/runtimes/ledger.rs core/src/runtimes/mod.rs
git commit -m "feat(runtimes): add CapabilityLedger with state machine and JSON persistence"
```

---

### Task 2: Add legacy manifest migration

**Files:**
- Modify: `core/src/runtimes/ledger.rs` (add migration function + test)
- Reference: `core/src/runtimes/manifest.rs` (read LegacyManifest format, lines 58-67)

**Step 1: Write the failing test**

Add to the `tests` module in `ledger.rs`:

```rust
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
    ).unwrap();

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
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test runtimes::ledger::tests::test_migrate --no-default-features 2>&1 | head -20`
Expected: FAIL — `migrate_from_legacy` not defined

**Step 3: Implement migration**

Add to `ledger.rs` (before `#[cfg(test)]`):

```rust
use crate::runtimes::manifest::Manifest;

/// Migrate from legacy manifest.json to ledger.json
///
/// If manifest.json exists but ledger.json doesn't, converts entries
/// to Stale status (need re-probe to verify paths still valid).
/// If ledger.json already exists, loads it directly.
pub fn migrate_from_legacy(runtimes_dir: &Path) -> Result<CapabilityLedger> {
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
                            .map(|s| s.to_string());

                        ledger.update(CapabilityEntry {
                            name: id.clone(),
                            bin_path: None, // Legacy didn't store paths, need re-probe
                            version,
                            status: CapabilityStatus::Stale,
                            source: CapabilitySource::AlephManaged,
                            last_probed: None,
                        })?;
                    }
                }
            }
        }

        ledger.persist()?;
        Ok(ledger)
    } else {
        CapabilityLedger::load_or_create(ledger_path)
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test runtimes::ledger --no-default-features -- --test-threads=1`
Expected: All 9 tests PASS

**Step 5: Commit**

```bash
git add core/src/runtimes/ledger.rs
git commit -m "feat(runtimes): add legacy manifest.json to ledger.json migration"
```

---

### Task 3: Reform init — remove forced runtime installation

**Files:**
- Modify: `core/src/init_unified/coordinator.rs` (lines 335-441: replace runtimes phase)
- Modify: `core/src/init_unified/mod.rs` (line 28: update init check)

**Step 1: Modify coordinator.rs — replace install_runtimes()**

In `core/src/init_unified/coordinator.rs`, replace the `install_runtimes` method (lines 335-441) with:

```rust
async fn install_runtimes(&self) -> Result<(), InitError> {
    use crate::runtimes::ledger::{migrate_from_legacy, CapabilityLedger};

    info!("Initializing runtime ledger (zero-install)...");

    let runtimes_dir = crate::utils::paths::get_runtimes_dir()
        .map_err(|e| InitError::new("runtimes", format!("Failed to get runtimes dir: {}", e)))?;

    // Create directory if needed
    if !runtimes_dir.exists() {
        std::fs::create_dir_all(&runtimes_dir)
            .map_err(|e| InitError::new("runtimes", format!("Failed to create runtimes dir: {}", e)))?;
    }

    // Migrate from legacy manifest.json or create fresh ledger
    let _ledger = migrate_from_legacy(&runtimes_dir)
        .map_err(|e| InitError::new("runtimes", format!("Failed to initialize ledger: {}", e)))?;

    info!("Runtime ledger initialized (no downloads, runtimes provisioned on-demand)");
    Ok(())
}
```

Also remove the `use crate::runtimes::RuntimeRegistry;` import at line 340 (it was inside the function).

**Step 2: Modify init check in mod.rs**

In `core/src/init_unified/mod.rs`, change line 28 from:
```rust
let has_manifest = config_dir.join("runtimes").join("manifest.json").exists();
```
to:
```rust
let has_manifest = config_dir.join("runtimes").join("manifest.json").exists()
    || config_dir.join("runtimes").join("ledger.json").exists();
```

**Step 3: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check 2>&1 | tail -10`
Expected: Compiles (existing runtimes code still present but no longer called from init)

**Step 4: Run existing tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test init_unified 2>&1 | tail -10`
Expected: PASS (or no tests — in either case, compilation success is the key check)

**Step 5: Commit**

```bash
git add core/src/init_unified/coordinator.rs core/src/init_unified/mod.rs
git commit -m "refactor(init): replace forced runtime installation with zero-install ledger"
```

---

## Phase 2: Integration — Probe + Exec PATH + Prompt Wiring

### Task 4: Create Probe module for system-first detection

**Files:**
- Create: `core/src/runtimes/probe.rs`
- Modify: `core/src/runtimes/mod.rs` (add `pub mod probe;`)

**Step 1: Write the failing test**

Create `core/src/runtimes/probe.rs`:

```rust
//! System-first capability probing
//!
//! Probes for tools in this priority order:
//! 1. Aleph managed directory (~/.aleph/runtimes/)
//! 2. System PATH (via `which`)
//! 3. Known platform paths (/usr/local/bin, /opt/homebrew/bin)

use crate::error::Result;
use crate::runtimes::ledger::CapabilitySource;
use regex::Regex;
use std::path::PathBuf;
use std::process::Command;

/// Result of probing for a capability
#[derive(Debug)]
pub struct ProbeResult {
    pub found: bool,
    pub bin_path: Option<PathBuf>,
    pub version: Option<String>,
    pub source: CapabilitySource,
    /// Warning if version is below minimum
    pub version_warning: Option<String>,
}

/// Probe specification for a capability
struct ProbeSpec {
    capability: &'static str,
    binaries: &'static [&'static str],
    version_flag: &'static str,
    version_regex: &'static str,
    min_version: Option<&'static str>,
    aleph_paths: &'static [&'static str], // Relative to ~/.aleph/runtimes/
}

const PROBE_SPECS: &[ProbeSpec] = &[
    ProbeSpec {
        capability: "python",
        binaries: &["python3", "python"],
        version_flag: "--version",
        version_regex: r"Python (\d+\.\d+\.\d+)",
        min_version: Some("3.10"),
        aleph_paths: &["python/default/bin/python3", "uv/envs/default/bin/python3"],
    },
    ProbeSpec {
        capability: "node",
        binaries: &["node"],
        version_flag: "--version",
        version_regex: r"v(\d+\.\d+\.\d+)",
        min_version: Some("18.0"),
        aleph_paths: &["fnm/versions/default/bin/node"],
    },
    ProbeSpec {
        capability: "ffmpeg",
        binaries: &["ffmpeg"],
        version_flag: "-version",
        version_regex: r"ffmpeg version (\S+)",
        min_version: None,
        aleph_paths: &["ffmpeg/ffmpeg"],
    },
    ProbeSpec {
        capability: "uv",
        binaries: &["uv"],
        version_flag: "--version",
        version_regex: r"uv (\d+\.\d+\.\d+)",
        min_version: None,
        aleph_paths: &["uv/uv"],
    },
    ProbeSpec {
        capability: "yt-dlp",
        binaries: &["yt-dlp"],
        version_flag: "--version",
        version_regex: r"(\d{4}\.\d+\.\d+)",
        min_version: None,
        aleph_paths: &["yt-dlp"],
    },
];

/// Probe for a capability across all known sources
pub fn probe(name: &str) -> ProbeResult {
    let spec = match PROBE_SPECS.iter().find(|s| s.capability == name) {
        Some(s) => s,
        None => {
            tracing::warn!("No probe spec for capability: {}", name);
            return ProbeResult {
                found: false,
                bin_path: None,
                version: None,
                source: CapabilitySource::System,
                version_warning: None,
            };
        }
    };

    // 1. Check Aleph managed paths
    if let Some(result) = probe_aleph_managed(spec) {
        return result;
    }

    // 2. Check system PATH via `which`
    if let Some(result) = probe_system_path(spec) {
        return result;
    }

    // 3. Not found
    ProbeResult {
        found: false,
        bin_path: None,
        version: None,
        source: CapabilitySource::System,
        version_warning: None,
    }
}

fn probe_aleph_managed(spec: &ProbeSpec) -> Option<ProbeResult> {
    let runtimes_dir = crate::utils::paths::get_runtimes_dir().ok()?;

    for relative_path in spec.aleph_paths {
        let full_path = runtimes_dir.join(relative_path);
        if full_path.exists() {
            let version = get_version(&full_path, spec.version_flag, spec.version_regex);
            let warning = check_version_warning(spec, version.as_deref());
            return Some(ProbeResult {
                found: true,
                bin_path: Some(full_path),
                version,
                source: CapabilitySource::AlephManaged,
                version_warning: warning,
            });
        }
    }
    None
}

fn probe_system_path(spec: &ProbeSpec) -> Option<ProbeResult> {
    for binary_name in spec.binaries {
        let which_cmd = if cfg!(target_os = "windows") { "where" } else { "which" };
        if let Ok(output) = Command::new(which_cmd).arg(binary_name).output() {
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    let bin_path = PathBuf::from(&path_str);
                    let version = get_version(&bin_path, spec.version_flag, spec.version_regex);
                    let warning = check_version_warning(spec, version.as_deref());
                    return Some(ProbeResult {
                        found: true,
                        bin_path: Some(bin_path),
                        version,
                        source: CapabilitySource::System,
                        version_warning: warning,
                    });
                }
            }
        }
    }
    None
}

fn get_version(bin_path: &PathBuf, version_flag: &str, version_regex: &str) -> Option<String> {
    let output = Command::new(bin_path).arg(version_flag).output().ok()?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let re = Regex::new(version_regex).ok()?;
    re.captures(&combined)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

fn check_version_warning(spec: &ProbeSpec, version: Option<&str>) -> Option<String> {
    let min = spec.min_version?;
    let actual = version?;

    if version_lt(actual, min) {
        Some(format!(
            "{} version {} is below recommended minimum {}",
            spec.capability, actual, min
        ))
    } else {
        None
    }
}

/// Simple semver comparison (major.minor only)
fn version_lt(actual: &str, minimum: &str) -> bool {
    let parse = |s: &str| -> (u32, u32) {
        let parts: Vec<&str> = s.split('.').collect();
        let major = parts.first().and_then(|p| p.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);
        (major, minor)
    };
    let (a_major, a_minor) = parse(actual);
    let (m_major, m_minor) = parse(minimum);
    (a_major, a_minor) < (m_major, m_minor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_lt() {
        assert!(version_lt("3.9", "3.10"));
        assert!(!version_lt("3.12", "3.10"));
        assert!(!version_lt("3.10", "3.10"));
        assert!(version_lt("2.0", "3.0"));
    }

    #[test]
    fn test_check_version_warning() {
        let spec = &PROBE_SPECS[0]; // python, min 3.10
        assert!(check_version_warning(spec, Some("3.9.0")).is_some());
        assert!(check_version_warning(spec, Some("3.12.0")).is_none());
        assert!(check_version_warning(spec, None).is_none());
    }

    #[test]
    fn test_probe_unknown_capability() {
        let result = probe("nonexistent_tool");
        assert!(!result.found);
    }

    #[test]
    fn test_probe_python_finds_system() {
        // This test depends on python3 being installed on the test machine
        // It's a real integration test
        let result = probe("python");
        // We don't assert found=true since CI may not have python
        // But we verify no panics and correct structure
        if result.found {
            assert!(result.bin_path.is_some());
        }
    }
}
```

**Step 2: Wire into mod.rs**

Add `pub mod probe;` to `core/src/runtimes/mod.rs` module declarations.
Add `pub use probe::ProbeResult;` to re-exports.

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test runtimes::probe --no-default-features -- --test-threads=1`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/runtimes/probe.rs core/src/runtimes/mod.rs
git commit -m "feat(runtimes): add system-first probe module with version detection"
```

---

### Task 5: Wire Ledger into exec layer PATH

**Files:**
- Modify: `core/src/builtin_tools/code_exec.rs` (lines 140-150 and 222-228)

**Step 1: Understand current code structure**

The exec tool at `core/src/builtin_tools/code_exec.rs`:
- Line 140-150: `CodeExecTool::new()` defines `pass_env` including `"PATH"`
- Line 222-228: `cmd.env_clear()` then passes filtered env vars

**Step 2: Add Ledger-aware PATH building**

The integration point depends on how `CodeExecTool` receives the ledger. Since this is a builtin tool, check how it currently gets invoked. The key change is:

In the environment setup section (around lines 222-228), replace the simple PATH passthrough:

```rust
// Before:
cmd.env_clear();
for var in &self.pass_env {
    if let Ok(value) = std::env::var(var) {
        cmd.env(var, value);
    }
}

// After:
cmd.env_clear();
for var in &self.pass_env {
    if var == "PATH" {
        // Use enhanced PATH with Aleph runtimes prepended
        if let Ok(enhanced) = crate::runtimes::ledger::build_enhanced_path() {
            cmd.env("PATH", enhanced);
        } else if let Ok(value) = std::env::var("PATH") {
            cmd.env("PATH", value);
        }
    } else if let Ok(value) = std::env::var(var) {
        cmd.env(var, value);
    }
}
```

For this, add a convenience function to `ledger.rs`:

```rust
/// Build enhanced PATH from the persisted ledger on disk
/// This is a convenience for callers that don't have the ledger in memory
pub fn build_enhanced_path() -> Result<String> {
    let runtimes_dir = crate::utils::paths::get_runtimes_dir()?;
    let ledger_path = runtimes_dir.join("ledger.json");
    let ledger = CapabilityLedger::load_or_create(ledger_path)?;
    Ok(ledger.build_path())
}
```

**Step 3: Verify compilation and run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check 2>&1 | tail -5`
Expected: Compiles

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test builtin_tools 2>&1 | tail -10`
Expected: Existing tests still pass

**Step 4: Commit**

```bash
git add core/src/builtin_tools/code_exec.rs core/src/runtimes/ledger.rs
git commit -m "feat(exec): inject Aleph runtime paths into subprocess PATH"
```

---

### Task 6: Wire RuntimeCapability into prompt system

**Files:**
- Modify: `core/src/runtimes/capability.rs` (adapt `format_for_prompt` to work with `CapabilityEntry`)
- Find and modify: the code path that constructs `PromptConfig` (in `core/src/gateway/execution_engine.rs` around line 466)

**Step 1: Add format_for_prompt for CapabilityEntry**

Add a new function to `core/src/runtimes/capability.rs`:

```rust
use crate::runtimes::ledger::CapabilityEntry;

/// Format capability entries for AI system prompt injection
pub fn format_entries_for_prompt(entries: &[&CapabilityEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    for entry in entries {
        output.push_str(&format!("**{}**\n", entry.name));
        if let Some(ref version) = entry.version {
            output.push_str(&format!("- Version: {}\n", version));
        }
        if let Some(ref path) = entry.bin_path {
            output.push_str(&format!("- Executable: {}\n", path.display()));
        }
        output.push_str(&get_usage_hints(&entry.name));
        output.push('\n');
    }
    output
}
```

**Step 2: Wire into PromptConfig construction**

In `core/src/gateway/execution_engine.rs` around line 466, where `PromptConfig` is constructed:

```rust
// Load runtime capabilities from ledger
let runtime_capabilities = {
    use crate::runtimes::ledger::build_enhanced_path;
    use crate::runtimes::ledger::CapabilityLedger;
    use crate::runtimes::capability::format_entries_for_prompt;

    let runtimes_dir = crate::utils::paths::get_runtimes_dir().ok();
    runtimes_dir.and_then(|dir| {
        let ledger = CapabilityLedger::load_or_create(dir.join("ledger.json")).ok()?;
        let ready = ledger.list_ready();
        if ready.is_empty() { None } else { Some(format_entries_for_prompt(&ready)) }
    })
};

let thinker_config = ThinkerConfig {
    prompt: PromptConfig {
        skill_instructions,
        runtime_capabilities,
        ..PromptConfig::default()
    },
    ..ThinkerConfig::default()
};
```

**Step 3: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check 2>&1 | tail -5`
Expected: Compiles

**Step 4: Commit**

```bash
git add core/src/runtimes/capability.rs core/src/gateway/execution_engine.rs
git commit -m "feat(prompt): wire runtime capabilities from ledger into AI system prompt"
```

---

## Phase 3: Bootstrap — Shell-Driven Installation

### Task 7: Create Bootstrap module with shell scripts

**Files:**
- Create: `core/src/runtimes/bootstrap.rs`
- Modify: `core/src/runtimes/mod.rs` (add `pub mod bootstrap;`)

**Step 1: Write the failing test**

Create `core/src/runtimes/bootstrap.rs`:

```rust
//! Shell-driven bootstrap for runtime installation
//!
//! Installation logic is embedded as shell scripts, not Rust code.
//! Scripts are executed through the exec layer's safety mechanisms.

use crate::error::{AlephError, Result};
use std::path::PathBuf;
use std::process::Command;

/// Result of a bootstrap attempt
#[derive(Debug)]
pub enum BootstrapResult {
    /// Binary found at expected path after install
    Success { bin_path: PathBuf },
    /// Install completed but binary not at expected path
    PathNotFound { expected: PathBuf },
    /// Install script failed
    Failed { stderr: String },
}

/// Bootstrap specification — embedded shell scripts
struct BootstrapSpec {
    capability: &'static str,
    script_macos: &'static str,
    script_linux: &'static str,
    expected_paths: &'static [&'static str], // Checked in order, first match wins
}

const BOOTSTRAP_SPECS: &[BootstrapSpec] = &[
    BootstrapSpec {
        capability: "uv",
        script_macos: "curl -LsSf https://astral.sh/uv/install.sh | sh",
        script_linux: "curl -LsSf https://astral.sh/uv/install.sh | sh",
        expected_paths: &["~/.local/bin/uv", "~/.cargo/bin/uv"],
    },
    BootstrapSpec {
        capability: "python",
        script_macos: "~/.local/bin/uv python install 3.12 && ~/.local/bin/uv venv ~/.aleph/runtimes/python/default --python 3.12",
        script_linux: "~/.local/bin/uv python install 3.12 && ~/.local/bin/uv venv ~/.aleph/runtimes/python/default --python 3.12",
        expected_paths: &["~/.aleph/runtimes/python/default/bin/python3"],
    },
    BootstrapSpec {
        capability: "node",
        script_macos: "curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell && eval \"$(~/.local/share/fnm/fnm env)\" && ~/.local/share/fnm/fnm install --lts",
        script_linux: "curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell && eval \"$(~/.local/share/fnm/fnm env)\" && ~/.local/share/fnm/fnm install --lts",
        expected_paths: &[
            "~/.local/share/fnm/aliases/default/bin/node",
            "~/.fnm/aliases/default/bin/node",
        ],
    },
    BootstrapSpec {
        capability: "ffmpeg",
        script_macos: "brew install ffmpeg 2>/dev/null || echo 'Please install ffmpeg manually: brew install ffmpeg'",
        script_linux: "sudo apt-get install -y ffmpeg 2>/dev/null || echo 'Please install ffmpeg manually'",
        expected_paths: &[
            "/opt/homebrew/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/usr/bin/ffmpeg",
        ],
    },
    BootstrapSpec {
        capability: "yt-dlp",
        script_macos: "~/.local/bin/uv tool install yt-dlp",
        script_linux: "~/.local/bin/uv tool install yt-dlp",
        expected_paths: &["~/.local/bin/yt-dlp"],
    },
];

/// Dependencies that must be bootstrapped first
pub fn dependencies(capability: &str) -> &'static [&'static str] {
    match capability {
        "python" => &["uv"],
        "yt-dlp" => &["uv"],
        _ => &[],
    }
}

/// Execute bootstrap for a capability via shell
pub fn bootstrap(capability: &str) -> Result<BootstrapResult> {
    let spec = BOOTSTRAP_SPECS
        .iter()
        .find(|s| s.capability == capability)
        .ok_or_else(|| AlephError::runtime(capability, "No bootstrap spec found"))?;

    let script = if cfg!(target_os = "macos") {
        spec.script_macos
    } else {
        spec.script_linux
    };

    tracing::info!("Bootstrapping {} via shell...", capability);

    let output = Command::new("sh")
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|e| AlephError::runtime(capability, &format!("Failed to execute bootstrap: {}", e)))?;

    if output.status.success() {
        // Check expected paths
        for expected in spec.expected_paths {
            let expanded = expand_tilde(expected);
            if expanded.exists() {
                tracing::info!("Bootstrap {} succeeded: {}", capability, expanded.display());
                return Ok(BootstrapResult::Success { bin_path: expanded });
            }
        }

        // Script succeeded but binary not found at expected paths
        let expected = expand_tilde(spec.expected_paths[0]);
        Ok(BootstrapResult::PathNotFound { expected })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        tracing::warn!("Bootstrap {} failed: {}", capability, stderr);
        Ok(BootstrapResult::Failed { stderr })
    }
}

/// Has a bootstrap spec for this capability
pub fn has_spec(capability: &str) -> bool {
    BOOTSTRAP_SPECS.iter().any(|s| s.capability == capability)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependencies() {
        assert_eq!(dependencies("python"), &["uv"]);
        assert_eq!(dependencies("yt-dlp"), &["uv"]);
        assert!(dependencies("ffmpeg").is_empty());
        assert!(dependencies("uv").is_empty());
    }

    #[test]
    fn test_has_spec() {
        assert!(has_spec("uv"));
        assert!(has_spec("python"));
        assert!(has_spec("node"));
        assert!(has_spec("ffmpeg"));
        assert!(has_spec("yt-dlp"));
        assert!(!has_spec("ruby"));
    }

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_tilde("~/.local/bin/uv");
        assert!(!expanded.to_string_lossy().contains('~'));
        assert!(expanded.to_string_lossy().contains(".local/bin/uv"));
    }

    #[test]
    fn test_expand_tilde_absolute() {
        let expanded = expand_tilde("/usr/local/bin/ffmpeg");
        assert_eq!(expanded, PathBuf::from("/usr/local/bin/ffmpeg"));
    }
}
```

**Step 2: Wire into mod.rs**

Add `pub mod bootstrap;` to `core/src/runtimes/mod.rs`.

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test runtimes::bootstrap --no-default-features -- --test-threads=1`
Expected: All 4 tests PASS

**Step 4: Commit**

```bash
git add core/src/runtimes/bootstrap.rs core/src/runtimes/mod.rs
git commit -m "feat(runtimes): add shell-driven bootstrap module with embedded scripts"
```

---

### Task 8: Create ensure_capability orchestration

**Files:**
- Create: `core/src/runtimes/ensure.rs`
- Modify: `core/src/runtimes/mod.rs` (add `pub mod ensure;`)

**Step 1: Create ensure.rs**

```rust
//! Capability orchestration — Probe → Bootstrap → Register
//!
//! The central function `ensure_capability` is called by the Dispatcher
//! when a tool needs a runtime that may not be installed.

use crate::error::{AlephError, Result};
use crate::runtimes::bootstrap::{self, BootstrapResult};
use crate::runtimes::ledger::{
    CapabilityEntry, CapabilityLedger, CapabilitySource, CapabilityStatus,
};
use crate::runtimes::probe;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Ensure a capability is ready, probing and bootstrapping if needed.
///
/// Returns the executable path on success.
pub async fn ensure_capability(
    capability: &str,
    ledger: &Arc<RwLock<CapabilityLedger>>,
) -> Result<PathBuf> {
    // Fast path: already Ready
    {
        let guard = ledger.read().await;
        if guard.status(capability) == CapabilityStatus::Ready {
            if let Some(path) = guard.executable(capability) {
                // Verify the path still exists
                if path.exists() {
                    return Ok(path.to_path_buf());
                }
                // Path gone — fall through to re-probe
            }
        }
    }

    // Probe phase
    info!("Probing for capability: {}", capability);
    {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Probing)?;
    }

    let probe_result = probe::probe(capability);

    if probe_result.found {
        let bin_path = probe_result.bin_path.clone().unwrap();
        if let Some(ref warning) = probe_result.version_warning {
            warn!("{}", warning);
        }

        let mut guard = ledger.write().await;
        guard.update(CapabilityEntry {
            name: capability.to_string(),
            bin_path: Some(bin_path.clone()),
            version: probe_result.version,
            status: CapabilityStatus::Ready,
            source: probe_result.source,
            last_probed: Some(std::time::SystemTime::now()),
        })?;
        guard.persist()?;

        info!("Capability {} found at {}", capability, bin_path.display());
        return Ok(bin_path);
    }

    // Bootstrap phase — resolve dependencies first
    for dep in bootstrap::dependencies(capability) {
        Box::pin(ensure_capability(dep, ledger)).await?;
    }

    // Check if bootstrap spec exists
    if !bootstrap::has_spec(capability) {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Missing)?;
        return Err(AlephError::runtime(
            capability,
            &format!("Capability '{}' not found and no bootstrap available", capability),
        ));
    }

    info!("Bootstrapping capability: {}", capability);
    {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Bootstrapping)?;
    }

    // Run bootstrap (blocking shell command in spawn_blocking)
    let cap_owned = capability.to_string();
    let bootstrap_result = tokio::task::spawn_blocking(move || {
        bootstrap::bootstrap(&cap_owned)
    })
    .await
    .map_err(|e| AlephError::runtime(capability, &format!("Bootstrap task panicked: {}", e)))??;

    match bootstrap_result {
        BootstrapResult::Success { bin_path } => {
            // Re-probe to get version info
            let version = {
                let re_probe = probe::probe(capability);
                re_probe.version
            };

            let mut guard = ledger.write().await;
            guard.update(CapabilityEntry {
                name: capability.to_string(),
                bin_path: Some(bin_path.clone()),
                version,
                status: CapabilityStatus::Ready,
                source: CapabilitySource::AlephManaged,
                last_probed: Some(std::time::SystemTime::now()),
            })?;
            guard.persist()?;

            info!("Capability {} bootstrapped at {}", capability, bin_path.display());
            Ok(bin_path)
        }
        BootstrapResult::PathNotFound { expected } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing)?;
            Err(AlephError::runtime(
                capability,
                &format!(
                    "Bootstrap completed but binary not found at expected path: {}",
                    expected.display()
                ),
            ))
        }
        BootstrapResult::Failed { stderr } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing)?;
            Err(AlephError::runtime(
                capability,
                &format!(
                    "Failed to bootstrap {}. Error: {}. Please install manually.",
                    capability, stderr
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_ensure_already_ready() {
        let dir = TempDir::new().unwrap();
        let ledger_path = dir.path().join("ledger.json");
        let mut ledger = CapabilityLedger::load_or_create(ledger_path).unwrap();

        // Pre-populate with a "ready" entry pointing to a real binary
        let bin = if cfg!(target_os = "windows") {
            PathBuf::from("C:\\Windows\\System32\\cmd.exe")
        } else {
            PathBuf::from("/bin/sh")
        };

        ledger.update(CapabilityEntry {
            name: "test-shell".into(),
            bin_path: Some(bin.clone()),
            version: Some("1.0".into()),
            status: CapabilityStatus::Ready,
            source: CapabilitySource::System,
            last_probed: Some(std::time::SystemTime::now()),
        }).unwrap();

        let ledger = Arc::new(RwLock::new(ledger));
        let result = ensure_capability("test-shell", &ledger).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), bin);
    }

    #[tokio::test]
    async fn test_ensure_unknown_capability() {
        let dir = TempDir::new().unwrap();
        let ledger_path = dir.path().join("ledger.json");
        let ledger = CapabilityLedger::load_or_create(ledger_path).unwrap();
        let ledger = Arc::new(RwLock::new(ledger));

        let result = ensure_capability("totally_unknown_thing_xyz", &ledger).await;
        assert!(result.is_err());
    }
}
```

**Step 2: Wire into mod.rs**

Add `pub mod ensure;` to `core/src/runtimes/mod.rs`.

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test runtimes::ensure --no-default-features -- --test-threads=1`
Expected: Both tests PASS

**Step 4: Commit**

```bash
git add core/src/runtimes/ensure.rs core/src/runtimes/mod.rs
git commit -m "feat(runtimes): add ensure_capability orchestration (probe → bootstrap → register)"
```

---

### Task 9: Update caption.rs consumer to use new Ledger

**Files:**
- Modify: `core/src/video/youtube/caption.rs` (lines 1-15)

**Step 1: Replace RuntimeRegistry usage**

Change `core/src/video/youtube/caption.rs`:

```rust
// Before (lines 1-15):
use crate::error::{AlephError, Result};
use crate::runtimes::RuntimeRegistry;
use std::path::PathBuf;
use tracing::debug;

pub async fn get_ytdlp_path() -> Result<PathBuf> {
    let registry = RuntimeRegistry::new()?;
    let ytdlp = registry.require("yt-dlp").await?;
    Ok(ytdlp.executable_path())
}

// After:
use crate::error::Result;
use crate::runtimes::ledger::CapabilityLedger;
use crate::runtimes::ensure::ensure_capability;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

pub async fn get_ytdlp_path() -> Result<PathBuf> {
    let runtimes_dir = crate::utils::paths::get_runtimes_dir()?;
    let ledger = CapabilityLedger::load_or_create(runtimes_dir.join("ledger.json"))?;
    let ledger = Arc::new(RwLock::new(ledger));
    ensure_capability("yt-dlp", &ledger).await
}
```

**Step 2: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check 2>&1 | tail -5`
Expected: Compiles

**Step 3: Commit**

```bash
git add core/src/video/youtube/caption.rs
git commit -m "refactor(video): migrate yt-dlp from RuntimeRegistry to CapabilityLedger"
```

---

## Phase 4: Cleanup — Delete Heavy Runtime Code

### Task 10: Delete old runtime manager files

**Files:**
- Delete: `core/src/runtimes/uv.rs`
- Delete: `core/src/runtimes/fnm.rs`
- Delete: `core/src/runtimes/ffmpeg.rs`
- Delete: `core/src/runtimes/ytdlp.rs`
- Delete: `core/src/runtimes/download.rs`
- Delete: `core/src/runtimes/git_check.rs`
- Modify: `core/src/runtimes/mod.rs` (remove old module declarations and re-exports)

**Step 1: Remove module declarations from mod.rs**

In `core/src/runtimes/mod.rs`, remove these lines:
- `mod download;`
- `mod ffmpeg;`
- `mod fnm;`
- `pub mod git_check;` (if present)
- `mod uv;`
- `mod ytdlp;`

And the corresponding re-exports:
- `pub use ffmpeg::FfmpegRuntime;`
- `pub use fnm::FnmRuntime;`
- `pub use uv::UvRuntime;`
- `pub use ytdlp::YtDlpRuntime;`

**Step 2: Delete the files**

```bash
rm core/src/runtimes/uv.rs
rm core/src/runtimes/fnm.rs
rm core/src/runtimes/ffmpeg.rs
rm core/src/runtimes/ytdlp.rs
rm core/src/runtimes/download.rs
rm core/src/runtimes/git_check.rs
```

**Step 3: Verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check 2>&1 | tail -20`
Expected: May produce errors if other files reference deleted types. Fix each one.

Likely fix needed: Remove references to `FfmpegRuntime`, `FnmRuntime`, `UvRuntime`, `YtDlpRuntime` from `registry.rs` — but since we're replacing registry too (next task), this is handled together.

**Step 4: Commit (only if compiles)**

```bash
git add -A core/src/runtimes/
git commit -m "refactor(runtimes): delete heavy runtime managers (~1776 LOC)"
```

---

### Task 11: Replace old RuntimeRegistry with minimal shim

**Files:**
- Modify: `core/src/runtimes/registry.rs` (replace entirely with thin shim, or delete)
- Modify: `core/src/runtimes/manager.rs` (delete or keep minimal trait)
- Modify: `core/src/runtimes/mod.rs` (update exports)

**Step 1: Decide on registry.rs**

If no other code references `RuntimeRegistry` (we confirmed only init and caption.rs did, and both are migrated), then:
- Delete `core/src/runtimes/registry.rs`
- Delete `core/src/runtimes/manager.rs`
- Remove their module declarations and re-exports from `mod.rs`

**Step 2: Clean up mod.rs**

The final `core/src/runtimes/mod.rs` should look like:

```rust
//! Runtime capability management
//!
//! Lightweight ledger-based runtime tracking with shell-driven bootstrapping.
//! Replaces the heavy RuntimeRegistry with on-demand provisioning.

pub mod bootstrap;
pub mod capability;
pub mod ensure;
pub mod ledger;
mod manifest; // Keep for migration support
pub mod probe;

pub use ledger::{CapabilityEntry, CapabilityLedger, CapabilitySource, CapabilityStatus};
pub use probe::ProbeResult;
pub use ensure::ensure_capability;
```

Also remove the old `build_aleph_path` function (lines 81-111) since `CapabilityLedger::build_path()` replaces it.

**Step 3: Fix any remaining compilation errors**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check 2>&1`

Fix any files that reference deleted types (grep for `RuntimeRegistry`, `RuntimeManager`, `RuntimeInfo`, `UpdateInfo`, `FfmpegRuntime`, etc.).

**Step 4: Run full test suite**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test 2>&1 | tail -20`
Expected: All tests PASS

**Step 5: Commit**

```bash
git add -A core/src/runtimes/
git commit -m "refactor(runtimes): replace RuntimeRegistry with CapabilityLedger (cleanup complete)"
```

---

### Task 12: Optional — remove unused Cargo dependencies

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Check if flate2/tar/xz2 are still used**

After deleting `download.rs` and `ffmpeg.rs`, check:

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && grep -r "flate2\|xz2\|tar::" src/ --include="*.rs" | grep -v "target/"
```

If only `download.rs` and `ffmpeg.rs` used them (both deleted), remove from `Cargo.toml`:
- `flate2 = "1.0"` (line 133)
- `tar = "0.4"` (line 134)
- `xz2 = "0.1"` (line 136)

Keep `zip` (used by plugins/skills).

**Step 2: Verify builds**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check 2>&1 | tail -5`

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "chore(deps): remove flate2/tar/xz2 deps (no longer needed after runtime cleanup)"
```

---

### Task 13: Final validation

**Step 1: Full build**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo build 2>&1 | tail -10`
Expected: PASS

**Step 2: Full test suite**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test 2>&1 | tail -20`
Expected: All tests PASS

**Step 3: Verify new runtime tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test runtimes:: 2>&1`
Expected: All ledger, probe, bootstrap, ensure tests PASS

**Step 4: Final commit (if any remaining changes)**

```bash
git status
# If clean, nothing to commit
# If there are fixes, commit them
```
