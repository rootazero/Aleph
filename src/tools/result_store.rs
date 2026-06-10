//! ToolResultStore — disk persistence for large tool results.
//!
//! When a tool result exceeds the token threshold, it is written to a
//! session-scoped directory on disk. A compact reference marker is injected
//! into the context window so the LLM can identify that the full output
//! exists but was offloaded.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crate::sync_primitives::Arc;

use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::retrieval::{ContentIndex, IndexOutcome, SearchHit};

/// Prefix used to identify persisted-result reference lines.
const PERSISTED_REF_PREFIX: &str = "[Full output persisted: ";

/// Default retention window for the periodic sweeper. Mirrors opencode's
/// `Truncate.cleanup` cutoff (7 days). Persisted tool-result files older
/// than this are garbage-collected by [`sweep_stale_tool_result_dirs`].
pub const DEFAULT_TOOL_RESULT_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Default sweep cadence for the background TTL task. Matches opencode's
/// hourly cadence — a directory walk over `tool_results/` is cheap and the
/// task does not need to wake more often than that to bound disk usage.
pub const DEFAULT_TOOL_RESULT_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

// =============================================================================
// Process-wide installer
// =============================================================================
//
// Layer 2 / Layer 3 of the tool-result budget both need access to a shared
// `Arc<ToolResultStore>` so the marker path the LLM sees matches a real file
// on disk. Rather than thread the `Arc` through every constructor that builds
// a `ScopedToolService` (gateway path) or an `AgentHarnessRunner` (orchestrator
// path), we install one at server boot via `set_global_tool_result_store` and
// read it back through `global_tool_result_store`. The same singleton is
// consumed by:
//   * `gateway::execution_engine::tool_service_builder::build_request_tool_service`
//   * `orchestrator::harness_bridge::AgentHarnessRunner` (HarnessDeps wiring)
// Tests or alternative bootstraps that prefer per-instance injection can still
// use `ScopedToolService::with_result_store` / `HarnessDeps.result_store`
// directly; the global slot is `Option`-shaped and a `None` value means
// "fall back to in-line truncation only".
static GLOBAL_STORE: OnceLock<Arc<ToolResultStore>> = OnceLock::new();

/// Install the process-wide `ToolResultStore`. Idempotent — subsequent calls
/// are silently ignored so multiple boot paths cannot stomp each other.
///
/// First-call side effect: spawns the periodic TTL sweeper
/// ([`spawn_periodic_sweeper`]) rooted at the store's `tool_results/` parent
/// directory. The sweeper opportunistically reclaims orphaned session dirs
/// left behind by hard crashes (where `Drop` cleanup never ran) and stale
/// dirs from previous runs of the process. Mirrors opencode's `Truncate`
/// background cleanup loop.
///
/// The sweeper is intentionally fire-and-forget: it has no shutdown handle
/// and is dropped only when the process exits. Test boot paths that want
/// deterministic GC should call [`sweep_stale_tool_result_dirs`] directly.
pub fn set_global_tool_result_store(store: Arc<ToolResultStore>) {
    // `OnceLock::set` errors if a prior install won — in that case we
    // intentionally do nothing (the existing sweeper is fine).
    let installed_now = GLOBAL_STORE.set(store.clone()).is_ok();
    if installed_now {
        // The sweeper walks the parent of the per-session dir
        // (`~/.aleph/data/tool_results/`) so every other session's dir is
        // visible. `parent()` is `None` only for fs roots — never the case
        // for our `~/.aleph/data/tool_results/<session_id>` layout.
        if let Some(parent) = store.base_dir.parent().map(Path::to_path_buf) {
            spawn_periodic_sweeper(
                parent,
                DEFAULT_TOOL_RESULT_RETENTION,
                DEFAULT_TOOL_RESULT_SWEEP_INTERVAL,
            );
        }
    }
}

/// Read the process-wide `ToolResultStore`, if installed.
pub fn global_tool_result_store() -> Option<Arc<ToolResultStore>> {
    GLOBAL_STORE.get().cloned()
}

// =============================================================================
// ToolResultStore
// =============================================================================

/// Filename of the FTS5 retrieval index inside [`ToolResultStore::base_dir`].
const INDEX_DB_NAME: &str = "index.db";

