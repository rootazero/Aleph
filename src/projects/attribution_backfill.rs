//! One-shot backfill of P1 attribution onto rooms created before it existed.
//!
//! ## Why this can exist at all
//!
//! Personal sessions from the pre-P1 era are genuinely unrecoverable: nothing
//! recorded who opened them, and the only honest reading is
//! adoption-by-absence (`visibility::effective_owner` treats a `NULL`
//! `owner_user_id` as the default owner's). Rooms are different, and the
//! difference is not a heuristic:
//!
//! [`ProjectStore::claim_session_key`] is the **sole** writer of
//! `projects.current_session_key`, and it writes only `WHERE
//! current_session_key IS NULL`. So a room's session key is *declared* in a
//! second table rather than guessed — the predicate "a project row whose
//! `current_session_key` names a session row with both attribution columns
//! `NULL`" never mistakes some other conversation for a room's.
//!
//! ## The population this CANNOT see (verified 2026-08-09, do not re-derive)
//!
//! That predicate is precise, not complete, and the gap is on the older side.
//! `handle_room_session` — the only producer of `current_session_key` — landed
//! **2026-08-07** (`546f76c46`); before it the `project_id → session_key` map
//! lived in each browser's `localStorage`, so the server never learned which
//! conversation belonged to a room. A room whose chat predates that commit
//! therefore has `current_session_key IS NULL`, and this pass `continue`s past
//! it without even counting it.
//!
//! Those rooms are unrecoverable *and* unidentifiable, and the second half is
//! why no counter is offered for them: `current_session_key IS NULL` is also
//! the ordinary state of a room **nobody has opened yet**. Discriminating the
//! two would mean guessing from `created_at`, i.e. a date heuristic, which is
//! the kind of enumeration that only describes the world on the day it was
//! written. A report that counted both together would be a confident wrong
//! answer, which is worse than the honest silence.
//!
//! The exposure this pass *does* cover is correspondingly narrow: a room whose
//! key was claimed (≥ 2026-08-07) but whose session row was created before
//! `run_loop::ensure_session_under_request_scope` fixed the cross-`spawn`
//! attribution loss (2026-08-08). Two days, not the "rooms created 08-06→08-08"
//! it is tempting to assume — the stamping writer (`fcbed9380`, 08-05) predates
//! rooms being creatable at all (`2f85faccc`, 08-06).
//!
//! Either way the damage was **visibility only**. Memory routing never depended
//! on these columns: turn-level writes resolve their partition from the run's
//! ambient scope (seeded from `request.metadata`, which carried the right
//! attribution the whole time) and `memory::flush` enumerates partitions by
//! `{agent}__` prefix. So a room in the uncovered population lost its roster's
//! view of the transcript, not the transcript's memory partition.
//!
//! ## What was actually broken
//!
//! Of the two columns, `owner_user_id` needs no backfill — `NULL` already reads
//! as the default owner. `scope_id` is the one with no benign default: `NULL`
//! means "org-era personal session", so `visibility::project_visible` never
//! fires for it and **the room's own conversation is invisible to every member
//! of its roster**. They can enter the room and find it empty. Both columns are
//! written together anyway, because `stamp_attribution` writes them as a pair
//! and every reader is entitled to assume that.
//!
//! ## Why it is fail-soft, and loudly so
//!
//! This migration GRANTS visibility, so it is deliberately conservative at
//! every step: the `IS NULL AND IS NULL` guard lives inside
//! [`SessionStore::backfill_attribution`] (not here), a room whose session key
//! does not parse is counted and skipped rather than guessed at, and a store
//! that does not implement the verb reports `unsupported` rather than "nothing
//! to do". The counts are logged because a silent zero is indistinguishable
//! from a backfill that never ran.

use crate::gateway::router::SessionKey;
use crate::gateway::security::store::OWNER_USER_ID;
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::SessionStore;
use crate::projects::ProjectStore;
use crate::scope::ScopeId;

/// What one backfill pass did. Every field is a distinct outcome on purpose:
/// "already stamped" and "could not be stamped" are the two answers a bare
/// success count would fuse.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BackfillReport {
    /// Project rows carrying a `current_session_key`.
    pub scanned: usize,
    /// Rows this pass wrote.
    pub stamped: usize,
    /// Rows that already had attribution — the steady state after the first run.
    pub already_stamped: usize,
    /// `current_session_key` values that are not parseable session keys.
    pub unparseable: usize,
    /// The store refused or failed the write.
    pub failed: usize,
}

impl BackfillReport {
    /// Nothing to say when a pass found no legacy rows AND hit no trouble —
    /// which is every boot after the first.
    #[must_use]
    pub const fn is_quiet(&self) -> bool {
        self.stamped == 0 && self.unparseable == 0 && self.failed == 0
    }
}

