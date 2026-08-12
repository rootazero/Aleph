//! Approval **grants** — what a human said *yes* to, and for how long.
//!
//! The positive twin of [`denial_ledger`](super::denial_ledger), and the store
//! the former `session_memory` grew into (removed 2026-08-11 — its name still
//! appeared in five doc links here and in the ledger for a day after). Two
//! scopes, one store:
//!
//! * [`GrantScope::Session`] — in-memory, keyed by the conversation's
//!   `SessionKey`, gone when the process is. The historical "approve for the
//!   rest of this session" tier.
//! * [`GrantScope::Always`] — persisted to `~/.aleph/approval-grants.json`, so
//!   it survives a restart. Created **only** by an operator-tier turn answering
//!   a card that offered the tier (`exec::allowed_decisions::for_confirm_gate`),
//!   never by a tool, never by config.
//!
//! # Why one store and not two
//!
//! Because the question a user asks is "what have I allowed?", not "what have I
//! allowed *at each of two tiers*". A listing surface that covers one tier is
//! the same defect as no listing at all, and two stores behind one facade is
//! two places to forget. The scope is a field, so [`GrantStore::list`] cannot
//! structurally miss a tier.
//!
//! # Every grant carries what the human read
//!
//! The key is an opaque action fingerprint
//! ([`grant_fingerprint`](super::action::grant_fingerprint)) — `(tool, canonical
//! arguments)`, at codex's grain, so a grant covers the call that was shown and
//! nothing else. A tool-name key would make one "allow session" on
//! `file_ops list` authorize `file_ops delete`.
//!
//! But a fingerprint is a SHA-256 prefix, and **a revocation list of hashes is
//! not a revocation list**: the person reading it cannot tell which entry is the
//! one they want gone. So every grant stores the same redacted, capped
//! `summary` the card put in front of the human ([`ApprovalAction::summary`]),
//! plus who granted it and when. That is the entire reason this type exists
//! rather than a `HashSet<String>`.
//!
//! # Persistence shape
//!
//! Mirrors [`ShellHookConsent`] — the repo's other human-consent registry —
//! down to the file lock, the atomic tmp+rename write, and the `(mtime, len)`
//! cache stamp: two stores answering "did a human approve this, durably?"
//! should not answer it in two different shapes. A corrupt file reads as empty
//! (fail **closed** — every gated call asks again), never as "allow".
//!
//! [`ApprovalAction::summary`]: super::action::ApprovalAction::summary
//! [`ShellHookConsent`]: crate::extension::hooks::consent::ShellHookConsent

use crate::sync_primitives::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use tracing::warn;

/// Max distinct sessions retained before FIFO eviction kicks in. Matches
/// [`denial_ledger`](super::denial_ledger)'s bound so the positive and negative
/// stores have an identical memory ceiling.
const MAX_SESSIONS: usize = 1024;

/// Max persisted "always" grants. Eviction drops the OLDEST grant, which costs
/// at most one re-prompt — the safe direction. A user with 512 standing
/// exceptions has a policy question, not a storage question.
const MAX_PERSISTENT: usize = 512;

/// On-disk document version. Bumped only on an incompatible layout change; a
/// document from a newer version reads as empty (fail closed) rather than being
/// half-interpreted.
const DOC_VERSION: u32 = 1;

/// How long a grant lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantScope {
    /// The remainder of one conversation, in this process.
    Session,
    /// Until revoked, across restarts.
    Always,
}

impl GrantScope {
    /// Stable token for wire payloads, RPC filters and tests.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Always => "always",
        }
    }

    /// Parse a wire token. `None` for anything else — an unrecognized scope is
    /// never treated as the wider one.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "session" => Some(Self::Session),
            "always" => Some(Self::Always),
            _ => None,
        }
    }
}