/// Session-scoped store that offloads large tool outputs to disk.
///
/// On drop the store removes its base directory, so tool result files are
/// automatically cleaned up when the session ends.
///
/// Alongside the raw `.txt` blobs, the store lazily maintains an FTS5
/// [`ContentIndex`] (`index.db`) so the model can BM25-search offloaded
/// output via `ctx_search` instead of re-reading whole files. The index is
/// opened on first use and shares the directory's Drop / TTL-sweep lifecycle.
pub struct ToolResultStore {
    base_dir: PathBuf,
    /// Lazily-opened retrieval index. `None` inside the `OnceLock` means an
    /// open attempt failed once and indexing/search degrade to no-ops.
    index: OnceLock<Option<ContentIndex>>,
}

impl ToolResultStore {
    /// Create a new store for the given session.
    ///
    /// Creates `~/.aleph/data/tool_results/{session_id}/` on disk.
    pub fn new(session_id: &str) -> std::io::Result<Self> {
        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".aleph")
            .join("data")
            .join("tool_results")
            .join(session_id);

        std::fs::create_dir_all(&base_dir)?;
        Ok(Self {
            base_dir,
            index: OnceLock::new(),
        })
    }

    /// Construct a store rooted at an arbitrary base directory. The
    /// caller must create the directory before this returns. Exposed
    /// for tests in adjacent modules (e.g. `result_processing`,
    /// `scoped`) that need a `ToolResultStore` without touching
    /// `~/.aleph/`.
    #[doc(hidden)]
    #[must_use]
    pub fn with_dir_for_tests(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            index: OnceLock::new(),
        }
    }

    /// Persist the content to disk if it exceeds `threshold_tokens`.
    ///
    /// Returns a reference marker string if the content was persisted, or
    /// `None` if the content is small enough to remain in the context window.
    pub fn persist_if_large(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        content: &str,
        threshold_tokens: usize,
    ) -> Option<String> {
        let tokens = estimate_tokens_smart(content);
        if tokens <= threshold_tokens {
            return None;
        }

        // Use a sanitized filename: {tool_call_id}_{tool_name}.txt
        let safe_name = format!(
            "{}_{}.txt",
            sanitize_for_filename(tool_call_id),
            sanitize_for_filename(tool_name)
        );
        let path = self.base_dir.join(&safe_name);

        if let Err(e) = std::fs::write(&path, content) {
            tracing::warn!(
                tool_call_id = tool_call_id,
                tool_name = tool_name,
                error = %e,
                "failed to persist tool result to disk"
            );
            return None;
        }

        let marker = format!(
            "{}{} ({} tokens, {})]",
            PERSISTED_REF_PREFIX,
            path.display(),
            tokens,
            tool_name,
        );
        Some(marker)
    }

    /// Lazily open (once) the FTS5 retrieval index in `base_dir`. Returns
    /// `None` if the index could not be opened — callers degrade to no-ops.
    fn index(&self) -> Option<&ContentIndex> {
        self.index
            .get_or_init(|| {
                let db_path = self.base_dir.join(INDEX_DB_NAME);
                match ContentIndex::open(&db_path) {
                    Ok(idx) => Some(idx),
                    Err(e) => {
                        tracing::warn!(
                            db = %db_path.display(),
                            error = %e,
                            "failed to open tool-result content index; ctx_search disabled"
                        );
                        None
                    }
                }
            })
            .as_ref()
    }

    /// Index an offloaded tool output into the retrieval store so the model
    /// can BM25-search it via `ctx_search`. Best-effort: returns `None` (and
    /// logs) on any failure, and `Some(outcome)` with the section count on
    /// success. Callers use the outcome to build the model-facing hint.
    pub fn index_output(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        content: &str,
    ) -> Option<IndexOutcome> {
        let idx = self.index()?;
        match idx.index_text(tool_call_id, tool_name, content) {
            Ok(out) => Some(out),
            Err(e) => {
                tracing::warn!(
                    tool_call_id,
                    tool_name,
                    error = %e,
                    "failed to index tool output for retrieval"
                );
                None
            }
        }
    }

    /// BM25-search previously-indexed tool output. Returns up to `limit` hits,
    /// most relevant first. Empty when nothing is indexed or the index is
    /// unavailable — never errors out to the caller.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        match self.index() {
            Some(idx) => idx.search(query, limit).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Number of indexed sections across all offloaded outputs. `0` when the
    /// index is empty or unavailable.
    pub fn indexed_sections(&self) -> usize {
        self.index().and_then(|idx| idx.len().ok()).unwrap_or(0)
    }

    /// Remove the base directory and all its contents.
    pub fn cleanup(&self) {
        if self.base_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&self.base_dir) {
                tracing::warn!(
                    dir = %self.base_dir.display(),
                    error = %e,
                    "failed to clean up tool result store"
                );
            }
        }
    }

    /// Purge every offloaded tool result — the `.txt` blobs *and* the FTS5
    /// index entries — while keeping the store usable for the rest of the
    /// session (the directory and `index.db` survive; only their contents go).
    ///
    /// This is the **anti-reference-bypass** countermeasure (maps OpenSquilla's
    /// `StaleOutputCache.purge`). Offloaded output is otherwise retrievable
    /// indefinitely via `read_file` on the `[Full output persisted: …]` marker
    /// path or via `ctx_search` over the index — neither of which consults the
    /// approval gate. So when a session trips the denial circuit-breaker, the
    /// gate's enforcement could be sidestepped by mining a result that was
    /// cached under an earlier, more permissive moment. Wiping both vectors at
    /// the trip closes that hole. It fires only on the brute-force threshold,
    /// so ordinary large-output workflows are never disturbed.
    pub fn purge_all(&self) {
        // 1. Remove offloaded `.txt` blobs (the `read_file`-via-marker vector).
        if let Ok(entries) = std::fs::read_dir(&self.base_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("txt") {
                    if let Err(e) = std::fs::remove_file(&path) {
                        tracing::warn!(
                            file = %path.display(),
                            error = %e,
                            "purge_all: failed to remove offloaded tool-result blob"
                        );
                    }
                }
            }
        }
        // 2. Clear the FTS5 index (the `ctx_search` vector). `index()` opens it
        // lazily, so this also catches an on-disk `index.db` left by an earlier
        // turn that was never reopened this run.
        if let Some(idx) = self.index() {
            if let Err(e) = idx.clear() {
                tracing::warn!(
                    error = %e,
                    "purge_all: failed to clear offloaded-output index"
                );
            }
        }
    }
}

