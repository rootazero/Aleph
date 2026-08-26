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
/// Returns `<identity_dir>/<filename>` if it exists, otherwise `None`. The
/// legacy `.aleph/<filename>` shadow was dropped: every write surface
/// (`write_identity_file`, `self_config`, `identity.set/clear`, the `list_*`
/// helpers) operates only on the root file, so a read-prefer on `.aleph/`
/// would let an editor write to root while the prompt rendered the shadow —
/// a silent-edit failure mode. Anything that still relies on the shadow
/// layout would have been orphaned since the Phase-D2 cleanup; if the layout
/// is needed again it must round-trip through the shared write helpers.
#[must_use]
pub fn resolve_path(identity_dir: &Path, filename: &str) -> Option<PathBuf> {
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
                    });
                    continue;
                }
            };

            // Apply per-file truncation
            let remaining_budget = config.total_max_chars.saturating_sub(total_chars);
            let effective_limit = config.per_file_max_chars.min(remaining_budget);

            if effective_limit == 0 {
                // Total budget exhausted
                files.push(IdentityFile {
                    name,
                    content: None,
                });
                continue;
            }

            let content = if raw.chars().count() > effective_limit {
                truncate_with_head_tail(raw, effective_limit, 0.7, 0.2)
            } else {
                raw.clone()
            };

            total_chars += content.chars().count();
            files.push(IdentityFile {
                name,
                content: Some(content),
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

/// Validate the write path *before* any filesystem side effect, returning the
/// single error message the caller should surface. Centralised so the sync
/// `write_identity_file` and the async `write_identity_file_async` cannot
/// disagree on what counts as a refused write — a previous shape had two
/// copies of this check (one returning `Err(String)`, one returning
/// `ToolError`) and they drifted on the MEMORY.md priority ordering.
fn validate_identity_write(file_name: &str, content: &str) -> Result<(), String> {
    // MEMORY.md is owned entirely by the curated-memory module, not by the
    // identity-file path. It is not one of IDENTITY_FILE_NAMES, so this guard
    // must run BEFORE validate_file_name — otherwise the generic "Invalid
    // file name" error would shadow this actionable deprecation message.
    // The name list and the wording live in `config::agent_manager` so this
    // surface cannot drift from `agents.files.set` / `write_identity_file`.
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
    Ok(())
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
    validate_identity_write(file_name, content)?;

    std::fs::create_dir_all(agent_dir)
        .map_err(|e| format!("Failed to create agent directory: {e}"))?;

    let path = agent_dir.join(file_name);
    // Snapshotting uses blocking I/O — push it onto the blocking pool so a
    // large prior file (up to 1 MB) doesn't stall the runtime. The follow-up
    // `std::fs::write` stays on the current thread because by then the
    // blocking work is done and the write itself is on the agent's hot path.
    let backup_path =
        tokio::task::block_in_place(|| backup_identity_file(agent_dir, file_name, &path));

    std::fs::write(&path, content).map_err(|e| format!("Failed to write {file_name}: {e}"))?;

    Ok(IdentityWriteOutcome {
        bytes_written: content.len(),
        backup_path,
    })
}

/// Async counterpart to [`write_identity_file`] used by the `self_config`
/// tool (which is reached from an async tool runtime) and by the
/// `identity.*` RPC handlers (which are async and have a runtime already).
///
/// The two paths used to be separate hand-rolled re-implementations — the
/// tool re-derived `is_curated_owned` + `validate_identity_file_name` +
/// 1 MB cap + `create_dir_all` + backup + write because the sync helper
/// above blocked the runtime. The duplication let the MEMORY.md deprecation
/// message drift and the size cap drift (`MAX_FILE_CONTENT_SIZE` was a
/// local constant in `self_config.rs` with the same value but no shared
/// test). They now share [`validate_identity_write`] and the file-write
/// body, so a future drift would have to land twice.
pub async fn write_identity_file_async(
    agent_dir: &Path,
    file_name: &str,
    content: &str,
) -> Result<IdentityWriteOutcome, String> {
    validate_identity_write(file_name, content)?;

    tokio::fs::create_dir_all(agent_dir)
        .await
        .map_err(|e| format!("Failed to create agent directory: {e}"))?;

    let path = agent_dir.join(file_name);
    let agent_dir = agent_dir.to_path_buf();
    let path_for_backup = path.clone();
    // Backup is the blocking leg (std::fs::copy + chrono timestamp). The
    // shape of the result is preserved — None on any failure, mirroring the
    // sync helper's best-effort contract. The closure is `move`, so the
    // `&str` argument must be owned (`String`) to satisfy `'static`. We
    // clone up front so the error message below can still name the file.
    let file_name_owned = file_name.to_string();
    let backup_path = tokio::task::spawn_blocking(move || {
        backup_identity_file(&agent_dir, &file_name_owned, &path_for_backup)
    })
    .await
    .ok()
    .flatten();

    tokio::fs::write(&path, content)
        .await
        .map_err(|e| format!("Failed to write {file_name}: {e}"))?;

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

    #[tokio::test]
    async fn write_identity_file_async_round_trips_through_disk() {
        // Mirror of `write_then_load_round_trips_through_disk` for the async
        // helper. Both helpers share `validate_identity_write`, so a drift
        // in either side (e.g. one tightening the size cap the other missed)
        // would show up here as well. The shared `MAX_IDENTITY_FILE_SIZE`
        // constant is also exercised through both paths in the oversize
        // test below.
        let dir = TempDir::new().unwrap();
        let initial = write_identity_file_async(dir.path(), "SOUL.md", "async v1")
            .await
            .expect("initial async write");
        assert!(initial.backup_path.is_none());

        let files_v1 = IdentityFiles::load(dir.path(), &IdentityFilesConfig::default());
        assert_eq!(files_v1.get("SOUL.md"), Some("async v1"));

        let second = write_identity_file_async(dir.path(), "SOUL.md", "async v2")
            .await
            .expect("second async write");
        assert!(second.backup_path.is_some(), "overwrite must snapshot");

        let files_v2 = IdentityFiles::load(dir.path(), &IdentityFilesConfig::default());
        assert_eq!(
            files_v2.get("SOUL.md"),
            Some("async v2"),
            "async loader must observe the most recent write"
        );
    }

    #[tokio::test]
    async fn write_identity_file_async_rejects_memory_md_with_curated_message() {
        // The shared `validate_identity_write` must surface the curated-owned
        // error BEFORE the generic "Invalid file name" — both the sync and
        // async helpers depend on this ordering, and the action message is
        // what the model reads to route to the `remember` tool.
        let dir = TempDir::new().unwrap();
        let err = write_identity_file_async(dir.path(), "MEMORY.md", "anything")
            .await
            .unwrap_err();
        assert!(
            err.contains("remember"),
            "curated-owned message must lead: {err}"
        );
        assert!(!dir.path().join("MEMORY.md").exists());
    }

    #[tokio::test]
    async fn write_identity_file_async_rejects_oversize_with_size_message() {
        // Pin the shared cap for the async helper — guards against a future
        // refactor that tightens one helper and forgets the other.
        let dir = TempDir::new().unwrap();
        let oversize = "x".repeat(MAX_IDENTITY_FILE_SIZE + 1);
        let err = write_identity_file_async(dir.path(), "SOUL.md", &oversize)
            .await
            .unwrap_err();
        assert!(err.contains("exceeds maximum size"), "msg was: {err}");
        assert!(!dir.path().join("SOUL.md").exists());
        assert!(!dir.path().join("backups").exists());
    }

    #[test]
    fn write_then_load_round_trips_through_disk() {
        // End-to-end guard for the write→read pipeline that the runtime
        // prompt uses every turn. A prior shape split writes and reads
        // between two surfaces with subtly different path resolution
        // (the `.aleph/` shadow read-prefer), so edits could land on
        // disk while the loader kept rendering the old copy. This test
        // exercises the full cycle: `write_identity_file` is the shared
        // primitive the two write surfaces use; `IdentityFiles::load` is
        // the only reader the prompt layer ever sees.
        let dir = TempDir::new().unwrap();
        let initial =
            write_identity_file(dir.path(), "SOUL.md", "you are aleph v1").expect("initial write");
        assert!(initial.backup_path.is_none(), "first write has no prior");

        let files_v1 = IdentityFiles::load(dir.path(), &IdentityFilesConfig::default());
        assert_eq!(files_v1.get("SOUL.md"), Some("you are aleph v1"));

        // Overwrite — the prior version must be snapshotted before the
        // loader can ever observe the new copy.
        let second =
            write_identity_file(dir.path(), "SOUL.md", "you are aleph v2").expect("second write");
        assert!(
            second.backup_path.is_some(),
            "overwrite must snapshot the prior version"
        );

        // The next-turn loader picks up the new copy verbatim. A stale read
        // here would mean an editing edit landed on disk while the prompt
        // kept rendering the old content — the exact failure mode this
        // guard was added to catch.
        let files_v2 = IdentityFiles::load(dir.path(), &IdentityFilesConfig::default());
        assert_eq!(
            files_v2.get("SOUL.md"),
            Some("you are aleph v2"),
            "loader must observe the most recent write"
        );
    }

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

        // Some later files should be skipped once the total budget is spent.
        let skipped = ws.files.iter().filter(|f| f.content.is_none()).count();
        assert!(skipped > 0, "Budget should cause truncation");
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
    fn resolve_path_returns_root_only() {
        // Regression: the read-prefer on `.aleph/` shadow was removed because
        // every write surface operates only on the root file. If a shadow
        // copy exists, `resolve_path` MUST ignore it — otherwise the prompt
        // would render stale content while `identity.set` writes the root.
        let dir = TempDir::new().unwrap();
        let aleph_dir = dir.path().join(".aleph");
        fs::create_dir_all(&aleph_dir).unwrap();

        fs::write(dir.path().join("SOUL.md"), "root version").unwrap();
        fs::write(aleph_dir.join("SOUL.md"), "stale shadow version").unwrap();

        let resolved = resolve_path(dir.path(), "SOUL.md").unwrap();
        assert_eq!(
            resolved,
            dir.path().join("SOUL.md"),
            "shadow .aleph/ copy must be ignored"
        );

        let config = IdentityFilesConfig::default();
        let ws = IdentityFiles::load(dir.path(), &config);
        assert_eq!(
            ws.get("SOUL.md"),
            Some("root version"),
            "loader must surface the root version"
        );
    }

    #[test]
    fn resolve_path_returns_root_when_present() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("SOUL.md"), "root only").unwrap();

        let resolved = resolve_path(dir.path(), "SOUL.md").unwrap();
        assert_eq!(resolved, dir.path().join("SOUL.md"));
    }

    #[test]
    fn resolve_path_returns_root_when_present_and_shadow_absent() {
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
    fn write_identity_file_rejects_content_above_size_cap() {
        // Pin the 1 MB ceiling at the library surface — the same cap
        // `self_config::write_file` and `identity.set` enforce. Any caller
        // that grows past `MAX_IDENTITY_FILE_SIZE` must be turned away
        // BEFORE any filesystem side effect (no partial write, no backup
        // of the previous good version against a now-corrupt candidate).
        let dir = TempDir::new().unwrap();
        let oversize = "x".repeat(MAX_IDENTITY_FILE_SIZE + 1);
        let err = write_identity_file(dir.path(), "SOUL.md", &oversize).unwrap_err();
        assert!(
            err.contains("exceeds maximum size"),
            "size error must name the cap: {err}"
        );
        assert!(
            err.contains(&MAX_IDENTITY_FILE_SIZE.to_string()),
            "size error must cite the byte limit: {err}"
        );
        assert!(!dir.path().join("SOUL.md").exists(), "no file created");
        assert!(
            !dir.path().join("backups").exists(),
            "no backup dir created — oversize must be a pre-flight gate"
        );

        // And exactly at the cap is still accepted.
        let exact = "y".repeat(MAX_IDENTITY_FILE_SIZE);
        write_identity_file(dir.path(), "SOUL.md", &exact).expect("exact-cap write should succeed");
        assert_eq!(
            std::fs::metadata(dir.path().join("SOUL.md")).unwrap().len(),
            MAX_IDENTITY_FILE_SIZE as u64
        );
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
