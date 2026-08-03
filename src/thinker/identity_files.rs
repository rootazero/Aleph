//! Standardized identity files for system prompt injection.
//!
//! Loads user-editable identity files (SOUL.md, IDENTITY.md, etc.) from the
//! identity directory, applying per-file and total budget constraints.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::thinker::prompt_budget::truncate_with_head_tail;

/// Canonical identity file names, loaded in this order.
///
/// Note: `MEMORY.md` is intentionally absent — it's owned by the curated
/// memory module (`src/memory/curated/`) and rendered into the prompt by
/// `CuratedMemoryLayer`, not loaded as a generic identity file.
pub const IDENTITY_FILE_NAMES: &[&str] = &[
    "SOUL.md",
    "IDENTITY.md",
    "AGENTS.md",
    "TOOLS.md",
    "HEARTBEAT.md",
];

/// Configuration for identity file loading and truncation.
#[derive(Debug, Clone)]
pub struct IdentityFilesConfig {
    /// Maximum characters per individual file before truncation.
    pub per_file_max_chars: usize,
    /// Maximum total characters across all loaded files.
    pub total_max_chars: usize,
}

impl Default for IdentityFilesConfig {
    fn default() -> Self {
        Self {
            per_file_max_chars: 20_000,
            total_max_chars: 100_000,
        }
    }
}

impl IdentityFilesConfig {
    /// Identity-file budget scaled to the model context window, mirroring the
    /// system-prompt budget (feature 1.2): the fixed 20k/100k caps become
    /// floors, so a large-window model may carry proportionally larger
    /// `SOUL.md` / `IDENTITY.md` content while small/unknown windows behave
    /// exactly as [`Default`]. Per-file rides a quarter of the per-file window
    /// fraction so no single file can dominate the total.
    #[must_use]
    pub fn for_context_window(window_tokens: u64) -> Self {
        use crate::thinker::prompt_budget::window_char_budget;
        Self {
            per_file_max_chars: window_char_budget(window_tokens, 0.025, 20_000, 120_000),
            total_max_chars: window_char_budget(window_tokens, 0.10, 100_000, 480_000),
        }
    }
}

/// A single loaded identity file with truncation metadata.
#[derive(Debug, Clone)]
pub struct IdentityFile {
    /// Canonical file name (e.g. "SOUL.md").
    pub name: &'static str,
    /// File content after truncation, or None if not found / empty.
    pub content: Option<String>,
    /// Whether the content was truncated to fit budget.
    pub truncated: bool,
    /// Original byte size before truncation (0 if not found).
    pub original_size: usize,
}

/// Collection of loaded identity files from an agent identity directory.
///
/// Identity files (SOUL.md / IDENTITY.md / AGENTS.md / TOOLS.md /
/// HEARTBEAT.md) live under `~/.aleph/agents/{agent_id}/` — this is the
/// agent's *identity* directory, distinct from `~/.aleph/workspaces/{agent_id}/`
/// which only holds runtime tool output and scratch files. `MEMORY.md` lives
/// alongside but is loaded by the curated memory module, not this loader.
#[derive(Debug, Clone)]
pub struct IdentityFiles {
    /// The agent identity directory these files were loaded from.
    pub identity_dir: PathBuf,
    /// Loaded files in canonical order.
    pub files: Vec<IdentityFile>,
}

/// Resolve the path for an identity file.
///
/// Checks `.aleph/<filename>` first, then `<identity_dir>/<filename>`.
/// Returns the first path that exists, or None.
#[must_use]
pub fn resolve_path(identity_dir: &Path, filename: &str) -> Option<PathBuf> {
    let aleph_path = identity_dir.join(".aleph").join(filename);
    if aleph_path.is_file() {
        return Some(aleph_path);
    }
    let root_path = identity_dir.join(filename);
    if root_path.is_file() {
        return Some(root_path);
    }
    None
}

