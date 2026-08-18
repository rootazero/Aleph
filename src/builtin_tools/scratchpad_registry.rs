//! Session → active-scratchpad pointer registry.
//!
//! The `scratchpad` tool is keyed by an LLM-chosen `project_id`, *not* by
//! session. To let the goal-loop hook ([`crate::verification::ScratchpadGoalVerifier`])
//! find *which* execution list belongs to the session that is about to
//! stop, the tool records its most-recently-touched `project_id` here,
//! keyed by the live session key. The verifier reads it back at stop time.
//!
//! ## Persistence (cross-restart goal-loop continuation)
//!
//! The scratchpad markdown file on disk remains the single source of truth
//! for the execution-list *contents*. This table is the session→project
//! *binding* — without it, after a daemon restart the goal-loop hook can no
//! longer tell that a resumed session still owns an unfinished plan, so an
//! in-flight multi-step task would silently lose its continuation. To close
//! that gap (mirroring openclaw's `reloadTaskRegistryFromStore()` at boot),
//! the binding table is mirrored write-through to a small JSON store and
//! reloaded on startup via [`init_persistence`].
//!
//! Persistence is *opt-in*: until [`init_persistence`] runs (the daemon
//! wires it at boot), the table behaves exactly as before — a pure in-memory
//! process-global pointer with no disk I/O. `session_key` is deterministic
//! per channel/chat, so a resumed session re-binds to the same plan.
//!
//! R10 note: this is plumbing, not cognition — it answers the purely
//! mechanical question "which project file is this session writing to?".
//! It performs no judgment of its own.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;

use crate::sync_primitives::Mutex;

type Bindings = HashMap<String, String>;

static ACTIVE: Lazy<Mutex<Bindings>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Disk mirror target. `None` keeps the registry in-memory-only (the
/// pre-persistence behavior); `init_persistence` sets it at boot.
static STORE_PATH: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));

fn active_lock() -> crate::sync_primitives::MutexGuard<'static, Bindings> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

