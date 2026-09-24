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
//!
//! # What an approval is bound to
//!
//! An entry is keyed by `(owner, project, command)` — see
//! [`ShellHookConsent::fingerprint`]. `project` is the canonical root of the
//! project the hook is bound to: its `ScopeKey::Project`, the same field the
//! executor's fire-time gate (`project_scope_allows`) reads.
//!
//! - A hook that fires everywhere (`ScopeKey::Global`: `~/.aleph/hooks.json`,
//!   a globally installed plugin) has no project and keeps the key it has
//!   always had. Its code lives under its root, and the approval is bound
//!   to that root (below), so the same plugin id installed elsewhere is not
//!   approved by it.
//! - A project hook (a project's `.aleph/hooks{,.local}.json`, a plugin found
//!   under a project) is keyed by its project as well. Every project's files
//!   load under the same `user:project` label, and a byte-identical template
//!   in another repo resolves `${CLAUDE_PLUGIN_ROOT}` to THAT repo's
//!   directory: approving `"${CLAUDE_PLUGIN_ROOT}"/hooks/lint.sh` in repo A
//!   must not run repo B's `lint.sh`. A project-scoped plugin is the same
//!   case — another repo can ship a plugin with the same id.
//!
//! Within its key, an approval also attests to what `aleph hooks test`
//! reviewed: the hook's root directory ([`ConsentEntry::plugin_root`]) and,
//! when the command names one, the script's content. The same key from
//! another root, or an edited script, is refused until it is revoked and
//! reviewed again ([`ShellHookConsent::is_approved`]). A pending entry is
//! refreshed by every fire, so what the review runs is what production runs.
//!
//! **Migration.** An entry recorded before the project binding carries no
//! project. It is kept on disk as it is — never rewritten, never deleted —
//! but a project hook is never looked up under it, so it authorises
//! nothing: each project's hooks come back as `pending` once and are
//! approved again, per project. `aleph hooks list` / `test` mark such a
//! `user:project*` entry ([`ConsentEntry::predates_project_binding`]). An
//! approval of a hook that fires everywhere, recorded before roots were
//! recorded, keeps working from any root until it is revoked and approved
//! again — which records and binds its root.

use crate::extension::visibility::{canonical_root, ScopeKey};
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
    /// Stable id: the `sha256` of `(plugin_name, project_root, command)`
    /// ([`ShellHookConsent::fingerprint`]) truncated to 16 hex chars.
    /// Changes when the command text changes — editing a hook revokes consent.
    pub fingerprint: String,
    /// Plugin that registered the hook.
    pub plugin_name: String,
    /// The project this entry is bound to: the canonical root of a hook that
    /// fires only inside one project. Part of the fingerprint, so approving
    /// a template in one project never approves it in another. `None` for a
    /// hook that fires everywhere — and for every entry recorded before
    /// approvals were bound to a project (see the module doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    /// The exact shell command string.
    pub command: String,
    /// The event name the hook is dispatched with — its registered spelling
    /// (`PreToolUse`, `before_tool_call`), exactly what its payload's
    /// `hook_event_name` carries — so `aleph hooks test` can hand it the same
    /// value. Best-effort: one entry per `(plugin, project, command)`, so a
    /// command bound to two events carries whichever fired last while it is
    /// pending ([`ShellHookConsent::record_pending`] refreshes it), and the
    /// one it was approved with afterwards. An entry recorded before this
    /// field held the spelling carries the enum's Rust name
    /// (`BeforeToolCall`) until it next fires while pending.
    #[serde(default)]
    pub event: String,
    /// The hook's plugin root, which its path variables
    /// (`${CLAUDE_PLUGIN_ROOT}` …) resolve to, so `aleph hooks test` runs the
    /// command as production does. Once approved, it is the directory the
    /// approval is bound to: the same key from another root is refused
    /// ([`ShellHookConsent::is_approved`]). While pending, the latest fire's.
    /// `None` for entries recorded before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_root: Option<PathBuf>,
    /// Consent status.
    pub status: ConsentStatus,
    /// Unix seconds when the record was first created.
    pub first_seen: u64,
    /// Unix seconds when approved (absent while pending).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<u64>,
    /// `sha256` of the script file the command invokes, captured when the
    /// entry was recorded / approved.
    ///
    /// The command STRING alone is a weak thing to consent to: approving
    /// `sh scripts/deploy.sh` once approves whatever that file contains
    /// forever, so an attacker who can write the script (or a `git pull`)
    /// silently inherits the approval. Binding the content closes that
    /// time-of-check/time-of-use window — [`ShellHookConsent::is_approved`]
    /// re-hashes and refuses on drift.
    ///
    /// `None` for commands with no resolvable script (`echo hi`), for
    /// unreadable files, and for entries written before this field existed —
    /// all of which keep the previous command-string-only semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_fingerprint: Option<String>,
}