/// One standing authorization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    /// `(tool, canonical arguments)` fingerprint — the match key.
    pub fingerprint: String,
    /// Registered tool name, for grouping and for the "revoke everything for
    /// this tool" affordance.
    pub tool: String,
    /// The redacted, capped one-liner the human actually read on the card.
    /// Without it a revocation list is a list of hashes.
    pub summary: String,
    pub scope: GrantScope,
    /// Unix ms. Also the eviction order for the persistent tier.
    pub granted_at_ms: u64,
    /// The *person* who granted it (`visibility::ambient_actor`), when known.
    /// Audit only — a persistent grant is install-wide, exactly like the
    /// `[policies.tool_permissions]` `allow` entry it is the per-call sibling
    /// of. Never used as a match key: an actor that resolves to `None` on a
    /// CLI/cron turn would silently stop matching, which reads as "my always
    /// grant stopped working" and is indistinguishable from a bug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_by: Option<String>,
    /// The conversation it was granted in. `Some` for [`GrantScope::Session`]
    /// (it *is* the bucket key); on a persistent grant it records where the
    /// decision was taken, for the listing surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
}

impl Grant {
    /// A grant of `action` in `scope`, stamped now.
    #[must_use]
    pub fn new(
        fingerprint: impl Into<String>,
        tool: impl Into<String>,
        summary: impl Into<String>,
        scope: GrantScope,
    ) -> Self {
        Self {
            fingerprint: fingerprint.into(),
            tool: tool.into(),
            summary: summary.into(),
            scope,
            granted_at_ms: now_ms(),
            granted_by: None,
            session_key: None,
        }
    }

    #[must_use]
    pub fn by(mut self, principal: Option<String>) -> Self {
        self.granted_by = principal;
        self
    }

    #[must_use]
    pub fn in_session(mut self, session: Option<String>) -> Self {
        self.session_key = session;
        self
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Default)]
struct SessionInner {
    /// session key → fingerprint → grant.
    by_session: HashMap<String, HashMap<String, Grant>>,
    /// FIFO of session keys, for bounded eviction of the oldest session.
    order: VecDeque<String>,
}

#[derive(Serialize, Deserialize)]
struct GrantsDoc {
    version: u32,
    #[serde(default)]
    grants: Vec<Grant>,
}

#[derive(Default, PartialEq, Eq, Clone, Copy)]
struct FileStamp {
    mtime_ms: u64,
    len: u64,
}

fn file_stamp(path: &Path) -> FileStamp {
    let Ok(meta) = std::fs::metadata(path) else {
        return FileStamp::default();
    };
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64);
    FileStamp {
        mtime_ms,
        len: meta.len(),
    }
}

#[derive(Default)]
struct PersistentCache {
    entries: BTreeMap<String, Grant>,
    stamp: FileStamp,
}

/// Session grants (memory) + always grants (disk), behind one API.
pub struct GrantStore {
    session: Mutex<SessionInner>,
    persistent: RwLock<PersistentCache>,
    path: PathBuf,
}

impl GrantStore {
    /// The persisted registry's default location. `get_config_dir` is the pure
    /// lookup (it does not create), so a read-only listing never creates the
    /// directory it is measuring — see 判据 §5.9.
    #[must_use]
    pub fn default_path() -> PathBuf {
        crate::utils::paths::get_config_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("approval-grants.json")
    }

