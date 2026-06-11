//! Shell-hook consent allowlist.
//!
//! Shell-command hooks (`HookAction::Command`) execute arbitrary code in the
//! agent's environment. Hermes-inspired: before such a hook runs, its command
//! must be explicitly approved by the operator. Un-approved shell hooks are
//! skipped (fail-safe) and recorded as `pending` so the operator can review
//! them via `aleph hooks list` / `aleph hooks test`.
//!
//! Registry file: `~/.aleph/shell-hooks-allowlist.json`. The file's
//! `(mtime, len)` pair is the cache fingerprint — `is_approved` re-reads the
//! registry whenever the file changes on disk, so an approval made via the
//! `aleph hooks` CLI is picked up by a running server without a restart.

use crate::sync_primitives::{Arc, RwLock};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

/// On-disk registry schema version.
const REGISTRY_VERSION: u32 = 1;

/// Cheap change-detection fingerprint for the registry file: `(mtime, len)`.
/// Length is included because some filesystems have coarse mtime resolution —
/// an approval that lands in the same second still changes the file length.
type FileStamp = Option<(SystemTime, u64)>;

/// Consent status of a shell-hook command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsentStatus {
    /// Seen by the server but awaiting operator approval. The hook is NOT run.
    Pending,
    /// Operator-approved. The hook runs normally.
    Approved,
}

/// A single shell-hook consent record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentEntry {
    /// Stable id: `sha256(plugin_name \0 command)` truncated to 16 hex chars.
    /// Changes when the command text changes — editing a hook revokes consent.
    pub fingerprint: String,
    /// Plugin that registered the hook.
    pub plugin_name: String,
    /// The exact shell command string.
    pub command: String,
    /// Hook event the command is bound to (display-only, best-effort).
    #[serde(default)]
    pub event: String,
    /// Consent status.
    pub status: ConsentStatus,
    /// Unix seconds when the record was first created.
    pub first_seen: u64,
    /// Unix seconds when approved (absent while pending).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<u64>,
}

#[derive(Serialize, Deserialize, Default)]
struct RegistryDoc {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: Vec<ConsentEntry>,
}

struct CacheState {
    entries: BTreeMap<String, ConsentEntry>,
    /// Stamp of the registry file the cache was loaded from.
    stamp: FileStamp,
}

/// Manages the shell-hook consent allowlist on disk.
pub struct ShellHookConsent {
    path: PathBuf,
    cache: RwLock<CacheState>,
}

impl ShellHookConsent {
    /// Compute the stable fingerprint for a `(plugin_name, command)` pair.
    #[must_use]
    pub fn fingerprint(plugin_name: &str, command: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(plugin_name.as_bytes());
        hasher.update([0u8]);
        hasher.update(command.as_bytes());
        hex16(&hasher.finalize())
    }

