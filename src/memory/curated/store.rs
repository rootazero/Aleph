//! `CuratedMemoryStore`: load → mutate → atomic write, with cross-process locking.

use crate::error::AlephError;
use crate::sync_primitives::Mutex;
use crate::utils::atomic_write::atomic_write_file;
use fs2::FileExt;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use tokio::fs;

use super::format::serialize;
use super::legacy::{load_body, ParsedLoad};

#[derive(Debug)]
pub struct CuratedMemoryStore {
    pub agent_id: String,
    pub file_path: PathBuf,
    pub char_limit: usize,
    state: Mutex<StoreState>,
    /// In-process async serialization for `with_lock`. Ensures only one task
    /// per process contends for the blocking fs2 advisory lock at a time, so
    /// the `flock` syscall stays uncontended (returns immediately) instead of
    /// blocking a tokio worker thread and starving the lock holder.
    io_gate: tokio::sync::Mutex<()>,
}

#[derive(Debug, Default, Clone)]
struct StoreState {
    entries: Vec<String>,
    legacy: bool,
}

#[derive(Debug, Clone)]
pub struct WriteOutcome {
    pub entries: Vec<String>,
    pub usage_pct: u8,
    pub usage_chars: usize,
    pub limit: usize,
    pub message: String,
    pub legacy: bool,
}

/// One operation inside an atomic batch — mirrors the single-op API surface.
/// Plain data type (no serde): the tool layer owns the wire schema.
#[derive(Debug, Clone)]
pub enum BatchOp {
    Add { content: String },
    Replace { old_text: String, content: String },
    Remove { old_text: String },
}

impl BatchOp {
    fn action(&self) -> &'static str {
        match self {
            Self::Add { .. } => "add",
            Self::Replace { .. } => "replace",
            Self::Remove { .. } => "remove",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CuratedError {
    #[error("entry already exists (no duplicate added)")]
    Duplicate,
    #[error("over budget: {used}/{limit} chars; replace or remove first")]
    OverBudget { used: usize, limit: usize },
    #[error("legacy entry detected — `add` blocked until file is curated; use `replace` or `remove` to shrink")]
    LegacyBlocked,
    #[error("no entry matched the substring `{0}`")]
    NoMatch(String),
    #[error("multiple entries matched `{0}`; provide a more specific substring")]
    Ambiguous(String),
    #[error("entry content is empty")]
    Empty,
    #[error(
        "operation {index} ({action}) failed: {reason} — no operations were applied \
         (batch is all-or-nothing)"
    )]
    BatchAborted {
        index: usize,
        action: &'static str,
        reason: String,
    },
    #[error(
        "after applying all {count} operations memory would be {used}/{limit} chars — \
         over budget; remove or shorten more entries in the same batch, then retry \
         (no operations were applied)"
    )]
    BatchOverBudget {
        count: usize,
        used: usize,
        limit: usize,
    },
    #[error("io: {0}")]
    Io(String),
}

impl From<CuratedError> for AlephError {
    fn from(e: CuratedError) -> Self {
        Self::tool(e.to_string())
    }
}

impl CuratedMemoryStore {
    /// Async constructor: read file from disk, parse as modern or legacy, return store.
    pub async fn load(
        file_path: PathBuf,
        char_limit: usize,
        agent_id: impl Into<String>,
    ) -> Result<Self, AlephError> {
        let body = if file_path.exists() {
            fs::read_to_string(&file_path)
                .await
                .map_err(|e| AlephError::tool(format!("read MEMORY.md: {e}")))?
        } else {
            String::new()
        };
        let ParsedLoad { entries, legacy } = load_body(&body);
        Ok(Self {
            agent_id: agent_id.into(),
            file_path,
            char_limit,
            state: Mutex::new(StoreState { entries, legacy }),
            io_gate: tokio::sync::Mutex::new(()),
        })
    }

    /// Snapshot of current entries (cheap clone).
    pub fn current_entries(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            // rust-doctor-disable-next-line excessive-clone
            .clone()
    }