    /// A store backed by an explicit path. The only constructor tests use — a
    /// test that reached [`global`] would write the developer's real registry.
    #[must_use]
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (entries, stamp) = Self::read_file(&path);
        Self {
            session: Mutex::new(SessionInner::default()),
            persistent: RwLock::new(PersistentCache { entries, stamp }),
            path,
        }
    }

    /// Registry file path (diagnostics, tests).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The scope under which `fingerprint` is already authorized, if any — with
    /// the persistent tier consulted only when the asking gate may honour it.
    ///
    /// `session` is `None` when the caller could not derive a session identity:
    /// the session tier is then simply not consulted (a grant must never be
    /// shared across an unknown session key), while the install-wide persistent
    /// tier still answers.
    ///
    /// [`GrantScope::Always`] is reported in preference to `Session` when both
    /// hold: the wider scope is the one the user would look for when asking
    /// "why does this not ask me any more".
    ///
    /// There is deliberately **no** `granted(session, fingerprint)` convenience
    /// that always honours everything. A shorter name meaning "the unrestricted
    /// read" is precisely the trap this split exists to prevent: the next gate
    /// added here would reach for it, get the wide answer, and nothing would
    /// say so. Every caller states its posture.
    ///
    /// # Why a card that cannot CREATE a persistent grant must not be
    /// SATISFIED by one
    ///
    /// The two questions have one answer
    /// (`exec::allowed_decisions::for_confirm_gate`), and letting them drift
    /// apart re-opens the gap the derivation closes. An operator answers
    /// "always" to some call; a MEMBER later issues the byte-identical call and
    /// trips the operator-escalation gate — a card raised *because* the
    /// requester may not decide this. Honouring the operator's grant there
    /// would mean the escalation silently stops happening for everyone, which
    /// is not what the person clicking "always" on their own card was asked
    /// about.
    ///
    /// The same clause covers the tool's declared confirmation floor: no card
    /// on such a tool offers the tier, so none may be waved through by a grant
    /// that predates the tool declaring it (an MCP server adding
    /// `destructiveHint`, a builtin joining `CONFIRMATION_REQUIRED_TOOLS`).
    ///
    /// The session tier is unconditional: it is created and consumed inside one
    /// conversation, which is the only principal it could belong to.
    #[must_use]
    pub fn granted_within(
        &self,
        session: Option<&str>,
        fingerprint: &str,
        honor_persistent: bool,
    ) -> Option<GrantScope> {
        if honor_persistent && self.persistent_contains(fingerprint) {
            return Some(GrantScope::Always);
        }
        let session = session?;
        let guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .by_session
            .get(session)
            .and_then(|m| m.get(fingerprint))
            .map(|_| GrantScope::Session)
    }

    /// Remember a session-scoped grant. `session` is the ledger key
    /// ([`denial_ledger::ledger_key`](super::denial_ledger::ledger_key)), so the
    /// positive and negative stores address one bucket.
    pub fn remember_session(&self, session: &str, grant: Grant) {
        let mut guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.by_session.contains_key(session) {
            // New session: enforce the bound before inserting so the map and
            // the FIFO order stay in lockstep.
            while guard.order.len() >= MAX_SESSIONS {
                match guard.order.pop_front() {
                    Some(evicted) => {
                        guard.by_session.remove(&evicted);
                    }
                    None => break,
                }
            }
            guard.order.push_back(session.to_string());
        }
        let grant = Grant {
            scope: GrantScope::Session,
            session_key: Some(session.to_string()),
            ..grant
        };
        guard
            .by_session
            .entry(session.to_string())
            .or_default()
            .insert(grant.fingerprint.clone(), grant);
    }

    /// Write a persistent grant. Errors are returned, never swallowed: a
    /// persistent grant that silently failed to persist re-prompts forever and
    /// the user has no way to tell why.
    ///
    /// # Errors
    /// Propagates any filesystem error from the locked read-modify-write.
    pub fn remember_always(&self, grant: Grant) -> io::Result<()> {
        let grant = Grant {
            scope: GrantScope::Always,
            ..grant
        };
        self.mutate(|entries| {
            entries.insert(grant.fingerprint.clone(), grant.clone());
            while entries.len() > MAX_PERSISTENT {
                // Oldest first — dropping a grant only ever costs a re-prompt.
                let Some(oldest) = entries
                    .values()
                    .min_by_key(|g| g.granted_at_ms)
                    .map(|g| g.fingerprint.clone())
                else {
                    break;
                };
                warn!(
                    fingerprint = %oldest,
                    "persistent approval grants at capacity — evicted the oldest grant"
                );
                entries.remove(&oldest);
            }
            true
        })
    }

    /// Every grant this store holds, newest first — both tiers.
    ///
    /// Unscoped on purpose: the *visibility* rule belongs to the surface that
    /// has an actor to scope against (`gateway::handlers::exec_grants`), not to
    /// the store. A store that filtered would be a second, weaker copy of that
    /// rule.
    #[must_use]
    pub fn list(&self) -> Vec<Grant> {
        // Same re-read the gate's own lookup does. Without it this surface —
        // the ONE that exists so a person can see and undo what is standing —
        // would serve a cache: a grant another process revoked would still be
        // listed, and one it added would be invisible AND unrevocable, since
        // `revoke` resolves its target through this list.
        self.reload_if_stale();
        let mut out: Vec<Grant> = {
            let guard = self.persistent.read().unwrap_or_else(|e| e.into_inner());
            guard.entries.values().cloned().collect()
        };
        {
            let guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
            for grants in guard.by_session.values() {
                out.extend(grants.values().cloned());
            }
        }
        out.sort_by_key(|g| std::cmp::Reverse(g.granted_at_ms));
        out
    }

    /// Drop one grant. `session` is required for [`GrantScope::Session`] and
    /// ignored for [`GrantScope::Always`]. Returns whether anything was removed.
    ///
    /// # Errors
    /// Propagates filesystem errors when revoking a persistent grant.
    pub fn revoke(
        &self,
        scope: GrantScope,
        session: Option<&str>,
        fingerprint: &str,
    ) -> io::Result<bool> {
        match scope {
            GrantScope::Session => {
                let Some(session) = session else {
                    return Ok(false);
                };
                let mut guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
                Ok(guard
                    .by_session
                    .get_mut(session)
                    .is_some_and(|m| m.remove(fingerprint).is_some()))
            }
            GrantScope::Always => {
                let mut removed = false;
                self.mutate(|entries| {
                    removed = entries.remove(fingerprint).is_some();
                    removed
                })?;
                Ok(removed)
            }
        }
    }

    fn persistent_contains(&self, fingerprint: &str) -> bool {
        self.reload_if_stale();
        let guard = self.persistent.read().unwrap_or_else(|e| e.into_inner());
        guard.entries.contains_key(fingerprint)
    }

    fn read_file(path: &Path) -> (BTreeMap<String, Grant>, FileStamp) {
        let stamp = file_stamp(path);
        let Ok(raw) = std::fs::read(path) else {
            return (BTreeMap::new(), stamp);
        };
        match serde_json::from_slice::<GrantsDoc>(&raw) {
            Ok(doc) if doc.version <= DOC_VERSION => (
                doc.grants
                    .into_iter()
                    .map(|g| (g.fingerprint.clone(), g))
                    .collect(),
                stamp,
            ),
            Ok(doc) => {
                warn!(
                    version = doc.version,
                    path = %path.display(),
                    "approval-grants registry is from a newer version — treating as empty \
                     (every gated call asks again)"
                );
                (BTreeMap::new(), stamp)
            }
            Err(e) => {
                warn!(
                    error = %e,
                    path = %path.display(),
                    "corrupt approval-grants registry — treating as empty"
                );
                (BTreeMap::new(), stamp)
            }
        }
    }

    /// Reload the cache when the file's `(mtime, len)` stamp changed — another
    /// process (a second server instance, a user editing the file) may have
    /// revoked a grant, and a cache that never re-reads would keep honouring it.
    fn reload_if_stale(&self) {
        let disk = file_stamp(&self.path);
        {
            let guard = self.persistent.read().unwrap_or_else(|e| e.into_inner());
            if guard.stamp == disk {
                return;
            }
        }
        let (entries, stamp) = Self::read_file(&self.path);
        let mut guard = self.persistent.write().unwrap_or_else(|e| e.into_inner());
        guard.entries = entries;
        guard.stamp = stamp;
    }

    /// Cross-process-safe read-modify-write, mirroring
    /// [`ShellHookConsent::mutate`]: exclusive file lock, re-read (another
    /// process may have changed it), apply, atomic tmp+rename when `f` reports
    /// a change, refresh the cache.
    ///
    /// [`ShellHookConsent::mutate`]: crate::extension::hooks::consent::ShellHookConsent
    fn mutate<F>(&self, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut BTreeMap<String, Grant>) -> bool,
    {
        use fs2::FileExt;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock_path = self.path.with_extension("lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        lock.lock_exclusive()?;

        let (mut entries, _) = Self::read_file(&self.path);
        let result = (|| {
            if f(&mut entries) {
                let doc = GrantsDoc {
                    version: DOC_VERSION,
                    grants: entries.values().cloned().collect(),
                };
                let bytes = serde_json::to_vec_pretty(&doc)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                let tmp = self.path.with_extension("json.tmp");
                std::fs::write(&tmp, &bytes)?;
                std::fs::rename(&tmp, &self.path)?;
            }
            Ok(())
        })();

        {
            let mut guard = self.persistent.write().unwrap_or_else(|e| e.into_inner());
            guard.entries = entries;
            guard.stamp = file_stamp(&self.path);
        }
        let _ = FileExt::unlock(&lock);
        result
    }
}