    /// Default registry path: `~/.aleph/shell-hooks-allowlist.json`.
    #[must_use]
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("shell-hooks-allowlist.json")
    }

    /// Process-wide consent instance backed by the default path.
    pub fn shared() -> Arc<Self> {
        static SHARED: OnceLock<Arc<ShellHookConsent>> = OnceLock::new();
        SHARED
            .get_or_init(|| Arc::new(Self::with_path(Self::default_path())))
            .clone()
    }

    /// Construct a consent manager backed by an explicit path (tests).
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (entries, stamp) = Self::read_file(&path);
        Self {
            path,
            cache: RwLock::new(CacheState { entries, stamp }),
        }
    }

    /// Registry file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether a `(plugin_name, command)` shell hook is approved to run.
    ///
    /// Re-reads the registry first if the file changed on disk, so approvals
    /// made via the `aleph hooks` CLI are honored without a server restart.
    pub fn is_approved(&self, plugin_name: &str, command: &str) -> bool {
        self.reload_if_stale();
        let fp = Self::fingerprint(plugin_name, command);
        self.cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .get(&fp)
            .is_some_and(|e| e.status == ConsentStatus::Approved)
    }

    /// Record an un-approved shell hook as `pending` so the operator can
    /// review it. No-op when the fingerprint is already known. Best-effort:
    /// a write failure is logged, not propagated.
    pub fn record_pending(&self, plugin_name: &str, command: &str, event: &str) {
        let fp = Self::fingerprint(plugin_name, command);
        {
            let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
            if cache.entries.contains_key(&fp) {
                return;
            }
        }
        let entry = ConsentEntry {
            fingerprint: fp.clone(),
            plugin_name: plugin_name.to_string(),
            command: command.to_string(),
            event: event.to_string(),
            status: ConsentStatus::Pending,
            first_seen: now_secs(),
            approved_at: None,
        };
        if let Err(e) = self.mutate(|entries| {
            if entries.contains_key(&fp) {
                false
            } else {
                entries.insert(fp.clone(), entry);
                true
            }
        }) {
            warn!(error = %e, "Failed to record pending shell-hook consent");
        }
    }

    /// Approve a hook by fingerprint (or unique prefix). Returns the approved
    /// entry, or `None` when no entry matches the prefix.
    pub fn approve(&self, fingerprint_prefix: &str) -> io::Result<Option<ConsentEntry>> {
        let mut approved: Option<ConsentEntry> = None;
        self.mutate(|entries| {
            let Some(key) = match_prefix(entries, fingerprint_prefix) else {
                return false;
            };
            match entries.get_mut(&key) {
                Some(entry) => {
                    entry.status = ConsentStatus::Approved;
                    entry.approved_at = Some(now_secs());
                    approved = Some(entry.clone());
                    true
                }
                None => false,
            }
        })?;
        Ok(approved)
    }

    /// Revoke consent for a hook (sets it back to `pending`). Returns the
    /// affected entry, or `None` when no entry matches the prefix.
    pub fn revoke(&self, fingerprint_prefix: &str) -> io::Result<Option<ConsentEntry>> {
        let mut revoked: Option<ConsentEntry> = None;
        self.mutate(|entries| {
            let Some(key) = match_prefix(entries, fingerprint_prefix) else {
                return false;
            };
            match entries.get_mut(&key) {
                Some(entry) => {
                    entry.status = ConsentStatus::Pending;
                    entry.approved_at = None;
                    revoked = Some(entry.clone());
                    true
                }
                None => false,
            }
        })?;
        Ok(revoked)
    }

    /// Revoke every approved hook (all back to `pending`). Returns the count.
    pub fn revoke_all(&self) -> io::Result<usize> {
        let mut count = 0usize;
        self.mutate(|entries| {
            for entry in entries.values_mut() {
                if entry.status == ConsentStatus::Approved {
                    entry.status = ConsentStatus::Pending;
                    entry.approved_at = None;
                    count += 1;
                }
            }
            count > 0
        })?;
        Ok(count)
    }

    /// All consent entries, fingerprint-sorted. Re-reads if the file changed.
    pub fn entries(&self) -> Vec<ConsentEntry> {
        self.reload_if_stale();
        self.cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .values()
            .cloned()
            .collect()
    }

    /// Look up a single entry by fingerprint prefix.
    pub fn find(&self, fingerprint_prefix: &str) -> Option<ConsentEntry> {
        self.reload_if_stale();
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        match_prefix(&cache.entries, fingerprint_prefix)
            .and_then(|k| cache.entries.get(&k).cloned())
    }

    // ---- internals --------------------------------------------------------

    /// Read + parse the registry file. Missing file ⇒ empty. A parse error is
    /// logged and treated as empty (defensive — a corrupt file must not crash
    /// the server, and re-recording pending hooks self-heals it).
    fn read_file(path: &Path) -> (BTreeMap<String, ConsentEntry>, FileStamp) {
        let stamp = file_stamp(path);
        let raw = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => return (BTreeMap::new(), stamp),
        };
        match serde_json::from_slice::<RegistryDoc>(&raw) {
            Ok(doc) => {
                let map = doc
                    .entries
                    .into_iter()
                    .map(|e| (e.fingerprint.clone(), e))
                    .collect();
                (map, stamp)
            }
            Err(e) => {
                warn!(
                    error = %e,
                    path = %path.display(),
                    "Corrupt shell-hook consent registry — treating as empty"
                );
                (BTreeMap::new(), stamp)
            }
        }
    }

    /// Reload the in-memory cache if the file's `(mtime, len)` stamp changed.
    fn reload_if_stale(&self) {
        let disk_stamp = file_stamp(&self.path);
        {
            let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
            if cache.stamp == disk_stamp {
                return;
            }
        }
        let (entries, stamp) = Self::read_file(&self.path);
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        cache.entries = entries;
        cache.stamp = stamp;
    }

    /// Cross-process-safe read-modify-write: take an exclusive file lock,
    /// re-read the on-disk registry (another process may have changed it),
    /// apply `f`, persist atomically when `f` reports a change, then refresh
    /// the in-memory cache.
    fn mutate<F>(&self, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut BTreeMap<String, ConsentEntry>) -> bool,
    {
        use fs2::FileExt;

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock_path = self.path.with_extension("lock");
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        lock.lock_exclusive()?;

        let (mut entries, _) = Self::read_file(&self.path);
        if f(&mut entries) {
            let doc = RegistryDoc {
                version: REGISTRY_VERSION,
                entries: entries.values().cloned().collect(),
            };
            let bytes = serde_json::to_vec_pretty(&doc)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            let tmp = self.path.with_extension("json.tmp");
            fs::write(&tmp, &bytes)?;
            fs::rename(&tmp, &self.path)?;
        }

        {
            let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
            cache.entries = entries;
            cache.stamp = file_stamp(&self.path);
        }
        let _ = FileExt::unlock(&lock);
        Ok(())
    }
}