    pub fn is_legacy(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).legacy
    }

    /// Append a new entry. Rejects: empty, exact duplicate, over budget, legacy mode.
    pub async fn add(&self, content: &str) -> Result<WriteOutcome, CuratedError> {
        let content = content.trim().to_string();
        if content.is_empty() {
            return Err(CuratedError::Empty);
        }
        self.with_lock(|st| {
            if st.legacy {
                return Err(CuratedError::LegacyBlocked);
            }
            if st.entries.iter().any(|e| e == &content) {
                return Err(CuratedError::Duplicate);
            }
            // rust-doctor-disable-next-line excessive-clone
            let mut new_entries = st.entries.clone();
            // rust-doctor-disable-next-line excessive-clone
            new_entries.push(content.clone());
            let used = super::budget::used_chars(&new_entries);
            if used > self.char_limit {
                return Err(CuratedError::OverBudget {
                    used,
                    limit: self.char_limit,
                });
            }
            st.entries = new_entries;
            Ok(())
        })
        .await?;
        Ok(self.outcome("Entry added."))
    }

    pub async fn replace(
        &self,
        old_substr: &str,
        new_content: &str,
    ) -> Result<WriteOutcome, CuratedError> {
        let old_substr = old_substr.trim();
        let new_content = new_content.trim().to_string();
        if old_substr.is_empty() {
            return Err(CuratedError::Empty);
        }
        if new_content.is_empty() {
            return Err(CuratedError::Empty);
        }
        self.with_lock(|st| {
            let idx = match_unique(&st.entries, old_substr)?;
            // rust-doctor-disable-next-line excessive-clone
            let mut new_entries = st.entries.clone();
            // rust-doctor-disable-next-line excessive-clone
            new_entries[idx] = new_content.clone();
            let used = super::budget::used_chars(&new_entries);
            if used > self.char_limit {
                return Err(CuratedError::OverBudget {
                    used,
                    limit: self.char_limit,
                });
            }
            st.entries = new_entries;
            // Replacing legacy entry de-legacys the file if the user shrinks/curates.
            if st.legacy
                && st.entries.len() == 1
                && !st.entries[0].contains(super::format::ENTRY_DELIMITER)
            {
                // Still single-entry → effectively still curated form, drop legacy flag
                st.legacy = false;
            }
            Ok(())
        })
        .await?;
        Ok(self.outcome("Entry replaced."))
    }

    pub async fn remove(&self, old_substr: &str) -> Result<WriteOutcome, CuratedError> {
        let old_substr = old_substr.trim();
        if old_substr.is_empty() {
            return Err(CuratedError::Empty);
        }
        self.with_lock(|st| {
            let idx = match_unique(&st.entries, old_substr)?;
            st.entries.remove(idx);
            if st.entries.is_empty() {
                st.legacy = false;
            }
            Ok(())
        })
        .await?;
        Ok(self.outcome("Entry removed."))
    }

    /// Apply a sequence of add/replace/remove ops atomically (all-or-nothing).
    ///
    /// Hermes `memory_tool.py::apply_batch` parity: every op is applied in
    /// order against the evolving working copy, the char budget is validated
    /// on the FINAL state only (intermediate overflow is irrelevant), and a
    /// single atomic write persists the result. This lets the model free
    /// space and add new entries in ONE call instead of the multi-turn
    /// remove→add dance. Any op failing validation rejects the whole batch
    /// (nothing written) with the 1-based op index and reason. Duplicate
    /// `add`s are idempotent: skipped, not failed.
    pub async fn apply_batch(&self, ops: &[BatchOp]) -> Result<WriteOutcome, CuratedError> {
        if ops.is_empty() {
            return Err(CuratedError::Empty);
        }
        let count = ops.len();
        self.with_lock(|st| {
            for (i, op) in ops.iter().enumerate() {
                apply_one(st, op).map_err(|e| CuratedError::BatchAborted {
                    index: i + 1,
                    action: op.action(),
                    reason: e.to_string(),
                })?;
            }
            // Budget is validated on the FINAL state only — a remove that
            // frees space for a later add within the same batch is legal.
            let used = super::budget::used_chars(&st.entries);
            if used > self.char_limit {
                return Err(CuratedError::BatchOverBudget {
                    count,
                    used,
                    limit: self.char_limit,
                });
            }
            Ok(())
        })
        .await?;
        Ok(self.outcome(&format!("Applied {count} operation(s).")))
    }

    /// D4 receipt data plane: where writes land, as a human-readable string —
    /// resolved file path (home abbreviated to `~`) plus the tier label, so
    /// the model can tell the user exactly where the memory lives.
    pub fn destination(&self) -> String {
        let path = crate::utils::paths::get_home_dir()
            .ok()
            .and_then(|home| {
                self.file_path
                    .strip_prefix(&home)
                    .ok()
                    .map(|rel| format!("~/{}", rel.display()))
            })
            .unwrap_or_else(|| self.file_path.display().to_string());
        format!(
            "{path} (curated hot zone — always in your system prompt; \
             visible from next session or compression)"
        )
    }

    fn outcome(&self, message: &str) -> WriteOutcome {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let used = super::budget::used_chars(&st.entries);
        let pct = super::budget::usage_pct(used, self.char_limit);
        WriteOutcome {
            // rust-doctor-disable-next-line excessive-clone
            entries: st.entries.clone(),
            usage_pct: pct,
            usage_chars: used,
            limit: self.char_limit,
            message: message.to_string(),
            legacy: st.legacy,
        }
    }

    /// Snapshot the current store state with an arbitrary message — used by
    /// callers (e.g. `RememberTool`) to construct a tool-result envelope for
    /// soft rejections (duplicate / over-budget / scanner-block) without
    /// raising a hard error and aborting the harness turn.
    pub fn snapshot_outcome(&self, message: impl Into<String>) -> WriteOutcome {
        self.outcome(&message.into())
    }

    /// Acquire fs2 advisory lock on a sidecar `.lock` file, re-read disk into
    /// state, run the mutator, write atomically, release lock.
    async fn with_lock<F>(&self, mutate: F) -> Result<(), CuratedError>
    where
        F: FnOnce(&mut StoreState) -> Result<(), CuratedError>,
    {
        // Serialize in-process callers first: with only one task per process
        // reaching the blocking `flock` below, the syscall is uncontended and
        // returns immediately rather than parking a tokio worker thread.
        let _gate = self.io_gate.lock().await;
        let lock_path = lock_sidecar(&self.file_path);
        if let Some(parent) = lock_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| CuratedError::Io(format!("mkdir {}: {e}", parent.display())))?;
        }
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|e| CuratedError::Io(format!("open lock {}: {e}", lock_path.display())))?;
        lock_file
            .lock_exclusive()
            .map_err(|e| CuratedError::Io(format!("acquire lock: {e}")))?;
        let result = self.with_lock_inner(mutate).await;
        if let Err(e) = FileExt::unlock(&lock_file) {
            tracing::warn!(error = %e, "unlock failed");
        }
        result
    }

    async fn with_lock_inner<F>(&self, mutate: F) -> Result<(), CuratedError>
    where
        F: FnOnce(&mut StoreState) -> Result<(), CuratedError>,
    {
        // Re-read disk under lock to pick up writes from other processes.
        let body = if self.file_path.exists() {
            tokio::fs::read_to_string(&self.file_path)
                .await
                .map_err(|e| CuratedError::Io(format!("read: {e}")))?
        } else {
            String::new()
        };
        let ParsedLoad {
            entries,
            legacy: disk_legacy,
        } = load_body(&body);
        // With the trailing-`\n§\n` sentinel emitted by `format::serialize`,
        // `load_body` is now correct on any reload (single-process or multi-
        // process): a curated file always contains the delimiter, a legacy
        // file never does. Trust disk.
        let mut working = StoreState {
            entries,
            legacy: disk_legacy,
        };
        mutate(&mut working)?;
        // Write back.
        let body = serialize(&working.entries);
        if let Some(parent) = self.file_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| CuratedError::Io(format!("mkdir {}: {e}", parent.display())))?;
        }
        atomic_write_file(&self.file_path, &body)
            .await
            .map_err(|e| CuratedError::Io(format!("atomic write: {e}")))?;
        // Update in-memory state.
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *st = working;
        Ok(())
    }
}