impl IdentityFiles {
    /// Load all identity files from the given agent identity directory,
    /// applying truncation.
    ///
    /// Files are loaded in `IDENTITY_FILE_NAMES` order. Each file is
    /// individually capped at `config.per_file_max_chars`, and the total
    /// across all files is capped at `config.total_max_chars`.
    #[must_use]
    pub fn load(identity_dir: &Path, config: &IdentityFilesConfig) -> Self {
        let mut files = Vec::with_capacity(IDENTITY_FILE_NAMES.len());
        let mut total_chars = 0usize;

        for &name in IDENTITY_FILE_NAMES {
            let path = resolve_path(identity_dir, name);

            let raw = path.and_then(|p| std::fs::read_to_string(p).ok());

            // Skip missing or empty files
            let raw = match raw {
                Some(ref s) if !s.trim().is_empty() => s,
                _ => {
                    files.push(IdentityFile {
                        name,
                        content: None,
                        truncated: false,
                        original_size: 0,
                    });
                    continue;
                }
            };

            let original_size = raw.len();

            // Apply per-file truncation
            let remaining_budget = config.total_max_chars.saturating_sub(total_chars);
            let effective_limit = config.per_file_max_chars.min(remaining_budget);

            if effective_limit == 0 {
                // Total budget exhausted
                files.push(IdentityFile {
                    name,
                    content: None,
                    truncated: true,
                    original_size,
                });
                continue;
            }

            let (content, truncated) = if raw.chars().count() > effective_limit {
                let truncated_content = truncate_with_head_tail(raw, effective_limit, 0.7, 0.2);
                (truncated_content, true)
            } else {
                (raw.clone(), false)
            };

            total_chars += content.chars().count();
            files.push(IdentityFile {
                name,
                content: Some(content),
                truncated,
                original_size,
            });
        }

        Self {
            identity_dir: identity_dir.to_path_buf(),
            files,
        }
    }

    /// Get the content of an identity file by name.
    ///
    /// Returns the (possibly truncated) content, or None if not loaded.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|f| f.name == name)
            .and_then(|f| f.content.as_deref())
    }
}

// =============================================================================
// Identity-file write helpers (single source of truth for identity-file I/O)
//
// Shared by the two write surfaces over the SAME agent-dir identity files:
//   * `self_config` tool  — the LLM's in-conversation edit path (R8)
//   * `identity.*` gateway handlers — the external RPC / CLI edit path
// Keeping validation + backup here (not duplicated per caller) means both
// surfaces enforce the identical safety boundary on one source of truth.
// =============================================================================

/// How many timestamped backups to keep per identity file.
pub(crate) const MAX_IDENTITY_BACKUPS: usize = 5;

/// Maximum byte size accepted for an identity file write (1 MB).
pub const MAX_IDENTITY_FILE_SIZE: usize = 1024 * 1024;

/// Validate an identity file name against the canonical allow-list and reject
/// path-traversal characters. Returns a human-readable reason on rejection so
/// each caller can wrap it in its own error type.
pub fn validate_identity_file_name(name: &str) -> Result<(), String> {
    if !IDENTITY_FILE_NAMES.contains(&name) {
        return Err("Invalid file name".to_string());
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err("Invalid characters in file name".to_string());
    }
    Ok(())
}

/// Snapshot the current content of an identity file before it is overwritten,
/// into `<agent_dir>/backups/<file>.<UTC timestamp>`. Returns the backup path,
/// or `None` when the file does not exist yet (first write) or the snapshot
/// could not be taken — backup is best-effort protection, never a write gate.
pub(crate) fn backup_identity_file(
    agent_dir: &Path,
    file_name: &str,
    path: &Path,
) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    let backups_dir = agent_dir.join("backups");
    std::fs::create_dir_all(&backups_dir).ok()?;
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
    let backup_path = backups_dir.join(format!("{file_name}.{ts}"));
    std::fs::copy(path, &backup_path).ok()?;
    prune_identity_backups(&backups_dir, file_name, MAX_IDENTITY_BACKUPS);
    Some(backup_path)
}

/// Keep only the newest `keep` backups for one identity file. The timestamp
/// suffix is zero-padded UTC, so lexicographic order equals chronological.
pub(crate) fn prune_identity_backups(backups_dir: &Path, file_name: &str, keep: usize) {
    let Ok(entries) = std::fs::read_dir(backups_dir) else {
        return;
    };
    let prefix = format!("{file_name}.");
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(&prefix))
        .collect();
    if names.len() <= keep {
        return;
    }
    names.sort();
    let excess = names.len() - keep;
    for name in names.into_iter().take(excess) {
        let _ = std::fs::remove_file(backups_dir.join(name));
    }
}

/// Status metadata for one canonical identity file (existence + on-disk size).
#[derive(Debug, Clone, Serialize)]
pub struct IdentityFileStatus {
    /// Canonical file name (e.g. "SOUL.md").
    pub name: &'static str,
    /// Whether the file currently exists on disk.
    pub exists: bool,
    /// On-disk byte size (0 when absent).
    pub size: u64,
    /// Absolute path to the file.
    pub path: String,
}