impl Drop for ToolResultStore {
    fn drop(&mut self) {
        self.cleanup();
    }
}

// =============================================================================
// Standalone helpers
// =============================================================================

/// Scan `text` for a `[Full output persisted: ...]` reference line and return
/// the first matching line if found.
#[must_use]
pub fn extract_persisted_ref(text: &str) -> Option<&str> {
    text.lines()
        .find(|line| line.starts_with(PERSISTED_REF_PREFIX))
}

// =============================================================================
// TTL sweeper for stale per-session dirs
// =============================================================================

/// Sweep stale persisted-result directories under `root` and remove any whose
/// most-recent file mtime is older than `cutoff`. Empty directories are
/// removed unconditionally.
///
/// Returns the number of directories removed. Errors on individual entries
/// are logged at WARN and skipped so a single permission denial cannot stall
/// the sweep.
///
/// This is the synchronous core of [`spawn_periodic_sweeper`]. It is exposed
/// publicly so test bootstraps and tools (e.g. an admin `aleph` CLI) can
/// trigger a one-shot GC pass without owning the background task.
pub fn sweep_stale_tool_result_dirs(root: &Path, cutoff: Duration) -> usize {
    let entries = match std::fs::read_dir(root) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
        Err(e) => {
            tracing::warn!(
                root = %root.display(),
                error = %e,
                "tool_result sweep: read_dir failed"
            );
            return 0;
        }
    };
    let now = SystemTime::now();
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        // The store layout is one directory per session_id; ignore stray
        // files at the root so we never delete user-placed artifacts.
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        if dir_is_stale(&path, now, cutoff) {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => removed += 1,
                Err(e) => {
                    tracing::warn!(
                        dir = %path.display(),
                        error = %e,
                        "tool_result sweep: remove_dir_all failed"
                    );
                }
            }
        }
    }
    removed
}

/// True iff `dir` has no entries newer than `cutoff` from `now`. An empty
/// directory is always considered stale (it serves no purpose). Errors
/// reading individual files are treated as "fresh" — the sweep prefers
/// false negatives (skip removal) over false positives (kill live state).
fn dir_is_stale(dir: &Path, now: SystemTime, cutoff: Duration) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            // Unreadable entry → assume live, keep dir.
            return false;
        };
        let mtime = meta.modified().unwrap_or(now);
        if now.duration_since(mtime).unwrap_or(Duration::ZERO) < cutoff {
            return false;
        }
    }
    // Either no entries (empty dir → stale) or every entry was older than
    // `cutoff` (we'd have early-returned `false` otherwise).
    true
}

