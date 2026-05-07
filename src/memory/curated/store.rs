//! CuratedMemoryStore: load → mutate → atomic write, with cross-process locking.

use crate::error::AlephError;
use crate::utils::atomic_write::atomic_write_file;
use fs2::FileExt;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use crate::sync_primitives::Mutex;
use tokio::fs;

use super::format::serialize;
use super::legacy::{load_body, ParsedLoad};

#[derive(Debug)]
pub struct CuratedMemoryStore {
    pub agent_id: String,
    pub file_path: PathBuf,
    pub char_limit: usize,
    state: Mutex<StoreState>,
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
    #[error("io: {0}")]
    Io(String),
}

impl From<CuratedError> for AlephError {
    fn from(e: CuratedError) -> Self {
        AlephError::tool(e.to_string())
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
        })
    }

    /// Snapshot of current entries (cheap clone).
    pub fn current_entries(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
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
            let mut new_entries = st.entries.clone();
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
            let matches: Vec<usize> = st
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.contains(old_substr))
                .map(|(i, _)| i)
                .collect();
            if matches.is_empty() {
                return Err(CuratedError::NoMatch(old_substr.to_string()));
            }
            if matches.len() > 1 {
                let unique: std::collections::HashSet<_> =
                    matches.iter().map(|&i| &st.entries[i]).collect();
                if unique.len() > 1 {
                    return Err(CuratedError::Ambiguous(old_substr.to_string()));
                }
            }
            let idx = matches[0];
            let mut new_entries = st.entries.clone();
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
            let matches: Vec<usize> = st
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.contains(old_substr))
                .map(|(i, _)| i)
                .collect();
            if matches.is_empty() {
                return Err(CuratedError::NoMatch(old_substr.to_string()));
            }
            if matches.len() > 1 {
                let unique: std::collections::HashSet<_> =
                    matches.iter().map(|&i| &st.entries[i]).collect();
                if unique.len() > 1 {
                    return Err(CuratedError::Ambiguous(old_substr.to_string()));
                }
            }
            st.entries.remove(matches[0]);
            if st.entries.is_empty() {
                st.legacy = false;
            }
            Ok(())
        })
        .await?;
        Ok(self.outcome("Entry removed."))
    }

    fn outcome(&self, message: &str) -> WriteOutcome {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let used = super::budget::used_chars(&st.entries);
        let pct = super::budget::usage_pct(used, self.char_limit);
        WriteOutcome {
            entries: st.entries.clone(),
            usage_pct: pct,
            usage_chars: used,
            limit: self.char_limit,
            message: message.to_string(),
            legacy: st.legacy,
        }
    }

    /// Snapshot the current store state with an arbitrary message — used by
    /// callers (e.g. RememberTool) to construct a tool-result envelope for
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
        let lock_path = lock_sidecar(&self.file_path);
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)
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
        let _ = FileExt::unlock(&lock_file);
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
            std::fs::create_dir_all(parent)
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
        std::fs::write(&path, "# legacy\n## free markdown\n- a\n- b\n").unwrap();
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