/// Find the single entry containing `old_substr`. Errors on no match, or when
/// the substring matches multiple distinct entries.
fn match_unique(entries: &[String], old_substr: &str) -> Result<usize, CuratedError> {
    let matches: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.contains(old_substr))
        .map(|(i, _)| i)
        .collect();
    if matches.is_empty() {
        return Err(CuratedError::NoMatch(old_substr.to_string()));
    }
    if matches.len() > 1 {
        let unique: std::collections::HashSet<_> = matches.iter().map(|&i| &entries[i]).collect();
        if unique.len() > 1 {
            return Err(CuratedError::Ambiguous(old_substr.to_string()));
        }
    }
    Ok(matches[0])
}

/// Apply one batch op against the evolving working state. No budget check
/// here — `apply_batch` validates the final state only.
fn apply_one(st: &mut StoreState, op: &BatchOp) -> Result<(), CuratedError> {
    match op {
        BatchOp::Add { content } => {
            let content = content.trim();
            if content.is_empty() {
                return Err(CuratedError::Empty);
            }
            if st.legacy {
                return Err(CuratedError::LegacyBlocked);
            }
            // Idempotent: a duplicate add is skipped, not a batch failure.
            if !st.entries.iter().any(|e| e == content) {
                st.entries.push(content.to_string());
            }
            Ok(())
        }
        BatchOp::Replace { old_text, content } => {
            let old_text = old_text.trim();
            let content = content.trim();
            if old_text.is_empty() || content.is_empty() {
                return Err(CuratedError::Empty);
            }
            let idx = match_unique(&st.entries, old_text)?;
            st.entries[idx] = content.to_string();
            // Replacing legacy entry de-legacys the file if the user shrinks/curates.
            if st.legacy
                && st.entries.len() == 1
                && !st.entries[0].contains(super::format::ENTRY_DELIMITER)
            {
                st.legacy = false;
            }
            Ok(())
        }
        BatchOp::Remove { old_text } => {
            let old_text = old_text.trim();
            if old_text.is_empty() {
                return Err(CuratedError::Empty);
            }
            let idx = match_unique(&st.entries, old_text)?;
            st.entries.remove(idx);
            if st.entries.is_empty() {
                st.legacy = false;
            }
            Ok(())
        }
    }
}