fn store_path() -> Option<PathBuf> {
    STORE_PATH.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Read the persisted binding table. Missing file or parse error → empty
/// (fail-open: a corrupt mirror must never block startup).
fn read_store(path: &Path) -> Bindings {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

/// Atomically write the binding table. Best-effort: callers log on error
/// rather than failing the originating tool action.
fn write_store(path: &Path, map: &Bindings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(map).map_err(std::io::Error::other)?;
    crate::utils::atomic_io::write_atomic(path, &json)
}

/// PR-5 / BT-D-R4-13: persist while holding the in-memory lock so the
/// disk mirror and the live map are updated as one linearizable step.
/// The previous shape released the lock before `persist`, so two
/// concurrent writers could both snapshot the live map before either
/// wrote to disk, and the second write would silently overwrite the
/// first — memory and disk were free to disagree for an arbitrary
/// interval. The acquire-then-persist-here path takes the lock once,
/// mutates, mirrors, and drops — no other caller can interleave
/// between the memory and disk sides.
fn persist_locked(map: &Bindings) {
    if let Some(path) = store_path() {
        if let Err(e) = write_store(&path, map) {
            tracing::warn!(error = %e, "scratchpad_registry: failed to persist bindings");
        }
    }
}

/// Legacy alias for callers that already hold the lock and just want
/// the disk-side write to run.
#[allow(dead_code)]
fn persist(map: &Bindings) {
    persist_locked(map);
}

/// Enable persistence and reload any bindings written by a prior process.
///
/// Called once at daemon boot, before sessions resume. Loaded bindings are
/// merged into the (empty-at-boot) in-memory table without clobbering any
/// live entry. Idempotent.
pub fn init_persistence(path: PathBuf) {
    let loaded = read_store(&path);
    {
        let mut store = STORE_PATH.lock().unwrap_or_else(|e| e.into_inner());
        *store = Some(path);
    }
    let mut map = active_lock();
    for (k, v) in loaded {
        map.entry(k).or_insert(v);
    }
}

/// Record `project_id` as the active execution list for `session_key`.
///
/// No-op when either key is empty (an unbound session must not shadow a
/// real one under the empty-string key).
pub fn set_active(session_key: &str, project_id: &str) {
    if session_key.is_empty() || project_id.is_empty() {
        return;
    }
    // PR-5 / BT-D-R4-13: hold the lock across the persist so the
    // memory map and the disk mirror are a single linearizable step.
    let mut map = active_lock();
    map.insert(session_key.to_string(), project_id.to_string());
    persist_locked(&map);
}

/// The active execution-list `project_id` for `session_key`, if any.
#[must_use]
pub fn active(session_key: &str) -> Option<String> {
    if session_key.is_empty() {
        return None;
    }
    active_lock().get(session_key).cloned()
}

/// Drop the pointer for `session_key` (e.g. the scratchpad was cleared).
pub fn clear(session_key: &str) {
    // PR-5 / BT-D-R4-13: hold the lock across the persist so the
    // memory map and the disk mirror are a single linearizable step.
    let mut map = active_lock();
    map.remove(session_key);
    persist_locked(&map);
}

/// Delete the plan file a deleted session owned, and drop its binding.
///
/// The sibling of `purge_session_snapshot` on the session-delete path: the
/// resume snapshot is a summary OF the conversation, and the scratchpad is its
/// working memory. Both are keyed by the *stable* session key, so leaving the
/// plan behind means a session re-created under the same key silently inherits
/// the deleted conversation's execution list — and the goal-loop keeps vetoing
/// stop until the new session finishes someone else's plan.
///
/// A `project_id` can be named explicitly by the model and shared across
/// conversations, so the file is only removed once no other live binding points
/// at it. Best-effort: a failed unlink is logged, never propagated — session
/// deletion must not fail because a markdown file could not be removed.
///
/// `ScratchpadManager` owns the `project_id → directory` mapping; this call
/// site must not re-derive the path.
pub async fn purge_session_scratchpad(session_key: &str) {
    if session_key.is_empty() {
        return;
    }
    // PR-5 / BT-D-R4-13: hold the lock across the in-memory removal AND
    // the disk mirror update so the two are a single linearizable step.
    // The async file purge stays outside the lock (we cannot hold an
    // async mutex across an .await without poisoning the executor), and
    // remains best-effort — a session that re-binds the project between
    // the lock release and the file delete is the documented narrow
    // race window this PR accepts.
    let project_id = {
        let mut map = active_lock();
        let Some(project_id) = map.remove(session_key) else {
            return;
        };
        if map.values().any(|p| *p == project_id) {
            // Still owned by another session; persist the now-shorter map
            // and leave the file alone.
            persist_locked(&map);
            return;
        }
        persist_locked(&map);
        project_id
    };
    if let Err(e) = crate::memory::scratchpad::ScratchpadManager::new(&project_id, session_key)
        .purge()
        .await
    {
        tracing::warn!(error = %e, session_key, project_id, "failed to purge session scratchpad");
    }
}

/// Drop bindings belonging to superseded epochs of `base_key`.
///
/// Session keys carry an epoch suffix (`…:s2`); once a newer epoch exists the
/// older key can never be addressed again, so its binding is dead weight that
/// is nonetheless rewritten to disk on every single scratchpad tool call. The
/// table is process-global and mirrored write-through, so without a pruning
/// point it only ever grows — one entry per conversation the user ever started.
pub fn clear_superseded_epochs(base_key: &str, current_epoch: u32) {
    if base_key.is_empty() || current_epoch == 0 {
        return;
    }
    // PR-5 / BT-D-R4-13: hold the lock across retain + persist so the
    // memory map and the disk mirror are a single linearizable step.
    let mut map = active_lock();
    let before = map.len();
    map.retain(|key, _| !is_superseded_epoch(key, base_key, current_epoch));
    if map.len() == before {
        return;
    }
    persist_locked(&map);
}

/// Does `key` name an epoch of `base_key` older than `current_epoch`?
///
/// Only exact epoch spellings count: a key that merely starts with `base_key`
/// (a different chat whose name is a prefix extension) is left alone.
fn is_superseded_epoch(key: &str, base_key: &str, current_epoch: u32) -> bool {
    let Some(rest) = key.strip_prefix(base_key) else {
        return false;
    };
    let epoch = if rest.is_empty() {
        0
    } else {
        match rest.strip_prefix(":s").and_then(|n| n.parse::<u32>().ok()) {
            Some(n) => n,
            None => return false,
        }
    };
    epoch < current_epoch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_active_then_read_roundtrips() {
        set_active("sess-roundtrip", "proj-a");
        assert_eq!(active("sess-roundtrip"), Some("proj-a".to_string()));
        clear("sess-roundtrip");
        assert_eq!(active("sess-roundtrip"), None);
    }

    #[test]
    fn empty_keys_are_ignored() {
        set_active("", "proj-x");
        assert_eq!(active(""), None);
        set_active("sess-empty-proj", "");
        assert_eq!(active("sess-empty-proj"), None);
    }

    #[test]
    fn latest_write_wins() {
        set_active("sess-latest", "proj-1");
        set_active("sess-latest", "proj-2");
        assert_eq!(active("sess-latest"), Some("proj-2".to_string()));
        clear("sess-latest");
    }

    #[test]
    fn superseded_epochs_are_pruned_but_the_current_one_survives() {
        let base = "agent:prune:main";
        set_active(base, "proj-e0");
        set_active("agent:prune:main:s1", "proj-e1");
        set_active("agent:prune:main:s2", "proj-e2");
        // A different chat whose key merely starts with the base must not be
        // swept up by the prefix match.
        set_active("agent:prune:main-notes", "proj-other");

        clear_superseded_epochs(base, 2);

        assert_eq!(active(base), None, "epoch 0 is dead once epoch 2 closed");
        assert_eq!(active("agent:prune:main:s1"), None);
        assert_eq!(
            active("agent:prune:main:s2"),
            Some("proj-e2".to_string()),
            "the closing epoch may still resume under the same key"
        );
        assert_eq!(
            active("agent:prune:main-notes"),
            Some("proj-other".to_string()),
            "a prefix-sharing key belongs to a different conversation"
        );
        clear("agent:prune:main:s2");
        clear("agent:prune:main-notes");
    }

    #[tokio::test]
    async fn purge_drops_the_binding_and_the_plan_file() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let manager =
            crate::memory::scratchpad::ScratchpadManager::new("proj-purge", "sess-purge-owner");
        manager.initialize(Some("Ship it")).await.unwrap();
        assert!(manager.exists());
        set_active("sess-purge-owner", "proj-purge");

        purge_session_scratchpad("sess-purge-owner").await;

        assert_eq!(active("sess-purge-owner"), None);
        assert!(
            !manager.exists(),
            "the deleted conversation's plan must not outlive it"
        );
    }

    #[tokio::test]
    async fn purge_spares_a_plan_another_session_still_owns() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let manager =
            crate::memory::scratchpad::ScratchpadManager::new("proj-shared", "sess-shared-a");
        manager.initialize(Some("Shared objective")).await.unwrap();
        set_active("sess-shared-a", "proj-shared");
        set_active("sess-shared-b", "proj-shared");

        purge_session_scratchpad("sess-shared-a").await;

        assert_eq!(active("sess-shared-a"), None);
        assert_eq!(active("sess-shared-b"), Some("proj-shared".to_string()));
        assert!(
            manager.exists(),
            "an explicitly named project is shareable; the surviving session still owns it"
        );
        clear("sess-shared-b");
    }

    #[test]
    fn read_write_store_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("bindings.json");
        let mut map = HashMap::new();
        map.insert("s1".to_string(), "p1".to_string());
        map.insert("s2".to_string(), "p2".to_string());
        write_store(&path, &map).unwrap();
        assert_eq!(read_store(&path), map);
    }

    #[test]
    fn read_store_tolerates_missing_and_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        // Missing file → empty.
        assert!(read_store(&dir.path().join("nope.json")).is_empty());
        // Corrupt file → empty (fail-open, no panic).
        let corrupt = dir.path().join("corrupt.json");
        std::fs::write(&corrupt, b"{not json").unwrap();
        assert!(read_store(&corrupt).is_empty());
    }

    #[test]
    fn boot_reload_restores_binding_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bindings.json");
        // Simulate a prior process that persisted a binding for a unique
        // session key (no global-map mutation → safe under parallel tests).
        let mut prior = HashMap::new();
        prior.insert("sess-after-restart".to_string(), "proj-resumed".to_string());
        write_store(&path, &prior).unwrap();

        // Boot of the "new" process loads the mirror into the live table.
        init_persistence(path.clone());
        assert_eq!(
            active("sess-after-restart"),
            Some("proj-resumed".to_string())
        );

        // And write-through keeps the mirror current for the next restart.
        set_active("sess-write-through", "proj-wt");
        assert_eq!(
            read_store(&path)
                .get("sess-write-through")
                .map(String::as_str),
            Some("proj-wt")
        );

        // Restore in-memory-only default so sibling parallel tests are
        // unaffected by the process-global store path.
        *STORE_PATH.lock().unwrap_or_else(|e| e.into_inner()) = None;
        clear("sess-after-restart");
        clear("sess-write-through");
    }
}