/// List every canonical identity file under `agent_dir` with existence + size.
#[must_use]
pub fn list_identity_file_status(agent_dir: &Path) -> Vec<IdentityFileStatus> {
    IDENTITY_FILE_NAMES
        .iter()
        .map(|&name| {
            let path = agent_dir.join(name);
            let (exists, size) = std::fs::metadata(&path).map_or((false, 0), |m| (true, m.len()));
            IdentityFileStatus {
                name,
                exists,
                size,
                path: path.display().to_string(),
            }
        })
        .collect()
}

/// Outcome of a successful identity-file write.
#[derive(Debug)]
pub struct IdentityWriteOutcome {
    /// Number of bytes written to the live file.
    pub bytes_written: usize,
    /// Where the previous version was snapshotted, if one existed.
    pub backup_path: Option<PathBuf>,
}

/// Write `content` to `<agent_dir>/<file_name>`, validating the name, enforcing
/// the size cap, creating the directory, and snapshotting any prior version
/// first. Curated-memory-owned names (`MEMORY.md`) are rejected via the single
/// source `config::agent_manager::is_curated_owned` — the check must run
/// BEFORE `validate_identity_file_name`, or the generic "Invalid file name"
/// would shadow the actionable "use `remember`" message. Returns the write
/// outcome or a human-readable error reason.
///
/// This is the single low-level write used by both the `self_config` tool and
/// the `identity.*` handlers so the two paths cannot drift.
pub fn write_identity_file(
    agent_dir: &Path,
    file_name: &str,
    content: &str,
) -> Result<IdentityWriteOutcome, String> {
    if crate::config::agent_manager::is_curated_owned(file_name) {
        return Err(crate::config::agent_manager::curated_owned_reason(
            file_name,
        ));
    }
    validate_identity_file_name(file_name)?;

    if content.len() > MAX_IDENTITY_FILE_SIZE {
        return Err(format!(
            "Content exceeds maximum size limit of {MAX_IDENTITY_FILE_SIZE} bytes"
        ));
    }

    std::fs::create_dir_all(agent_dir)
        .map_err(|e| format!("Failed to create agent directory: {e}"))?;

    let path = agent_dir.join(file_name);
    let backup_path = backup_identity_file(agent_dir, file_name, &path);

    std::fs::write(&path, content).map_err(|e| format!("Failed to write {file_name}: {e}"))?;

    Ok(IdentityWriteOutcome {
        bytes_written: content.len(),
        backup_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn workspace_file_names_match_spec() {
        assert_eq!(IDENTITY_FILE_NAMES.len(), 5);
        assert_eq!(IDENTITY_FILE_NAMES[0], "SOUL.md");
        assert_eq!(IDENTITY_FILE_NAMES[1], "IDENTITY.md");
        assert_eq!(IDENTITY_FILE_NAMES[2], "AGENTS.md");
        assert_eq!(IDENTITY_FILE_NAMES[3], "TOOLS.md");
        assert_eq!(IDENTITY_FILE_NAMES[4], "HEARTBEAT.md");
        assert!(
            !IDENTITY_FILE_NAMES.contains(&"MEMORY.md"),
            "MEMORY.md is owned by curated memory module, not identity files"
        );
    }

    #[test]
    fn default_config_values() {
        let config = IdentityFilesConfig::default();
        assert_eq!(config.per_file_max_chars, 20_000);
        assert_eq!(config.total_max_chars, 100_000);
    }

    #[test]
    fn load_finds_existing_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("SOUL.md"), "You are Aleph.").unwrap();
        fs::write(dir.path().join("IDENTITY.md"), "Name: Aleph").unwrap();

        let config = IdentityFilesConfig::default();
        let ws = IdentityFiles::load(dir.path(), &config);

        assert_eq!(ws.files.len(), IDENTITY_FILE_NAMES.len());
        assert_eq!(ws.get("SOUL.md"), Some("You are Aleph."));
        assert_eq!(ws.get("IDENTITY.md"), Some("Name: Aleph"));
    }

    #[test]
    fn load_skips_missing_files() {
        let dir = TempDir::new().unwrap();
        // No files created

        let config = IdentityFilesConfig::default();
        let ws = IdentityFiles::load(dir.path(), &config);

        for file in &ws.files {
            assert!(file.content.is_none());
            assert!(!file.truncated);
            assert_eq!(file.original_size, 0);
        }
    }

    #[test]
    fn load_skips_empty_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("SOUL.md"), "").unwrap();
        fs::write(dir.path().join("IDENTITY.md"), "   \n  ").unwrap();

        let config = IdentityFilesConfig::default();
        let ws = IdentityFiles::load(dir.path(), &config);

        assert!(ws.get("SOUL.md").is_none());
        assert!(ws.get("IDENTITY.md").is_none());
    }

    #[test]
    fn load_truncates_large_files() {
        let dir = TempDir::new().unwrap();
        let large_content = "A".repeat(5000);
        fs::write(dir.path().join("SOUL.md"), &large_content).unwrap();

        let config = IdentityFilesConfig {
            per_file_max_chars: 200,
            total_max_chars: 100_000,
        };
        let ws = IdentityFiles::load(dir.path(), &config);

        let soul = ws.files.iter().find(|f| f.name == "SOUL.md").unwrap();
        assert!(soul.truncated);
        assert_eq!(soul.original_size, 5000);
        let content = soul.content.as_ref().unwrap();
        assert!(content.len() < 5000);
        assert!(content.contains("[..."));
        assert!(content.contains("truncated ...]"));
    }

    #[test]
    fn load_respects_total_budget() {
        let dir = TempDir::new().unwrap();
        // Each file 500 chars, total budget 900 — not all can fit
        for name in IDENTITY_FILE_NAMES {
            fs::write(dir.path().join(name), "X".repeat(500)).unwrap();
        }

        let config = IdentityFilesConfig {
            per_file_max_chars: 10_000,
            total_max_chars: 900,
        };
        let ws = IdentityFiles::load(dir.path(), &config);

        let total: usize = ws
            .files
            .iter()
            .filter_map(|f| f.content.as_ref().map(|c| c.len()))
            .sum();
        assert!(total <= 900, "Total {} exceeded budget 900", total);

        // First file should be loaded fully (500 < per_file and < total)
        assert!(ws.get("SOUL.md").is_some());

        // Some later files should be truncated or skipped
        let skipped_or_truncated = ws
            .files
            .iter()
            .filter(|f| f.original_size > 0 && (f.content.is_none() || f.truncated))
            .count();
        assert!(skipped_or_truncated > 0, "Budget should cause truncation");
    }

    #[test]
    fn get_returns_none_for_unknown_name() {
        let dir = TempDir::new().unwrap();
        let config = IdentityFilesConfig::default();
        let ws = IdentityFiles::load(dir.path(), &config);

        assert!(ws.get("NONEXISTENT.md").is_none());
    }

    #[test]
    fn get_returns_content_by_name() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("TOOLS.md"), "tool: bash").unwrap();

        let config = IdentityFilesConfig::default();
        let ws = IdentityFiles::load(dir.path(), &config);

        assert_eq!(ws.get("TOOLS.md"), Some("tool: bash"));
        assert!(ws.get("SOUL.md").is_none());
    }

    #[test]
    fn resolve_path_prefers_aleph_directory() {
        let dir = TempDir::new().unwrap();
        let aleph_dir = dir.path().join(".aleph");
        fs::create_dir_all(&aleph_dir).unwrap();

        // File exists in both root and .aleph/
        fs::write(dir.path().join("SOUL.md"), "root version").unwrap();
        fs::write(aleph_dir.join("SOUL.md"), "aleph version").unwrap();

        let resolved = resolve_path(dir.path(), "SOUL.md").unwrap();
        assert_eq!(resolved, aleph_dir.join("SOUL.md"));

        // Load should pick the .aleph/ version
        let config = IdentityFilesConfig::default();
        let ws = IdentityFiles::load(dir.path(), &config);
        assert_eq!(ws.get("SOUL.md"), Some("aleph version"));
    }

    #[test]
    fn resolve_path_falls_back_to_root() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("SOUL.md"), "root only").unwrap();

        let resolved = resolve_path(dir.path(), "SOUL.md").unwrap();
        assert_eq!(resolved, dir.path().join("SOUL.md"));
    }

    #[test]
    fn resolve_path_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        assert!(resolve_path(dir.path(), "SOUL.md").is_none());
    }

    #[test]
    fn for_context_window_floors_at_legacy_defaults() {
        // 200k window lands exactly on the legacy 20k/100k defaults; small
        // windows are floored there too — never tighter than ::default().
        let mid = IdentityFilesConfig::for_context_window(200_000);
        let def = IdentityFilesConfig::default();
        assert_eq!(mid.per_file_max_chars, def.per_file_max_chars);
        assert_eq!(mid.total_max_chars, def.total_max_chars);

        let tiny = IdentityFilesConfig::for_context_window(8_000);
        assert_eq!(tiny.per_file_max_chars, def.per_file_max_chars);
        assert_eq!(tiny.total_max_chars, def.total_max_chars);
    }

    #[test]
    fn for_context_window_scales_up_for_large_windows() {
        let big = IdentityFilesConfig::for_context_window(1_000_000);
        // 1M × 0.025 × 3.5 chars/token (single-source prose ratio).
        assert_eq!(big.per_file_max_chars, 87_500);
        // 1M × 0.10 × 3.5 chars/token (single-source prose ratio).
        assert_eq!(big.total_max_chars, 350_000);
    }

    #[test]
    fn validate_identity_file_name_accepts_canonical_and_rejects_others() {
        assert!(validate_identity_file_name("SOUL.md").is_ok());
        assert!(validate_identity_file_name("IDENTITY.md").is_ok());
        // Not on the allow-list.
        assert!(validate_identity_file_name("MEMORY.md").is_err());
        assert!(validate_identity_file_name("evil.md").is_err());
        // Path traversal is rejected even if it were on the list.
        assert!(validate_identity_file_name("../SOUL.md").is_err());
    }

    #[test]
    fn list_identity_file_status_reports_existence_and_size() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("SOUL.md"), "you are aleph").unwrap();

        let status = list_identity_file_status(dir.path());
        assert_eq!(status.len(), IDENTITY_FILE_NAMES.len());
        let soul = status.iter().find(|s| s.name == "SOUL.md").unwrap();
        assert!(soul.exists);
        assert_eq!(soul.size, "you are aleph".len() as u64);
        let heartbeat = status.iter().find(|s| s.name == "HEARTBEAT.md").unwrap();
        assert!(!heartbeat.exists);
        assert_eq!(heartbeat.size, 0);
    }

    #[test]
    fn write_identity_file_creates_and_backs_up() {
        let dir = TempDir::new().unwrap();

        // First write: no prior content, so no backup.
        let first = write_identity_file(dir.path(), "SOUL.md", "version one").unwrap();
        assert_eq!(first.bytes_written, "version one".len());
        assert!(first.backup_path.is_none());
        assert_eq!(
            fs::read_to_string(dir.path().join("SOUL.md")).unwrap(),
            "version one"
        );

        // Overwrite: previous content must be snapshotted.
        let second = write_identity_file(dir.path(), "SOUL.md", "version two").unwrap();
        let backup = second.backup_path.expect("overwrite must back up");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "version one");
        assert_eq!(
            fs::read_to_string(dir.path().join("SOUL.md")).unwrap(),
            "version two"
        );
    }

    #[test]
    fn write_identity_file_rejects_memory_md_and_bad_names() {
        let dir = TempDir::new().unwrap();
        let mem = write_identity_file(dir.path(), "MEMORY.md", "x").unwrap_err();
        assert!(mem.contains("remember"));
        assert!(!dir.path().join("MEMORY.md").exists());

        let bad = write_identity_file(dir.path(), "../../etc/passwd", "x").unwrap_err();
        assert!(bad.contains("Invalid"));
    }

    #[test]
    fn prune_keeps_newest_backups_only() {
        let tmp = TempDir::new().unwrap();
        let backups_dir = tmp.path().join("backups");
        std::fs::create_dir_all(&backups_dir).unwrap();
        // Seven fake backups with ascending (= chronological) suffixes,
        // plus one for another file that must be untouched.
        for i in 1..=7 {
            std::fs::write(backups_dir.join(format!("SOUL.md.2026010100000{i}Z")), "x").unwrap();
        }
        std::fs::write(backups_dir.join("TOOLS.md.20260101000001Z"), "y").unwrap();

        prune_identity_backups(&backups_dir, "SOUL.md", 5);

        let mut remaining: Vec<String> = std::fs::read_dir(&backups_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("SOUL.md."))
            .collect();
        remaining.sort();
        assert_eq!(remaining.len(), 5);
        // The two OLDEST were pruned.
        assert_eq!(remaining[0], "SOUL.md.20260101000003Z");
        // Other files' backups are untouched.
        assert!(backups_dir.join("TOOLS.md.20260101000001Z").exists());
    }
}
