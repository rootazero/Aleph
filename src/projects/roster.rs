//! In-process projection of the project roster.
//!
//! The SSOT is the `project_members` table; this is a read-optimised snapshot
//! that [`crate::projects::ProjectStore`] republishes inside its own write
//! lock, exactly like `MessageProjector` republishes the `messages` table from
//! `session_events`. Never write to it from anywhere else — a second writer is
//! a second source of truth.
//!
//! It exists because `gateway::visibility::session_visible` is a SYNCHRONOUS
//! predicate called once per session while filtering a list. Querying SQLite
//! there would put N round-trips on the list path; making it async would spread
//! virally through every P1 predicate.
//!
//! Cross-process caveat: a second process writing `projects.db` will not be
//! seen here. Roster mutation is RPC-only (this process) today. **If a CLI
//! roster subcommand is ever added it MUST go through IPC, not straight to the
//! database.**
//!
//! ## Test isolation
//!
//! [`publish`] REPLACES the whole snapshot — it does not merge — because the
//! store republishes its entire `project_members` table on every write. So two
//! `cargo test` threads each holding their own in-memory store will erase each
//! other's projection, and per-test unique project ids do **not** help: the
//! second `publish` drops the first test's project outright, not merely its
//! members. Every test that reaches a store write therefore serialises on
//! [`TEST_GUARD`].

use std::collections::{HashMap, HashSet};
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Default, Clone)]
pub struct RosterSnapshot {
    members: HashMap<String, HashSet<String>>,
}

impl RosterSnapshot {
    #[must_use]
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut members: HashMap<String, HashSet<String>> = HashMap::new();
        for (project_id, user_id) in pairs {
            members.entry(project_id).or_default().insert(user_id);
        }
        Self { members }
    }
}

fn cell() -> &'static RwLock<RosterSnapshot> {
    static ROSTER: OnceLock<RwLock<RosterSnapshot>> = OnceLock::new();
    ROSTER.get_or_init(|| RwLock::new(RosterSnapshot::default()))
}

/// Replace the projection. Called by `ProjectStore` after every mutation and
/// once at `migrate()`.
pub fn publish(snapshot: RosterSnapshot) {
    let mut guard = cell().write().unwrap_or_else(|e| e.into_inner());
    *guard = snapshot;
}

/// Whether `user_id` is on `project_id`'s roster. `false` for an unknown
/// project and for a never-published projection (tests, CLI) — fail closed.
/// Every caller checks the unrestricted-caller arm BEFORE reaching this.
#[must_use]
pub fn is_member(project_id: &str, user_id: &str) -> bool {
    let guard = cell().read().unwrap_or_else(|e| e.into_inner());
    guard
        .members
        .get(project_id)
        .is_some_and(|m| m.contains(user_id))
}

/// Serialises every test that publishes a roster.
///
/// [`publish`] REPLACES the snapshot rather than merging into it (the store
/// republishes its whole `project_members` table on every write), so two
/// parallel test threads holding their own in-memory stores erase each other's
/// projection. Per-test unique project ids do NOT help: the second publish
/// drops the first test's project outright, not merely its members.
///
/// Lives here rather than in `store::tests` because the state being guarded is
/// this module's, and because `projects::store` is a private module — the
/// gateway's visibility tests need the same guard.
#[cfg(test)]
pub static TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Every project `user_id` belongs to, sorted for deterministic SQL.
#[must_use]
pub fn projects_of(user_id: &str) -> Vec<String> {
    let guard = cell().read().unwrap_or_else(|e| e.into_inner());
    let mut ids: Vec<String> = guard
        .members
        .iter()
        .filter(|(_, m)| m.contains(user_id))
        .map(|(p, _)| p.clone())
        .collect();
    ids.sort();
    ids
}