/// Find the single registry key matching a fingerprint prefix. Returns `None`
/// when there is no match or the prefix is ambiguous (matches >1 entry).
fn match_prefix(entries: &BTreeMap<String, ConsentEntry>, prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return None;
    }
    if entries.contains_key(prefix) {
        return Some(prefix.to_string());
    }
    let mut matches = entries.keys().filter(|k| k.starts_with(prefix));
    let first = matches.next()?.clone();
    match matches.next() {
        Some(_) => None, // ambiguous
        None => Some(first),
    }
}

fn file_stamp(path: &Path) -> FileStamp {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    Some((mtime, meta.len()))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn hex16(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(16);
    for b in bytes.iter().take(8) {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_consent() -> (tempfile::TempDir, ShellHookConsent) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shell-hooks-allowlist.json");
        let consent = ShellHookConsent::with_path(path);
        (dir, consent)
    }

    #[test]
    fn fingerprint_is_stable_and_command_sensitive() {
        let a = ShellHookConsent::fingerprint("plug", "echo hi");
        let b = ShellHookConsent::fingerprint("plug", "echo hi");
        let c = ShellHookConsent::fingerprint("plug", "echo HI");
        let d = ShellHookConsent::fingerprint("other", "echo hi");
        assert_eq!(a, b);
        assert_ne!(a, c, "command change must change the fingerprint");
        assert_ne!(a, d, "plugin change must change the fingerprint");
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn unknown_hook_is_not_approved() {
        let (_d, consent) = tmp_consent();
        assert!(!consent.is_approved("p", "rm -rf /"));
    }

    #[test]
    fn record_pending_then_approve_round_trip() {
        let (_d, consent) = tmp_consent();
        consent.record_pending("p", "echo hi", "after_tool_call");
        assert!(!consent.is_approved("p", "echo hi"), "pending != approved");

        let entries = consent.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, ConsentStatus::Pending);
        let fp = entries[0].fingerprint.clone();

        let approved = consent.approve(&fp).expect("approve").expect("entry");
        assert_eq!(approved.status, ConsentStatus::Approved);
        assert!(consent.is_approved("p", "echo hi"), "approved hook runs");
    }

    #[test]
    fn record_pending_is_idempotent() {
        let (_d, consent) = tmp_consent();
        consent.record_pending("p", "echo hi", "after_tool_call");
        consent.record_pending("p", "echo hi", "after_tool_call");
        assert_eq!(consent.entries().len(), 1);
    }

    #[test]
    fn revoke_sends_approved_hook_back_to_pending() {
        let (_d, consent) = tmp_consent();
        consent.record_pending("p", "echo hi", "before_tool_call");
        let fp = consent.entries()[0].fingerprint.clone();
        consent.approve(&fp).unwrap();
        assert!(consent.is_approved("p", "echo hi"));

        let revoked = consent.revoke(&fp).expect("revoke").expect("entry");
        assert_eq!(revoked.status, ConsentStatus::Pending);
        assert!(!consent.is_approved("p", "echo hi"));
    }

    #[test]
    fn approve_by_prefix_resolves_unique_match() {
        let (_d, consent) = tmp_consent();
        consent.record_pending("p", "echo hi", "e");
        let fp = consent.entries()[0].fingerprint.clone();
        let approved = consent.approve(fp.get(..6).unwrap_or(&fp)).expect("approve").expect("entry");
        assert_eq!(approved.fingerprint, fp);
    }

    #[test]
    fn approve_unknown_prefix_returns_none() {
        let (_d, consent) = tmp_consent();
        assert!(consent.approve("ffffffff").expect("approve").is_none());
    }

    #[test]
    fn changes_persist_across_reload() {
        let (_d, consent) = tmp_consent();
        let path = consent.path().to_path_buf();
        consent.record_pending("p", "echo hi", "e");
        let fp = consent.entries()[0].fingerprint.clone();
        consent.approve(&fp).unwrap();

        // A fresh instance reads the same file from disk.
        let reopened = ShellHookConsent::with_path(path);
        assert!(reopened.is_approved("p", "echo hi"));
    }

    #[test]
    fn external_file_change_is_picked_up_via_stamp() {
        let (_d, consent) = tmp_consent();
        let path = consent.path().to_path_buf();
        consent.record_pending("p", "echo hi", "e");
        assert!(!consent.is_approved("p", "echo hi"));

        // A second process approves the hook by writing the file directly;
        // the approval grows the file, so the `(mtime, len)` stamp differs.
        let writer = ShellHookConsent::with_path(&path);
        let fp = writer.entries()[0].fingerprint.clone();
        writer.approve(&fp).unwrap();

        assert!(
            consent.is_approved("p", "echo hi"),
            "is_approved must reload after the registry file changes"
        );
    }
}