/// Stamp `project:<id>` scope onto every legacy room conversation.
///
/// Idempotent: a second pass reports every row as `already_stamped`.
pub async fn backfill_legacy_room_attribution(
    projects: &ProjectStore,
    sessions: &dyn SessionStore,
) -> BackfillReport {
    let mut report = BackfillReport::default();
    let rows = match projects.list() {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "room attribution backfill: cannot read the project table");
            return report;
        }
    };

    for project in rows {
        let Some(raw_key) = project.current_session_key.as_deref() else {
            continue;
        };
        report.scanned += 1;
        let Some(key) = SessionKey::from_key_string(raw_key) else {
            report.unparseable += 1;
            tracing::warn!(
                project = %project.id,
                session_key = %raw_key,
                "room attribution backfill: unparseable session key, skipping"
            );
            continue;
        };
        // The room's creator is the best-attested owner available, and for a
        // legacy row it is also what adoption-by-absence already resolves to,
        // so this half of the write changes no verdict. It is written anyway
        // so the pair is never half-set.
        let owner = project.owner_user_id.as_deref().unwrap_or(OWNER_USER_ID);
        let scope = ScopeId::Project(project.id.clone()).render();
        match sessions.backfill_attribution(&key, owner, &scope).await {
            Ok(true) => {
                report.stamped += 1;
                tracing::info!(
                    project = %project.id,
                    session_key = %raw_key,
                    scope = %scope,
                    "room attribution backfill: stamped a legacy room conversation"
                );
            }
            Ok(false) => report.already_stamped += 1,
            Err(SessionStoreError::Unsupported) => {
                report.failed += 1;
                tracing::warn!(
                    project = %project.id,
                    "room attribution backfill: this session store cannot stamp attribution"
                );
            }
            Err(e) => {
                report.failed += 1;
                tracing::warn!(
                    project = %project.id,
                    error = %e,
                    "room attribution backfill: write failed"
                );
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::projects::roster::TEST_GUARD as ROSTER_TEST_GUARD;

    /// Builds the state a migration actually meets: a room whose conversation
    /// was created BEFORE attribution existed. A fixture that stamps the row
    /// and then clears it is the same thing here, and it is the only way to
    /// produce a pre-migration row — the current code path cannot write one.
    async fn legacy_room(
        dir: &std::path::Path,
    ) -> (ProjectStore, FileSessionStore, String, SessionKey) {
        let store = ProjectStore::new(rusqlite::Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let project = store.add_for(dir, None, Some("u-alice")).unwrap();

        let sessions = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: dir.join("sessions"),
            ..FileSessionStoreConfig::default()
        })
        .expect("file session store");
        // Built, then round-tripped through the string form the project row
        // stores — so the parse this migration depends on is part of what the
        // test exercises, not an assumption it makes.
        let key = SessionKey::Main {
            agent_id: "main".into(),
            main_key: "legacy".into(),
            epoch: 0,
        };
        assert_eq!(
            SessionKey::from_key_string(&key.to_key_string()).as_ref(),
            Some(&key),
            "a stored current_session_key must parse back to the key that wrote it"
        );
        let mut meta = sessions.get_or_create(&key).await.unwrap();
        // Force the pre-P1 shape regardless of any ambient scope.
        meta.owner_user_id = None;
        meta.scope_id = None;
        sessions
            .write_metadata(&key.to_key_string(), &meta)
            .await
            .unwrap();

        store
            .claim_session_key(&project.id, &key.to_key_string())
            .unwrap();
        (store, sessions, project.id, key)
    }

    #[tokio::test]
    async fn a_legacy_room_conversation_gains_its_project_scope_exactly_once() {
        let _guard = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (projects, sessions, project_id, key) = legacy_room(dir.path()).await;

        let first = backfill_legacy_room_attribution(&projects, &sessions).await;
        assert_eq!(first.scanned, 1);
        assert_eq!(first.stamped, 1, "the legacy row must be stamped");
        assert_eq!(first.failed, 0);

        let meta = sessions.get_metadata(&key).await.unwrap().expect("row");
        assert_eq!(
            meta.scope_id.as_deref(),
            Some(format!("project:{project_id}").as_str()),
            "scope_id is the column that makes the room visible to its roster"
        );
        assert_eq!(meta.owner_user_id.as_deref(), Some("u-alice"));

        // Idempotence is what makes this safe to run on every boot.
        let second = backfill_legacy_room_attribution(&projects, &sessions).await;
        assert_eq!(second.stamped, 0);
        assert_eq!(second.already_stamped, 1);
        assert!(second.is_quiet());
    }

    /// The guard that keeps this from being a re-scoping tool. It lives inside
    /// the store method, so it holds no matter who calls it — including a
    /// future caller that has not read this module.
    #[tokio::test]
    async fn an_already_attributed_session_is_never_rewritten() {
        let _guard = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (projects, sessions, _project_id, key) = legacy_room(dir.path()).await;

        let mut meta = sessions.get_metadata(&key).await.unwrap().expect("row");
        meta.owner_user_id = Some("u-bob".into());
        meta.scope_id = Some("personal:u-bob".into());
        sessions
            .write_metadata(&key.to_key_string(), &meta)
            .await
            .unwrap();

        let report = backfill_legacy_room_attribution(&projects, &sessions).await;
        assert_eq!(report.stamped, 0);
        assert_eq!(report.already_stamped, 1);

        let after = sessions.get_metadata(&key).await.unwrap().expect("row");
        assert_eq!(after.owner_user_id.as_deref(), Some("u-bob"));
        assert_eq!(after.scope_id.as_deref(), Some("personal:u-bob"));
    }
}