impl ConsentEntry {
    /// The fingerprint this entry's own key derives — what `fingerprint`
    /// must equal unless the registry was edited by hand.
    #[must_use]
    pub fn expected_fingerprint(&self) -> String {
        ShellHookConsent::fingerprint(
            &self.plugin_name,
            self.project_root.as_deref(),
            &self.command,
        )
    }

    /// Whether this is a project-settings entry recorded before approvals
    /// were bound to a project: a `user:project*` owner with no project. It
    /// authorises nothing — no project hook is looked up without its
    /// project. A project-scoped plugin's entry from that time cannot be
    /// told apart from a global plugin's, so it is not marked (it is just as
    /// inert).
    #[must_use]
    pub fn predates_project_binding(&self) -> bool {
        self.project_root.is_none()
            && super::user_settings::PROJECT_LABELS.contains(&self.plugin_name.as_str())
    }
}

/// The project a consent key names: the root of a hook bound to one project,
/// `None` for a hook that fires everywhere.
fn bound_project(scope: &ScopeKey) -> Option<&Path> {
    match scope {
        ScopeKey::Global => None,
        ScopeKey::Project(root) => Some(root),
    }
}

/// Whether two spellings name one directory: equal as written, or once
/// canonicalised (`/var` → `/private/var`) the way the scope key is.
fn same_dir(a: &Path, b: &Path) -> bool {
    a == b || canonical_root(a) == canonical_root(b)
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
    /// Compute the stable fingerprint for a `(plugin_name, project_root,
    /// command)` key. `project_root` is the canonical root of a hook bound to
    /// one project, `None` for a hook that fires everywhere.
    ///
    /// Without a project the hashed bytes are `plugin_name \0 command` — the
    /// key every entry had before approvals were bound to a project, so a
    /// global hook's approval survives. With one, a `0xFF` tag and the
    /// length-prefixed root come first: no global key starts with `0xFF` (a
    /// plugin name is UTF-8), so a project key can never equal a global one,
    /// and the prefix keeps one root from running into the name after it.
    #[must_use]
    pub fn fingerprint(plugin_name: &str, project_root: Option<&Path>, command: &str) -> String {
        let mut hasher = Sha256::new();
        if let Some(root) = project_root {
            let root = root.as_os_str().as_encoded_bytes();
            hasher.update([0xFFu8]);
            hasher.update((root.len() as u64).to_le_bytes());
            hasher.update(root);
        }
        hasher.update(plugin_name.as_bytes());
        hasher.update([0u8]);
        hasher.update(command.as_bytes());
        hex16(&hasher.finalize())
    }

    /// Default registry path: `<config_dir>/shell-hooks-allowlist.json`.
    ///
    /// Resolved through `utils::paths::get_config_dir` like every other piece
    /// of Aleph state, so it follows `ALEPH_HOME`. The former hand-rolled
    /// `dirs::home_dir().join(".aleph")` did not: under a relocated home the
    /// approval registry — and the `hooks/consent` doctor check that reads it
    /// — silently addressed the developer's real `~/.aleph` instead. Identical
    /// bytes when `ALEPH_HOME` is unset.
    #[must_use]
    pub fn default_path() -> PathBuf {
        crate::utils::paths::get_config_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
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

    /// Whether a shell hook is approved to run: `command`, owned by
    /// `plugin_name`, from a hook whose visibility key is `scope` — the
    /// hook's own `scope_key`, so a project hook is looked up under its
    /// project and nowhere else (module doc).
    ///
    /// Re-reads the registry first if the file changed on disk, so approvals
    /// made via the `aleph hooks` CLI are honored without a server restart.
    ///
    /// Three conditions, all required:
    ///
    /// 1. an `Approved` entry exists for that key,
    /// 2. if that entry recorded a [`plugin_root`](ConsentEntry::plugin_root),
    ///    it is the directory this hook runs from (`plugin_root`), and
    /// 3. if that entry recorded a [`script_fingerprint`](ConsentEntry::script_fingerprint),
    ///    the script on disk still hashes to it.
    ///
    /// (2) binds the approval to the root `aleph hooks test` reviewed: the
    /// same key from another directory — a plugin id installed again
    /// elsewhere — runs other code behind `${CLAUDE_PLUGIN_ROOT}`. (3) is the
    /// TOCTOU guard: `sh scripts/deploy.sh` approved in March must not keep
    /// running after the script is rewritten in April. Either drift fails
    /// SAFE (the hook is skipped, exactly like an un-approved one) and is
    /// logged loudly, because the alternative — running code nobody reviewed
    /// — is the whole thing consent exists to prevent. The way back is
    /// `aleph hooks revoke`: the entry turns pending, the next fire records
    /// what it runs now ([`Self::record_pending`]), and `aleph hooks test`
    /// reviews and approves that.
    ///
    /// An entry with no recorded root predates root recording; like one with
    /// no script fingerprint it keeps the older, looser meaning until it is
    /// revoked and approved again.
    pub fn is_approved(
        &self,
        plugin_name: &str,
        scope: &ScopeKey,
        plugin_root: &Path,
        command: &str,
    ) -> bool {
        self.reload_if_stale();
        let fp = Self::fingerprint(plugin_name, bound_project(scope), command);
        let (approved_root, recorded) = {
            let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
            match cache.entries.get(&fp) {
                Some(e) if e.status == ConsentStatus::Approved => {
                    (e.plugin_root.clone(), e.script_fingerprint.clone())
                }
                // Missing or still pending — not approved either way.
                _ => return false,
            }
        };

        if let Some(approved_root) = approved_root {
            if !same_dir(&approved_root, plugin_root) {
                warn!(
                    plugin = plugin_name,
                    command,
                    approved_root = %approved_root.display(),
                    root = %plugin_root.display(),
                    "Hook was approved for another directory — refusing to run. \
                     `aleph hooks revoke <fingerprint>`, then review it again with \
                     `aleph hooks test <fingerprint>` after it next fires."
                );
                return false;
            }
        }

        // Entry predates content binding, or its command has no resolvable
        // script: keep the historical command-string-only semantics.
        let Some(approved_hash) = recorded else {
            return true;
        };

        match script_fingerprint(command) {
            Some(current) if current == approved_hash => true,
            Some(_) => {
                warn!(
                    plugin = plugin_name,
                    command,
                    "Hook script changed since it was approved — refusing to run. \
                     `aleph hooks revoke <fingerprint>`, then review it again with \
                     `aleph hooks test <fingerprint>`."
                );
                false
            }
            None => {
                // The script hashed cleanly at approval time and cannot be
                // read now (deleted, renamed, permissions). Something moved
                // under us; fail safe rather than assume it's benign.
                warn!(
                    plugin = plugin_name,
                    command, "Approved hook script is no longer readable — refusing to run."
                );
                false
            }
        }
    }

    /// Record an un-approved shell hook as `pending` so the operator can
    /// review it. Best-effort: a write failure is logged, not propagated.
    ///
    /// `scope` is the hook's `scope_key`, keyed exactly as in
    /// [`Self::is_approved`]. `event` is the name the hook is dispatched with
    /// (`HookConfig::event_name`) and `plugin_root` the root its path
    /// variables resolve to — what `aleph hooks test` rebuilds the run from.
    ///
    /// A key already on file is refreshed while it is still `pending`: its
    /// `event` and `plugin_root` become this fire's, so a review runs what
    /// production just ran — not an entry's first-seen root that has since
    /// been deleted or moved, nor a spelling recorded before the spelling
    /// was kept. An `Approved` entry is never rewritten: its root and script
    /// are what the approval attests to ([`Self::is_approved`]).
    pub fn record_pending(
        &self,
        plugin_name: &str,
        scope: &ScopeKey,
        command: &str,
        event: &str,
        plugin_root: &Path,
    ) {
        let project_root = bound_project(scope);
        let fp = Self::fingerprint(plugin_name, project_root, command);
        // Whether `entry` should be refreshed to this fire; `false` for an
        // approved entry and for one that already says what this fire says.
        let stale = |entry: &ConsentEntry| {
            entry.status == ConsentStatus::Pending
                && (entry.event != event || entry.plugin_root.as_deref() != Some(plugin_root))
        };
        {
            let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
            if cache.entries.get(&fp).is_some_and(|e| !stale(e)) {
                return;
            }
        }
        let entry = ConsentEntry {
            fingerprint: fp.clone(),
            plugin_name: plugin_name.to_string(),
            project_root: project_root.map(Path::to_path_buf),
            command: command.to_string(),
            event: event.to_string(),
            plugin_root: Some(plugin_root.to_path_buf()),
            status: ConsentStatus::Pending,
            first_seen: now_secs(),
            approved_at: None,
            script_fingerprint: script_fingerprint(command),
        };
        if let Err(e) = self.mutate(|entries| match entries.get_mut(&fp) {
            None => {
                entries.insert(fp.clone(), entry);
                true
            }
            Some(known) if stale(known) => {
                known.event = event.to_string();
                known.plugin_root = Some(plugin_root.to_path_buf());
                true
            }
            Some(_) => false,
        }) {
            warn!(error = %e, "Failed to record pending shell-hook consent");
        }
    }

    /// Approve a hook by fingerprint (or unique prefix). Returns the approved
    /// entry, or `None` when no entry matches the prefix — or when the
    /// entry's root is no longer `reviewed_root`.
    ///
    /// `reviewed_root` is the [`plugin_root`](ConsentEntry::plugin_root) the
    /// operator's review ran against (`aleph hooks test` reads it from the
    /// entry). A pending entry follows the hook's latest fire
    /// ([`Self::record_pending`]), so it can move to another root between
    /// that review and this call; approving it anyway would bind the
    /// approval to a directory nobody reviewed. Checked under the registry
    /// lock, with the write.
    pub fn approve(
        &self,
        fingerprint_prefix: &str,
        reviewed_root: Option<&Path>,
    ) -> io::Result<Option<ConsentEntry>> {
        let mut approved: Option<ConsentEntry> = None;
        self.mutate(|entries| {
            let Some(key) = match_prefix(entries, fingerprint_prefix) else {
                return false;
            };
            match entries.get_mut(&key) {
                Some(entry) if entry.plugin_root.as_deref() == reviewed_root => {
                    entry.status = ConsentStatus::Approved;
                    entry.approved_at = Some(now_secs());
                    // Re-hash at approval time, not record time: the operator
                    // just reviewed (and `aleph hooks test` just RAN) the
                    // script as it exists NOW, so that content is what the
                    // approval attests to. A stale record-time hash would
                    // refuse the very version the operator green-lit.
                    entry.script_fingerprint = script_fingerprint(&entry.command);
                    approved = Some(entry.clone());
                    true
                }
                // No such entry, or it moved to a root nobody reviewed.
                _ => false,
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
    // review(extension): require at least 4 hex chars — the full fingerprint
    // is 16 hex chars (see `fingerprint`), so 4 hex chars still has ~65k
    // possible prefixes per entry and stops a one-character prefix from
    // resolving to whichever entry happens to sort first alphabetically.
    // Without this, `aleph hooks approve a` would approve the first entry
    // starting with `a` and silently skip the matching-by-prefix step.
    if prefix.len() < 4 {
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

/// Largest script we will hash. A hook driven by a multi-megabyte file is not
/// a thing; the cap just stops a pathological path from being read into memory
/// on every `is_approved` call.
const MAX_SCRIPT_HASH_BYTES: u64 = 1024 * 1024;

/// File extensions that positively identify the script token in a command.
const SCRIPT_EXTENSIONS: [&str; 7] = [".sh", ".bash", ".zsh", ".py", ".js", ".ts", ".rb"];

/// Extract the script file a shell command invokes, if any.
///
/// Handles the three shapes that cover essentially every hook in practice:
/// `./hook.sh`, `python3 /path/hook.py`, `/usr/bin/env bash hook.sh`.
///
/// **Two passes, and the order matters.** A known script extension wins over a
/// merely path-shaped token, because a single pass binds to whichever comes
/// first — and in `sh --rcfile /etc/bashrc run.sh` that is the *config file*.
/// Binding to the wrong file inverts the guard: editing the config would
/// revoke consent (harmless but confusing) while editing `run.sh` — the thing
/// that actually executes — would NOT. Extension-first makes the common case
/// bind to the code.
///
/// Conservative throughout: a candidate must also resolve to an existing
/// regular file. Anything else yields `None`, which keeps the entry on
/// command-string-only semantics rather than binding to something wrong.
/// Only the first match is used; a command chaining two scripts binds to the
/// first, and hashing every token would make the common case pay for a shape
/// nobody writes.
fn script_path_from_command(command: &str) -> Option<PathBuf> {
    let tokens: Vec<&str> = command
        .split_whitespace()
        // Strip the quoting a shell would remove; anything with a glob or a
        // variable is not a stable path, so drop it entirely.
        .map(|raw| raw.trim_matches(|c| c == '"' || c == '\''))
        .filter(|t| !t.is_empty() && !t.contains('$') && !t.contains('*'))
        .collect();

    // Pass 1: a token that names itself a script.
    let by_extension = tokens.iter().find(|t| {
        let lower = t.to_ascii_lowercase();
        SCRIPT_EXTENSIONS.iter().any(|e| lower.ends_with(e))
    });
    // Pass 2: fall back to anything path-shaped (a bare `./hook`, no extension).
    let candidates = by_extension.into_iter().chain(
        tokens
            .iter()
            .filter(|t| t.contains('/') || t.contains('\\')),
    );

    for token in candidates {
        let expanded = match token.strip_prefix("~/") {
            Some(rest) => match dirs::home_dir() {
                Some(h) => h.join(rest),
                None => continue,
            },
            None => PathBuf::from(*token),
        };
        if expanded.is_file() {
            return Some(expanded);
        }
    }
    None
}

/// `sha256` (16 hex chars) of the script `command` invokes, or `None` when
/// there is no resolvable/readable script. Never propagates an I/O error: an
/// unreadable file simply has no fingerprint, and the caller decides what that
/// means (record time: no binding; check time: fail safe).
fn script_fingerprint(command: &str) -> Option<String> {
    let path = script_path_from_command(command)?;
    let meta = fs::metadata(&path).ok()?;
    if meta.len() > MAX_SCRIPT_HASH_BYTES {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(hex16(&hasher.finalize()))
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
        let a = ShellHookConsent::fingerprint("plug", None, "echo hi");
        let b = ShellHookConsent::fingerprint("plug", None, "echo hi");
        let c = ShellHookConsent::fingerprint("plug", None, "echo HI");
        let d = ShellHookConsent::fingerprint("other", None, "echo hi");
        assert_eq!(a, b);
        assert_ne!(a, c, "command change must change the fingerprint");
        assert_ne!(a, d, "plugin change must change the fingerprint");
        assert_eq!(a.len(), 16);
    }

    /// A global key is the pre-binding key, byte for byte, so every approval
    /// of a hook that fires everywhere survives the change. Pinned value:
    /// `sha256("plug\0echo hi")`, what `286ccf533` computed.
    #[test]
    fn a_global_key_is_the_key_it_always_was() {
        assert_eq!(
            ShellHookConsent::fingerprint("plug", None, "echo hi"),
            "c64b0e5b341bfa92"
        );
    }

    #[test]
    fn a_project_key_names_its_project() {
        let key = |root: Option<&str>| {
            ShellHookConsent::fingerprint("user:project", root.map(Path::new), "lint")
        };
        assert_ne!(
            key(Some("/repo/a")),
            key(Some("/repo/b")),
            "one template in two projects is two keys"
        );
        assert_ne!(
            key(Some("/repo/a")),
            key(None),
            "a project key is never the shared-label key"
        );
    }

    /// The doctor's drift check recomputes each entry's key from its own
    /// fields; a project entry must round-trip through the file, or every
    /// one of them reads as hand-edited.
    #[test]
    fn a_recorded_project_entry_derives_its_own_fingerprint() {
        let (_d, consent) = tmp_consent();
        let root = tempfile::tempdir().expect("tempdir");
        let scope = ScopeKey::project(root.path());
        consent.record_pending("user:project", &scope, "lint", "PreToolUse", root.path());

        let entry = consent.entries().remove(0);
        assert_eq!(
            entry.project_root,
            Some(crate::extension::visibility::canonical_root(root.path()))
        );
        assert_eq!(entry.expected_fingerprint(), entry.fingerprint);
        assert!(!entry.predates_project_binding());
        let reopened = ShellHookConsent::with_path(consent.path());
        assert_eq!(
            reopened.entries()[0].expected_fingerprint(),
            entry.fingerprint
        );
    }

    /// Recording under `Global` is exactly how every entry looked before the
    /// binding. Only the project-settings labels can be told apart.
    #[test]
    fn only_an_unbound_project_label_entry_predates_the_binding() {
        let (_d, consent) = tmp_consent();
        for (owner, command) in [
            ("user:project", "a"),
            ("user:project-local", "b"),
            ("user:global", "c"),
            ("some-plugin", "d"),
        ] {
            consent.record_pending(owner, &ScopeKey::Global, command, "e", Path::new("/p"));
        }
        let mut marked: Vec<String> = consent
            .entries()
            .into_iter()
            .filter(ConsentEntry::predates_project_binding)
            .map(|e| e.command)
            .collect();
        marked.sort();
        assert_eq!(marked, ["a", "b"]);
    }

    // -- the root an approval attests to ------------------------------------

    fn key_of(consent: &ShellHookConsent) -> String {
        consent.entries()[0].fingerprint.clone()
    }

    #[test]
    fn an_approval_does_not_follow_its_key_to_another_root() {
        let (_d, consent) = tmp_consent();
        let global = ScopeKey::Global;
        consent.record_pending("fmt", &global, "lint", "PreToolUse", Path::new("/a"));
        consent
            .approve(&key_of(&consent), Some(Path::new("/a")))
            .unwrap();

        assert!(consent.is_approved("fmt", &global, Path::new("/a"), "lint"));
        assert!(
            !consent.is_approved("fmt", &global, Path::new("/b"), "lint"),
            "approved for /a, run from /b"
        );
    }

    /// While pending, an entry says what the latest fire ran; once
    /// approved, it keeps what was approved.
    #[test]
    fn a_pending_entry_follows_the_latest_fire_and_an_approved_one_does_not() {
        let (_d, consent) = tmp_consent();
        let global = ScopeKey::Global;
        consent.record_pending("fmt", &global, "lint", "PreToolUse", Path::new("/a"));
        consent.record_pending("fmt", &global, "lint", "before_tool_call", Path::new("/b"));
        let entry = consent.entries().remove(0);
        assert_eq!(
            (entry.event.as_str(), entry.plugin_root.as_deref()),
            ("before_tool_call", Some(Path::new("/b")))
        );

        consent
            .approve(&entry.fingerprint, Some(Path::new("/b")))
            .unwrap();
        consent.record_pending("fmt", &global, "lint", "PreToolUse", Path::new("/c"));
        let entry = consent.entries().remove(0);
        assert_eq!(entry.status, ConsentStatus::Approved);
        assert_eq!(entry.plugin_root.as_deref(), Some(Path::new("/b")));
    }

    /// An entry recorded before the spelling and the root were kept — the
    /// shape every entry on an upgraded install has — is repaired by its
    /// next fire while pending, so it can be reviewed as production runs it.
    #[test]
    fn a_pending_entry_from_before_roots_were_kept_is_filled_in_by_its_next_fire() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shell-hooks-allowlist.json");
        let fp = ShellHookConsent::fingerprint("fmt", None, "lint");
        let legacy = serde_json::json!({ "version": 1, "entries": [{
            "fingerprint": fp, "plugin_name": "fmt", "command": "lint",
            "event": "BeforeToolCall", "status": "pending", "first_seen": 1
        }] });
        std::fs::write(&path, legacy.to_string()).expect("seed legacy registry");
        let consent = ShellHookConsent::with_path(&path);

        consent.record_pending(
            "fmt",
            &ScopeKey::Global,
            "lint",
            "PreToolUse",
            Path::new("/a"),
        );

        let entry = consent.entries().remove(0);
        assert_eq!(entry.event, "PreToolUse");
        assert_eq!(entry.plugin_root.as_deref(), Some(Path::new("/a")));
        assert_eq!(entry.status, ConsentStatus::Pending);
    }

    /// The review ran against `/a`; a fire moved the pending entry to `/b`
    /// before the operator said yes. Approving now would bind `/b`, which
    /// nobody reviewed.
    #[test]
    fn approval_is_refused_when_the_entry_moved_since_it_was_reviewed() {
        let (_d, consent) = tmp_consent();
        let global = ScopeKey::Global;
        consent.record_pending("fmt", &global, "lint", "PreToolUse", Path::new("/a"));
        let reviewed = consent.entries().remove(0);
        consent.record_pending("fmt", &global, "lint", "PreToolUse", Path::new("/b"));

        let outcome = consent
            .approve(&reviewed.fingerprint, reviewed.plugin_root.as_deref())
            .unwrap();
        assert!(outcome.is_none(), "approved a root nobody reviewed");
        assert_eq!(consent.entries()[0].status, ConsentStatus::Pending);
    }

    /// An approval recorded before roots were kept attests to no root; like
    /// one with no script fingerprint it keeps its older meaning.
    #[test]
    fn an_approval_from_before_roots_were_kept_is_not_bound_to_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shell-hooks-allowlist.json");
        let fp = ShellHookConsent::fingerprint("fmt", None, "lint");
        let legacy = serde_json::json!({ "version": 1, "entries": [{
            "fingerprint": fp, "plugin_name": "fmt", "command": "lint",
            "status": "approved", "first_seen": 1, "approved_at": 2
        }] });
        std::fs::write(&path, legacy.to_string()).expect("seed legacy registry");
        let consent = ShellHookConsent::with_path(&path);
        assert!(consent.is_approved("fmt", &ScopeKey::Global, Path::new("/any"), "lint"));
    }

    #[test]
    fn unknown_hook_is_not_approved() {
        let (_d, consent) = tmp_consent();
        assert!(!consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "rm -rf /"));
    }

    // -- script-content binding (TOCTOU guard) -----------------------------

    /// Write an executable-ish script and return `(dir, command)`.
    fn script_hook(body: &str) -> (tempfile::TempDir, PathBuf, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("hook.sh");
        std::fs::write(&script, body).expect("write script");
        let command = format!("sh {}", script.display());
        (dir, script, command)
    }

    #[test]
    fn approval_binds_to_script_content_and_drift_revokes_it() {
        // The whole point: approving `sh …/hook.sh` must NOT keep approving it
        // after the file is rewritten. Command string is byte-identical
        // throughout, so only content binding can catch this.
        let (_d, consent) = tmp_consent();
        let (_sd, script, command) = script_hook("echo safe\n");

        consent.record_pending(
            "p",
            &ScopeKey::Global,
            &command,
            "before_tool_call",
            Path::new("/p"),
        );
        let fp = consent.entries()[0].fingerprint.clone();
        consent
            .approve(&fp, Some(Path::new("/p")))
            .expect("approve");
        assert!(
            consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), &command),
            "freshly approved"
        );

        std::fs::write(&script, "rm -rf /\n").expect("rewrite script");
        assert!(
            !consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), &command),
            "edited script must lose its approval"
        );

        // Restoring the exact approved bytes restores the approval — the
        // guard keys on content, not on an mtime that any touch would bump.
        std::fs::write(&script, "echo safe\n").expect("restore script");
        assert!(consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), &command));
    }

    #[test]
    fn approving_records_the_content_reviewed_at_approval_time() {
        // `aleph hooks test` runs the script, THEN asks to approve. If the
        // fingerprint were frozen at record time, editing between the two
        // steps would make the just-approved version fail on first fire.
        let (_d, consent) = tmp_consent();
        let (_sd, script, command) = script_hook("echo v1\n");
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            &command,
            "before_tool_call",
            Path::new("/p"),
        );

        std::fs::write(&script, "echo v2\n").expect("edit before approving");
        let fp = consent.entries()[0].fingerprint.clone();
        consent
            .approve(&fp, Some(Path::new("/p")))
            .expect("approve");

        assert!(
            consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), &command),
            "approval must attest to the content present when approving"
        );
    }

    #[test]
    fn deleting_an_approved_script_fails_safe() {
        let (_d, consent) = tmp_consent();
        let (_sd, script, command) = script_hook("echo hi\n");
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            &command,
            "before_tool_call",
            Path::new("/p"),
        );
        let fp = consent.entries()[0].fingerprint.clone();
        consent
            .approve(&fp, Some(Path::new("/p")))
            .expect("approve");

        std::fs::remove_file(&script).expect("delete script");
        assert!(!consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), &command));
    }

    #[test]
    fn commands_without_a_script_keep_string_only_semantics() {
        // `echo hi` has nothing to bind to; it must still approve normally
        // rather than being permanently refused for lack of a fingerprint.
        let (_d, consent) = tmp_consent();
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            "echo hi",
            "before_tool_call",
            Path::new("/p"),
        );
        assert!(consent.entries()[0].script_fingerprint.is_none());
        let fp = consent.entries()[0].fingerprint.clone();
        consent
            .approve(&fp, Some(Path::new("/p")))
            .expect("approve");
        assert!(consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"));
    }

    #[test]
    fn legacy_entries_without_a_script_fingerprint_still_work() {
        // Back-compat: a registry written before this field existed
        // deserializes with `script_fingerprint: None` and must keep running.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shell-hooks-allowlist.json");
        std::fs::write(
            &path,
            r#"{"version":1,"entries":[{
                "fingerprint":"deadbeefdeadbeef","plugin_name":"p",
                "command":"sh /nonexistent/legacy.sh","event":"BeforeToolCall",
                "status":"approved","first_seen":1,"approved_at":2
            }]}"#,
        )
        .expect("seed legacy registry");
        let consent = ShellHookConsent::with_path(&path);

        let entry = &consent.entries()[0];
        assert!(entry.script_fingerprint.is_none());
        // The seeded fingerprint is synthetic, so look it up the way the
        // executor does: by (plugin, command).
        let real_fp = ShellHookConsent::fingerprint("p", None, "sh /nonexistent/legacy.sh");
        assert_ne!(real_fp, entry.fingerprint, "seeded id is deliberately fake");
        // The stored entry is keyed by its recorded fingerprint and stays
        // approved — no forced re-consent for pre-existing registries.
        assert_eq!(entry.status, ConsentStatus::Approved);
    }

    #[test]
    fn script_path_resolution_covers_the_common_command_shapes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("hook.py");
        std::fs::write(&script, "print(1)").expect("write");
        let p = script.display().to_string();

        assert_eq!(
            script_path_from_command(&format!("python3 {p}")).as_ref(),
            Some(&script),
            "interpreter-prefixed"
        );
        assert_eq!(
            script_path_from_command(&format!("/usr/bin/env python3 \"{p}\"")).as_ref(),
            Some(&script),
            "env-prefixed and quoted"
        );
        assert_eq!(
            script_path_from_command(&p).as_ref(),
            Some(&script),
            "bare path"
        );
        // Nothing resolvable → no binding (not a wrong binding).
        assert!(script_path_from_command("echo hi").is_none());
        assert!(script_path_from_command("sh /does/not/exist.sh").is_none());
        // Variables and globs are not stable paths.
        assert!(script_path_from_command("sh $HOME/hook.sh").is_none());
        assert!(script_path_from_command("sh ./hooks/*.sh").is_none());
    }

    #[test]
    fn a_script_extension_wins_over_an_earlier_path_argument() {
        // Single-pass resolution would bind to `--rcfile`'s target, which
        // INVERTS the guard: editing the config would revoke consent while
        // editing the script that actually runs would not.
        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("bashrc");
        let script = dir.path().join("run.sh");
        std::fs::write(&config, "# config").expect("write config");
        std::fs::write(&script, "echo hi").expect("write script");

        let command = format!("sh --rcfile {} {}", config.display(), script.display());
        assert_eq!(
            script_path_from_command(&command).as_ref(),
            Some(&script),
            "must bind to the executed script, not an earlier path argument"
        );
    }

    #[test]
    fn extensionless_script_still_resolves_via_the_path_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("hook");
        std::fs::write(&script, "echo hi").expect("write");
        assert_eq!(
            script_path_from_command(&script.display().to_string()).as_ref(),
            Some(&script)
        );
    }

    #[test]
    fn record_pending_then_approve_round_trip() {
        let (_d, consent) = tmp_consent();
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            "echo hi",
            "after_tool_call",
            Path::new("/p"),
        );
        assert!(
            !consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"),
            "pending != approved"
        );

        let entries = consent.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, ConsentStatus::Pending);
        let fp = entries[0].fingerprint.clone();

        let approved = consent
            .approve(&fp, Some(Path::new("/p")))
            .expect("approve")
            .expect("entry");
        assert_eq!(approved.status, ConsentStatus::Approved);
        assert!(
            consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"),
            "approved hook runs"
        );
    }

    #[test]
    fn record_pending_is_idempotent() {
        let (_d, consent) = tmp_consent();
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            "echo hi",
            "after_tool_call",
            Path::new("/p"),
        );
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            "echo hi",
            "after_tool_call",
            Path::new("/p"),
        );
        assert_eq!(consent.entries().len(), 1);
    }

    #[test]
    fn revoke_sends_approved_hook_back_to_pending() {
        let (_d, consent) = tmp_consent();
        consent.record_pending(
            "p",
            &ScopeKey::Global,
            "echo hi",
            "before_tool_call",
            Path::new("/p"),
        );
        let fp = consent.entries()[0].fingerprint.clone();
        consent.approve(&fp, Some(Path::new("/p"))).unwrap();
        assert!(consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"));

        let revoked = consent.revoke(&fp).expect("revoke").expect("entry");
        assert_eq!(revoked.status, ConsentStatus::Pending);
        assert!(!consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"));
    }

    #[test]
    fn approve_by_prefix_resolves_unique_match() {
        let (_d, consent) = tmp_consent();
        consent.record_pending("p", &ScopeKey::Global, "echo hi", "e", Path::new("/p"));
        let fp = consent.entries()[0].fingerprint.clone();
        let approved = consent
            .approve(fp.get(..6).unwrap_or(&fp), Some(Path::new("/p")))
            .expect("approve")
            .expect("entry");
        assert_eq!(approved.fingerprint, fp);
    }

    #[test]
    fn approve_unknown_prefix_returns_none() {
        let (_d, consent) = tmp_consent();
        assert!(consent
            .approve("ffffffff", None)
            .expect("approve")
            .is_none());
    }

    #[test]
    fn changes_persist_across_reload() {
        let (_d, consent) = tmp_consent();
        let path = consent.path().to_path_buf();
        consent.record_pending("p", &ScopeKey::Global, "echo hi", "e", Path::new("/p"));
        let fp = consent.entries()[0].fingerprint.clone();
        consent.approve(&fp, Some(Path::new("/p"))).unwrap();

        // A fresh instance reads the same file from disk.
        let reopened = ShellHookConsent::with_path(path);
        assert!(reopened.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"));
    }

    #[test]
    fn external_file_change_is_picked_up_via_stamp() {
        let (_d, consent) = tmp_consent();
        let path = consent.path().to_path_buf();
        consent.record_pending("p", &ScopeKey::Global, "echo hi", "e", Path::new("/p"));
        assert!(!consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"));

        // A second process approves the hook by writing the file directly;
        // the approval grows the file, so the `(mtime, len)` stamp differs.
        let writer = ShellHookConsent::with_path(&path);
        let fp = writer.entries()[0].fingerprint.clone();
        writer.approve(&fp, Some(Path::new("/p"))).unwrap();

        assert!(
            consent.is_approved("p", &ScopeKey::Global, Path::new("/p"), "echo hi"),
            "is_approved must reload after the registry file changes"
        );
    }
}