/// Spawn a background Tokio task that periodically calls
/// [`sweep_stale_tool_result_dirs`] on `root`.
///
/// Fire-and-forget: the task is detached. Multiple calls with the same
/// `root` will spawn multiple sweepers — callers should invoke this once
/// per `root` (the [`set_global_tool_result_store`] entry point enforces
/// that via `OnceLock`).
///
/// No-op if there is no running Tokio runtime — log at DEBUG and return so
/// that non-async test bootstraps don't panic.
pub fn spawn_periodic_sweeper(root: PathBuf, retention: Duration, interval: Duration) {
    if tokio::runtime::Handle::try_current().is_err() {
        tracing::debug!(
            root = %root.display(),
            "tool_result sweeper not spawned: no Tokio runtime"
        );
        return;
    }
    tokio::spawn(async move {
        // First sweep is delayed by one interval so boot doesn't block on
        // a synchronous fs walk; opencode follows the same pattern.
        let mut ticker = tokio::time::interval(interval);
        // Skip the initial "fire immediately" tick.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let root = root.clone();
            // Move the blocking fs walk off the async runtime.
            let removed = match tokio::task::spawn_blocking(move || {
                sweep_stale_tool_result_dirs(&root, retention)
            })
            .await
            {
                Ok(count) => count,
                Err(e) => {
                    tracing::warn!(error = %e, "tool_result sweeper task failed");
                    0
                }
            };
            if removed > 0 {
                tracing::info!(removed, "tool_result sweeper reclaimed stale session dirs");
            }
        }
    });
}