fn lock_sidecar(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".lock");
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn fresh(dir: &Path, limit: usize) -> CuratedMemoryStore {
        CuratedMemoryStore::load(dir.join("MEMORY.md"), limit, "test-agent")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn add_to_empty_succeeds() {
        let d = tempdir().unwrap();
        let s = fresh(d.path(), 100).await;
        let r = s.add("hello").await.unwrap();
        assert_eq!(r.entries, vec!["hello"]);
        assert!(!r.legacy);
        // "hello" + trailing "\n§\n" sentinel = 5 + 4 = 9 chars used.
        assert_eq!(r.usage_chars, 9);
    }

    #[tokio::test]
    async fn add_rejects_duplicate() {
        let d = tempdir().unwrap();
        let s = fresh(d.path(), 100).await;
        s.add("hello").await.unwrap();
        let err = s.add("hello").await.unwrap_err();
        assert!(matches!(err, CuratedError::Duplicate));
    }

    #[tokio::test]
    async fn add_rejects_over_budget() {
        let d = tempdir().unwrap();
        let s = fresh(d.path(), 10).await;
        s.add("12345").await.unwrap(); // 5 chars
        let err = s.add("12345678").await.unwrap_err(); // 5 + 4 (\n§\n) + 8 = 17 > 10
        assert!(matches!(err, CuratedError::OverBudget { .. }));
    }

    #[tokio::test]
    async fn replace_substring_uniquely() {
        let d = tempdir().unwrap();
        let s = fresh(d.path(), 200).await;
        s.add("Alice prefers tabs").await.unwrap();
        s.add("Bob prefers spaces").await.unwrap();
        let r = s
            .replace("Alice", "Alice prefers two-space indent")
            .await
            .unwrap();
        assert!(r.entries[0].contains("two-space"));
        assert!(r.entries[1].contains("Bob"));
    }

    #[tokio::test]
    async fn replace_rejects_ambiguous() {
        let d = tempdir().unwrap();
        let s = fresh(d.path(), 200).await;
        s.add("a x b").await.unwrap();
        s.add("c x d").await.unwrap();
        let err = s.replace("x", "y").await.unwrap_err();
        assert!(matches!(err, CuratedError::Ambiguous(_)));
    }

    #[tokio::test]
    async fn remove_substring() {
        let d = tempdir().unwrap();
        let s = fresh(d.path(), 200).await;
        s.add("keep me").await.unwrap();
        s.add("delete me").await.unwrap();
        let r = s.remove("delete").await.unwrap();
        assert_eq!(r.entries, vec!["keep me"]);
    }

    #[tokio::test]
    async fn legacy_blocks_add_but_allows_remove() {
        let d = tempdir().unwrap();
        let path = d.path().join("MEMORY.md");
        tokio::fs::write(&path, "# legacy\n## free markdown\n- a\n- b\n")
            .await
            .unwrap();
        let s = CuratedMemoryStore::load(path.clone(), 200, "agent")
            .await
            .unwrap();
        assert!(s.is_legacy());
        let err = s.add("new").await.unwrap_err();
        assert!(matches!(err, CuratedError::LegacyBlocked));
        // Remove the legacy entry → file becomes non-legacy and empty.
        let _ = s.remove("legacy").await.unwrap();
        assert!(!s.is_legacy());
        assert!(s.current_entries().is_empty());
    }

    #[tokio::test]
    async fn batch_budget_checked_on_final_state_only() {
        let d = tempdir().unwrap();
        let s = fresh(d.path(), 60).await;
        let old = "x".repeat(40);
        s.add(&old).await.unwrap();
        let new = "y".repeat(45);
        // A plain add cannot fit alongside the old entry…
        let err = s.add(&new).await.unwrap_err();
        assert!(matches!(err, CuratedError::OverBudget { .. }));
        // …but one batch frees the space and adds in a single atomic write.
        let r = s
            .apply_batch(&[
                BatchOp::Remove {
                    old_text: old.clone(),
                },
                BatchOp::Add {
                    content: new.clone(),
                },
            ])
            .await
            .unwrap();
        assert_eq!(r.entries, vec![new]);
        assert!(r.message.contains("Applied 2 operation(s)"));
    }

    #[tokio::test]
    async fn batch_is_all_or_nothing() {
        let d = tempdir().unwrap();
        let path = d.path().join("MEMORY.md");
        let s = CuratedMemoryStore::load(path.clone(), 200, "agent")
            .await
            .unwrap();
        s.add("keep me").await.unwrap();
        let err = s
            .apply_batch(&[
                BatchOp::Add {
                    content: "should not land".into(),
                },
                BatchOp::Remove {
                    old_text: "ghost".into(),
                },
            ])
            .await
            .unwrap_err();
        match err {
            CuratedError::BatchAborted { index, action, .. } => {
                assert_eq!(index, 2, "1-based failing op index");
                assert_eq!(action, "remove");
            }
            other => panic!("expected BatchAborted, got {other}"),
        }
        // Nothing from the batch was applied — including the valid first op.
        assert_eq!(s.current_entries(), vec!["keep me"]);
        // Disk untouched too.
        let s2 = CuratedMemoryStore::load(path, 200, "agent").await.unwrap();
        assert_eq!(s2.current_entries(), vec!["keep me"]);
    }

    #[tokio::test]
    async fn batch_over_budget_rejects_whole_batch() {
        let d = tempdir().unwrap();
        let s = fresh(d.path(), 20).await;
        let err = s
            .apply_batch(&[BatchOp::Add {
                content: "this content is way too long for the tiny budget".into(),
            }])
            .await
            .unwrap_err();
        assert!(matches!(err, CuratedError::BatchOverBudget { .. }));
        assert!(s.current_entries().is_empty());
    }

    #[tokio::test]
    async fn batch_duplicate_add_is_idempotent_skip() {
        let d = tempdir().unwrap();
        let s = fresh(d.path(), 200).await;
        s.add("already here").await.unwrap();
        let r = s
            .apply_batch(&[
                BatchOp::Add {
                    content: "already here".into(),
                },
                BatchOp::Add {
                    content: "brand new".into(),
                },
            ])
            .await
            .expect("duplicate add inside a batch is skipped, not failed");
        assert_eq!(r.entries, vec!["already here", "brand new"]);
    }

    #[tokio::test]
    async fn batch_can_curate_legacy_file_in_one_call() {
        let d = tempdir().unwrap();
        let path = d.path().join("MEMORY.md");
        std::fs::write(&path, "# legacy\n## free markdown\n- a\n- b\n").unwrap();
        let s = CuratedMemoryStore::load(path, 200, "agent").await.unwrap();
        assert!(s.is_legacy());
        // Ops apply in order against the evolving state: removing the legacy
        // blob clears the legacy flag, so the following add is allowed.
        let r = s
            .apply_batch(&[
                BatchOp::Remove {
                    old_text: "legacy".into(),
                },
                BatchOp::Add {
                    content: "curated fact".into(),
                },
            ])
            .await
            .unwrap();
        assert_eq!(r.entries, vec!["curated fact"]);
        assert!(!r.legacy);
    }

    #[tokio::test]
    async fn destination_names_file_and_tier() {
        let d = tempdir().unwrap();
        let s = fresh(d.path(), 100).await;
        let dest = s.destination();
        assert!(dest.contains("MEMORY.md"), "dest was {dest}");
        assert!(dest.contains("curated hot zone"));
    }

    #[tokio::test]
    async fn write_persists_atomically() {
        let d = tempdir().unwrap();
        let path = d.path().join("MEMORY.md");
        let s = CuratedMemoryStore::load(path.clone(), 100, "agent")
            .await
            .unwrap();
        s.add("durable").await.unwrap();
        // Reload from disk in a fresh store; entry should survive.
        let s2 = CuratedMemoryStore::load(path, 100, "agent").await.unwrap();
        assert_eq!(s2.current_entries(), vec!["durable"]);
    }
}
