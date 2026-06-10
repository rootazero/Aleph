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

/// Mirror the table to disk when persistence is enabled. No-op otherwise.
fn persist(map: &Bindings) {
    if let Some(path) = store_path() {
        if let Err(e) = write_store(&path, map) {
            tracing::warn!(error = %e, "scratchpad_registry: failed to persist bindings");
        }
    }
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
    let snapshot = {
        let mut map = active_lock();
        map.insert(session_key.to_string(), project_id.to_string());
        map.clone()
    };
    persist(&snapshot);
}

/// The active execution-list `project_id` for `session_key`, if any.
pub fn active(session_key: &str) -> Option<String> {
    if session_key.is_empty() {
        return None;
    }
    active_lock().get(session_key).cloned()
}

/// Drop the pointer for `session_key` (e.g. the scratchpad was cleared).
pub fn clear(session_key: &str) {
    let snapshot = {
        let mut map = active_lock();
        map.remove(session_key);
        map.clone()
    };
    persist(&snapshot);
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