/// Replace characters unsafe for filenames with underscores.
fn sanitize_for_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Build a test store rooted in a temp directory instead of ~/.aleph/.
    fn test_store(name: &str) -> (ToolResultStore, PathBuf) {
        let base = std::env::temp_dir()
            .join("aleph_test_tool_result_store")
            .join(name);
        std::fs::create_dir_all(&base).unwrap();
        let store = ToolResultStore {
            base_dir: base.clone(),
            index: OnceLock::new(),
        };
        (store, base)
    }

    #[test]
    fn small_result_not_persisted() {
        let (store, _base) = test_store("small_result_not_persisted");
        // threshold = 10_000 tokens; short content is well under
        let result = store.persist_if_large("call_1", "read_file", "hello world", 10_000);
        assert!(result.is_none(), "short content should not be persisted");
    }

    #[test]
    fn large_result_persisted_and_recoverable() {
        let (store, base) = test_store("large_result_persisted");
        // Generate content that is definitely > 1 token (threshold = 1)
        let content = "a".repeat(1000);
        let result = store.persist_if_large("call_abc", "bash", &content, 1);
        assert!(result.is_some(), "large content should be persisted");
        let marker = result.unwrap();
        assert!(
            marker.starts_with(PERSISTED_REF_PREFIX),
            "marker must start with prefix: {marker}"
        );
        // Verify a .txt file was created and its content matches
        let files: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1, "exactly one file should be written");
        let written = std::fs::read_to_string(files[0].path()).unwrap();
        assert_eq!(written, content, "written content must match original");
    }

    #[test]
    fn cleanup_removes_directory() {
        let base = std::env::temp_dir()
            .join("aleph_test_tool_result_store")
            .join("cleanup_test");
        std::fs::create_dir_all(&base).unwrap();
        let store = ToolResultStore {
            base_dir: base.clone(),
            index: OnceLock::new(),
        };
        assert!(base.exists());
        store.cleanup();
        assert!(!base.exists(), "cleanup should remove the base directory");
    }

    #[test]
    fn extract_persisted_ref_finds_marker() {
        let text =
            "some output\n[Full output persisted: /tmp/foo.txt (1234 tokens, bash)]\nmore text";
        let found = extract_persisted_ref(text);
        assert!(found.is_some(), "should find marker line");
        assert!(found.unwrap().contains("Full output persisted"));
    }

    #[test]
    fn extract_persisted_ref_returns_none_when_absent() {
        let text = "no marker here\njust regular output";
        let found = extract_persisted_ref(text);
        assert!(found.is_none(), "should return None when no marker present");
    }

    #[test]
    fn purge_all_removes_blobs_and_clears_index() {
        let (store, base) = test_store("purge_all_removes_blobs");
        // Clean any residue from a prior run so the count assertions are exact.
        if let Ok(entries) = std::fs::read_dir(&base) {
            for e in entries.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let content = "secret-token-abcdef ".repeat(200);
        // Offload a large result (writes a `.txt` blob) and index it (so
        // ctx_search could later mine it — the reference-bypass vector).
        assert!(store
            .persist_if_large("call_x", "bash", &content, 1)
            .is_some());
        let _ = store.index_output("call_x", "bash", &content);
        assert!(store.indexed_sections() > 0, "content should be indexed");

        let txt_count = |base: &std::path::Path| {
            std::fs::read_dir(base)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("txt"))
                .count()
        };
        assert_eq!(
            txt_count(&base),
            1,
            "one offloaded blob present before purge"
        );

        store.purge_all();

        assert_eq!(
            txt_count(&base),
            0,
            "offloaded blobs must be gone after purge"
        );
        assert_eq!(
            store.indexed_sections(),
            0,
            "index must be empty after purge"
        );
        assert!(
            store.search("secret-token-abcdef", 5).is_empty(),
            "ctx_search must find nothing after purge"
        );
    }

    // -------------------------------------------------------------------
    // TTL sweeper
    // -------------------------------------------------------------------

    use std::time::Duration;

    fn sweeper_root(name: &str) -> PathBuf {
        let base = std::env::temp_dir()
            .join("aleph_test_tool_result_sweeper")
            .join(name);
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// Set mtime backwards by `secs` via `filetime` so the dir looks stale.
    /// We use the `filetime` crate which is already a transitive dep; if
    /// not available, fall back to creating the dir and immediately
    /// claiming it's old enough by passing `cutoff = Duration::ZERO`.
    fn touch_old(path: &Path, secs: u64) {
        let when = SystemTime::now() - Duration::from_secs(secs);
        let ft = filetime::FileTime::from_system_time(when);
        // Set mtime on the dir and every file inside it.
        let _ = filetime::set_file_mtime(path, ft);
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let _ = filetime::set_file_mtime(entry.path(), ft);
            }
        }
    }

    #[test]
    fn sweep_removes_stale_dir() {
        let root = sweeper_root("removes_stale_dir");
        let stale = root.join("session_old");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("call_1_bash.txt"), "old data").unwrap();
        // 14 days old, retention 7 days → stale.
        touch_old(&stale, 14 * 24 * 60 * 60);

        let removed = sweep_stale_tool_result_dirs(&root, DEFAULT_TOOL_RESULT_RETENTION);
        assert_eq!(removed, 1, "should remove the one stale dir");
        assert!(!stale.exists(), "stale dir should be gone");
    }

    #[test]
    fn sweep_preserves_fresh_dir() {
        let root = sweeper_root("preserves_fresh_dir");
        let fresh = root.join("session_live");
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(fresh.join("call_1_bash.txt"), "fresh data").unwrap();

        let removed = sweep_stale_tool_result_dirs(&root, DEFAULT_TOOL_RESULT_RETENTION);
        assert_eq!(removed, 0, "fresh dir must not be touched");
        assert!(fresh.exists(), "fresh dir should survive sweep");
    }

    #[test]
    fn sweep_removes_empty_dir() {
        let root = sweeper_root("removes_empty_dir");
        let empty = root.join("session_empty");
        std::fs::create_dir_all(&empty).unwrap();
        // No files inside — empty session dir is always stale.
        let removed = sweep_stale_tool_result_dirs(&root, DEFAULT_TOOL_RESULT_RETENTION);
        assert_eq!(removed, 1);
        assert!(!empty.exists());
    }

    #[test]
    fn sweep_ignores_stray_files_at_root() {
        // We never delete root-level files — only sub-directories — so a
        // user-placed README at the root survives an aggressive cutoff.
        let root = sweeper_root("ignores_stray_files");
        let stray = root.join("README.txt");
        std::fs::write(&stray, "do not delete").unwrap();

        let removed = sweep_stale_tool_result_dirs(&root, Duration::ZERO);
        assert_eq!(removed, 0);
        assert!(stray.exists(), "stray top-level file must survive sweep");
    }

    #[test]
    fn sweep_on_missing_root_is_noop() {
        let missing = std::env::temp_dir()
            .join("aleph_test_tool_result_sweeper")
            .join("definitely_missing_xyz");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(!missing.exists());
        let removed = sweep_stale_tool_result_dirs(&missing, DEFAULT_TOOL_RESULT_RETENTION);
        assert_eq!(removed, 0);
    }
}