/// Where the process-wide store writes.
///
/// In a test build this is a per-process file in the temp dir, NOT the real
/// registry: a test that drives an approval to "always" would otherwise write a
/// permanent grant into the developer's own `~/.aleph`, and a test suite that
/// edits the machine it runs on is a defect regardless of what it was testing.
///
/// Both test builds are covered, and that is the point: `cfg(test)` catches the
/// unit tests, and the `test-helpers` feature catches the integration tests
/// (`tests/*.rs` link a NON-`cfg(test)` alephcore, so the first arm alone would
/// have left exactly the harnesses that drive whole runs writing the real file).
/// Only some integration tests set `ALEPH_HOME`, so that convention could not be
/// leaned on either.
///
/// Tests that need isolation from EACH OTHER use [`GrantStore::with_path`]; this
/// only guarantees isolation from the user.
fn global_path() -> PathBuf {
    #[cfg(any(test, feature = "test-helpers"))]
    {
        std::env::temp_dir().join(format!(
            "aleph-approval-grants-test-{}.json",
            std::process::id()
        ))
    }
    #[cfg(not(any(test, feature = "test-helpers")))]
    {
        GrantStore::default_path()
    }
}

static GLOBAL: LazyLock<Arc<GrantStore>> =
    LazyLock::new(|| Arc::new(GrantStore::with_path(global_path())));

/// The process-wide grant store shared by the confirm gate and the
/// `exec.grants.*` RPCs.
///
/// One instance, by construction: the gate reads it and the revocation RPC
/// writes it, so a second instance would make "revoke" a no-op that reports
/// success — the store equivalent of the two-buckets bug
/// [`ledger_key`](super::denial_ledger::ledger_key) exists to prevent.
#[must_use]
pub fn global() -> Arc<GrantStore> {
    GLOBAL.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, GrantStore) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("approval-grants.json");
        (dir, GrantStore::with_path(path))
    }

    fn grant(fp: &str) -> Grant {
        Grant::new(
            fp,
            "file_ops",
            "file_ops: operation=delete path=/tmp/x",
            GrantScope::Session,
        )
    }

    #[test]
    fn remembers_and_recalls_per_session() {
        let (_d, s) = store();
        assert!(s.granted_within(Some("s1"), "fp-a", true).is_none());
        s.remember_session("s1", grant("fp-a"));
        assert_eq!(
            s.granted_within(Some("s1"), "fp-a", true),
            Some(GrantScope::Session)
        );
        // A different action under the same session is not implicitly granted.
        assert!(s.granted_within(Some("s1"), "fp-b", true).is_none());
    }

    #[test]
    fn grants_do_not_leak_across_sessions() {
        let (_d, s) = store();
        s.remember_session("s1", grant("fp-a"));
        // The core isolation invariant.
        assert!(s.granted_within(Some("s2"), "fp-a", true).is_none());
    }

    #[test]
    fn a_session_grant_is_invisible_without_a_session_key() {
        let (_d, s) = store();
        s.remember_session("s1", grant("fp-a"));
        assert!(
            s.granted_within(None, "fp-a", true).is_none(),
            "an unknown session must never inherit another session's grant"
        );
    }

    #[test]
    fn repeated_remember_is_idempotent() {
        let (_d, s) = store();
        s.remember_session("s1", grant("fp-a"));
        s.remember_session("s1", grant("fp-a"));
        let guard = s.session.lock().unwrap();
        assert_eq!(guard.order.len(), 1, "same session enqueued once");
        assert_eq!(guard.by_session.get("s1").map(HashMap::len), Some(1));
    }

    #[test]
    fn bounded_eviction_drops_oldest_session() {
        let (_d, s) = store();
        for i in 0..MAX_SESSIONS {
            s.remember_session(&format!("s{i}"), grant("fp"));
        }
        assert!(s.granted_within(Some("s0"), "fp", true).is_some());
        s.remember_session("overflow", grant("fp"));
        assert!(
            s.granted_within(Some("s0"), "fp", true).is_none(),
            "oldest evicted"
        );
        assert!(s.granted_within(Some("overflow"), "fp", true).is_some());
        let guard = s.session.lock().unwrap();
        assert_eq!(guard.order.len(), MAX_SESSIONS);
        assert_eq!(guard.by_session.len(), MAX_SESSIONS);
    }

    /// The point of the tier: it outlives the process, so a *second* store over
    /// the same path — which is what a restart is — still honours it.
    #[test]
    fn a_persistent_grant_survives_a_restart() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("approval-grants.json");
        {
            let s = GrantStore::with_path(&path);
            s.remember_always(grant("fp-a")).expect("persist");
            assert_eq!(
                s.granted_within(None, "fp-a", true),
                Some(GrantScope::Always)
            );
        }
        let reopened = GrantStore::with_path(&path);
        assert_eq!(
            reopened.granted_within(None, "fp-a", true),
            Some(GrantScope::Always),
            "a persistent grant must outlive the process that took it"
        );
        // And it is session-independent, which is the whole difference.
        assert_eq!(
            reopened.granted_within(Some("some-other-session"), "fp-a", true),
            Some(GrantScope::Always)
        );
    }

    #[test]
    fn revoking_a_persistent_grant_reaches_the_disk() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("approval-grants.json");
        let s = GrantStore::with_path(&path);
        s.remember_always(grant("fp-a")).expect("persist");
        assert!(s.revoke(GrantScope::Always, None, "fp-a").expect("revoke"));
        assert!(s.granted_within(None, "fp-a", true).is_none());
        // Not merely dropped from the cache — the next process must not see it.
        let reopened = GrantStore::with_path(&path);
        assert!(reopened.granted_within(None, "fp-a", true).is_none());
        // Revoking twice is not an error, it is a `false`.
        assert!(!s.revoke(GrantScope::Always, None, "fp-a").expect("revoke"));
    }

    #[test]
    fn revoking_a_session_grant_needs_its_session() {
        let (_d, s) = store();
        s.remember_session("s1", grant("fp-a"));
        assert!(
            !s.revoke(GrantScope::Session, Some("s2"), "fp-a")
                .expect("revoke"),
            "another session's key must not revoke this grant"
        );
        assert!(s
            .revoke(GrantScope::Session, Some("s1"), "fp-a")
            .expect("revoke"));
        assert!(s.granted_within(Some("s1"), "fp-a", true).is_none());
    }

    /// A listing that covers one tier is the same defect as no listing: this is
    /// the property that makes the single-store design load-bearing.
    #[test]
    fn list_covers_both_tiers_and_carries_what_the_human_read() {
        let (_d, s) = store();
        s.remember_session("s1", grant("fp-a"));
        s.remember_always(grant("fp-b")).expect("persist");
        let all = s.list();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|g| g.scope == GrantScope::Session));
        assert!(all.iter().any(|g| g.scope == GrantScope::Always));
        assert!(
            all.iter()
                .all(|g| g.summary.contains("operation=delete") && !g.tool.is_empty()),
            "a grant list of bare fingerprints is not revocable by a human"
        );
    }

    /// The listing surface re-reads too. It is the one place a person looks to
    /// see what is standing; serving a stale cache there would show a revoked
    /// grant as live, and hide one this process never learned about — which
    /// `revoke` would then be unable to find, because it resolves its target
    /// through this same list.
    #[test]
    fn the_listing_sees_another_processs_writes() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("approval-grants.json");
        let reader = GrantStore::with_path(&path);
        let writer = GrantStore::with_path(&path);
        assert!(reader.list().is_empty());

        writer.remember_always(grant("fp-a")).expect("persist");
        assert_eq!(
            reader.list().len(),
            1,
            "a grant written elsewhere is listed"
        );
        assert!(
            reader
                .revoke(GrantScope::Always, None, "fp-a")
                .expect("revoke"),
            "…and revocable from here"
        );
        assert!(writer.list().is_empty());
    }

    /// A grant file we cannot parse must not authorize anything.
    #[test]
    fn a_corrupt_registry_reads_as_empty_not_as_allow() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("approval-grants.json");
        std::fs::write(&path, b"{not json").expect("write");
        let s = GrantStore::with_path(&path);
        assert!(s.granted_within(None, "fp-a", true).is_none());
        assert!(s.list().is_empty());
    }

    /// A newer on-disk layout is not half-read.
    #[test]
    fn a_future_version_reads_as_empty() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("approval-grants.json");
        let doc = serde_json::json!({
            "version": DOC_VERSION + 1,
            "grants": [{
                "fingerprint": "fp-a", "tool": "bash", "summary": "bash: rm -rf /",
                "scope": "always", "granted_at_ms": 1
            }]
        });
        std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).expect("write");
        let s = GrantStore::with_path(&path);
        assert!(s.granted_within(None, "fp-a", true).is_none());
    }

    /// Another process revoked a grant: the cache stamp must notice.
    #[test]
    fn an_external_revocation_is_picked_up_without_a_restart() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("approval-grants.json");
        let reader = GrantStore::with_path(&path);
        let writer = GrantStore::with_path(&path);
        writer.remember_always(grant("fp-a")).expect("persist");
        assert_eq!(
            reader.granted_within(None, "fp-a", true),
            Some(GrantScope::Always)
        );
        writer
            .revoke(GrantScope::Always, None, "fp-a")
            .expect("revoke");
        assert!(
            reader.granted_within(None, "fp-a", true).is_none(),
            "a cache that never re-reads keeps honouring a revoked grant"
        );
    }

    #[test]
    fn persistent_tier_is_bounded() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("approval-grants.json");
        let s = GrantStore::with_path(&path);
        for i in 0..MAX_PERSISTENT {
            let mut g = grant(&format!("fp-{i}"));
            g.granted_at_ms = 1000 + i as u64;
            s.remember_always(g).expect("persist");
        }
        let mut newest = grant("fp-newest");
        newest.granted_at_ms = 9_999_999;
        s.remember_always(newest).expect("persist");
        assert!(
            s.granted_within(None, "fp-0", true).is_none(),
            "oldest evicted"
        );
        assert!(s.granted_within(None, "fp-newest", true).is_some());
        assert_eq!(s.list().len(), MAX_PERSISTENT);
    }

    /// A gate that may not hand out a persistent grant may not be satisfied by
    /// one either — and its own session grant still works.
    #[test]
    fn a_gate_that_cannot_create_a_persistent_grant_is_not_satisfied_by_one() {
        let (_d, s) = store();
        s.remember_always(grant("fp-a")).expect("persist");
        assert_eq!(
            s.granted_within(Some("s1"), "fp-a", true),
            Some(GrantScope::Always)
        );
        assert!(
            s.granted_within(Some("s1"), "fp-a", false).is_none(),
            "an escalation card must not be waved through by a grant it could not have created"
        );
        // …while a session grant taken at that same gate still holds.
        s.remember_session("s1", grant("fp-a"));
        assert_eq!(
            s.granted_within(Some("s1"), "fp-a", false),
            Some(GrantScope::Session)
        );
    }

    #[test]
    fn scope_tokens_round_trip_and_reject_unknown() {
        assert_eq!(GrantScope::parse("session"), Some(GrantScope::Session));
        assert_eq!(GrantScope::parse("always"), Some(GrantScope::Always));
        assert_eq!(GrantScope::parse("forever"), None);
        assert_eq!(GrantScope::Always.as_str(), "always");
    }

    /// Both tiers hold: the wider one is reported, because that is the one the
    /// user is looking for when they ask why a call stopped asking.
    #[test]
    fn always_outranks_session_in_the_report() {
        let (_d, s) = store();
        s.remember_session("s1", grant("fp-a"));
        s.remember_always(grant("fp-a")).expect("persist");
        assert_eq!(
            s.granted_within(Some("s1"), "fp-a", true),
            Some(GrantScope::Always)
        );
    }
}
