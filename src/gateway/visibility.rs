//! Caller-visibility predicates for session ownership (P1 data isolation,
//! spec §5.4 唯一强制点).
//!
//! `SessionMetadata::owner_user_id` is `None` on legacy/pre-P1 rows and on
//! rows created outside any dispatch scope (cron, internal, A2A). Those rows
//! read as owned by the org-era single operator — adoption-by-absence, not a
//! missing value. [`effective_owner`] is the one place that rule is encoded;
//! both `SessionStore` backends' `SessionFilter::owner_visible_to` filter
//! call it, so the fallback can never drift between them.
//!
//! ALL user-visibility decisions for session-scoped RPCs live in this
//! module. The resolution itself is not re-derived here — it is the P0
//! `CALLER_USER` task-local ([`crate::gateway::caller_identity`]),
//! resolved once at `connect` and scoped around every dispatch
//! (`process_request`, both stations). This module only turns that single
//! resolution into a boolean/filter/response — "resolve once" (spec §5.4)
//! is satisfied by the task-local, not by anything added here. Any handler
//! that writes its own `meta.owner_user_id == caller` comparison instead of
//! calling [`session_visible`] (or filters `list_sessions` without setting
//! [`visible_owner_filter`] into `SessionFilter::owner_visible_to`) is
//! exactly the bypass this module exists to prevent.

use crate::gateway::caller_identity::current_caller_user;
use crate::gateway::protocol::{JsonRpcResponse, RESOURCE_NOT_FOUND};
use crate::gateway::router::SessionKey;
use crate::gateway::security::store::OWNER_USER_ID;
use crate::gateway::session_store::types::SessionMetadata;
use crate::gateway::session_store::SessionStore;

/// The user who effectively owns `meta` for visibility purposes: its stamped
/// `owner_user_id`, or [`OWNER_USER_ID`] for a legacy/pre-P1 row with no
/// scope stamp.
#[must_use]
pub fn effective_owner(meta: &SessionMetadata) -> &str {
    meta.owner_user_id.as_deref().unwrap_or(OWNER_USER_ID)
}

/// The owner a `sessions.list`-shaped query should restrict itself to, for
/// [`crate::gateway::session_store::types::SessionFilter::owner_visible_to`].
///
/// `None` when no `CALLER_USER` task-local is scoped around the current
/// dispatch — an internal/in-process caller (cron, background sweep, A2A,
/// or a direct in-process test) is unrestricted, matching the pre-P1
/// behavior for those callers exactly (zero-change guarantee for
/// single-user / internal use). `Some(u)` restricts the list to sessions
/// whose [`effective_owner`] is `u`.
#[must_use]
pub fn visible_owner_filter() -> Option<String> {
    current_caller_user()
}

/// Whether a record carrying a raw stamped `owner_user_id` is visible to the
/// current caller.
///
/// [`session_visible`] is this predicate applied to a [`SessionMetadata`];
/// this is the shape for the OTHER P1-stamped records that are not sessions
/// and so have no `SessionMetadata` to pass — a group-chat session, a loop, a
/// goal. Same rule, one implementation: an unrestricted caller sees
/// everything, `None` reads as [`OWNER_USER_ID`] (owner-by-absence), and a
/// scoped caller sees only their own. Do not re-derive it at a call site —
/// that is exactly the bypass this module exists to prevent.
#[must_use]
pub fn stamped_owner_visible(owner_user_id: Option<&str>) -> bool {
    match visible_owner_filter() {
        None => true,
        Some(caller) => caller == owner_user_id.unwrap_or(OWNER_USER_ID),
    }
}

/// Whether `meta` is visible to the current caller.
///
/// An unrestricted caller (see [`visible_owner_filter`]) sees every session.
/// A scoped caller sees only sessions whose [`effective_owner`] equals their
/// own user id — legacy rows (`owner_user_id: None`) read as owned by
/// [`OWNER_USER_ID`], so only that user (or an unrestricted caller) sees
/// them, matching Global Constraint 2.
#[must_use]
pub fn session_visible(meta: &SessionMetadata) -> bool {
    stamped_owner_visible(meta.owner_user_id.as_deref())
}

/// The response for an addressed-key visibility failure.
///
/// Byte-identical to what a genuinely missing key produces (Global
/// Constraint 4 — no existence oracle): every addressed-session handler in
/// this crate returns exactly this response both when the key does not
/// exist and when it exists but belongs to someone else. Callers must never
/// substitute a different message/code for either case.
#[must_use]
pub fn not_found_response(id: Option<serde_json::Value>) -> JsonRpcResponse {
    JsonRpcResponse::error(id, RESOURCE_NOT_FOUND, "session not found")
}

/// Whether a session that MAY OR MAY NOT exist yet is safe to address for
/// the current caller — the shape `chat.send`'s session resolution needs,
/// which is deliberately different from the addressed-key pattern above
/// ([`not_found_response`] callers): a session that has never been created
/// is not a denial, it is the ordinary "first message of a new
/// conversation" case (the run that follows creates it, stamped to the
/// caller by [`SessionMetadata::stamp_attribution`]). Only an EXISTING
/// session that belongs to someone else is refused.
///
/// Fails closed on a store error (Global Constraint 3), matching every
/// other predicate in this module — a caller must never be let through
/// because the visibility check itself couldn't complete.
pub async fn existing_session_is_visible(store: &dyn SessionStore, key: &SessionKey) -> bool {
    match store.get_metadata(key).await {
        Ok(Some(meta)) => session_visible(&meta),
        Ok(None) => true,
        Err(_) => false,
    }
}

/// Whether a partition-composed `agent_id` (spec §11-1c, Task 4's grammar:
/// `<base>__<suffix>` via [`crate::memory::project_scope::NS_SEP`]) is
/// visible to the current caller.
///
/// Split ONCE on the namespace separator:
/// - No suffix (a bare base id like `"main"`) → `true`. The org layer is
///   shared by design — every member reads the same base partition.
/// - Suffix starts with `"proj-"` → `true`. The legacy project-directory
///   feature ([`crate::memory::project_scope::project_namespace`]) is
///   org-tier, not per-user; it predates per-user scoping and stays shared.
/// - Any other suffix → visible only when it equals the caller's own user id
///   ([`visible_owner_filter`]), or when the caller is unrestricted
///   (`None` — internal/cron/A2A, matching every other predicate in this
///   module). A personal-scope suffix ([`crate::scope::ScopeId::Personal`])
///   IS the owning user's id verbatim (see that module's doc), so this is a
///   direct string comparison, not a second parse of the suffix.
///
/// Unknown suffix families (anything that is not `proj-` and does not match
/// the caller) fail closed for a scoped caller — there is no positive-match
/// arm for them, so e.g. a `p-*` (project scope, P2) partition is invisible
/// to every member until P2 adds the membership check that would let one in.
#[must_use]
pub fn partition_visible(partition_id: &str) -> bool {
    let Some((_base, suffix)) = partition_id.split_once(crate::memory::project_scope::NS_SEP)
    else {
        return true;
    };
    if suffix.starts_with("proj-") {
        return true;
    }
    match visible_owner_filter() {
        None => true,
        Some(caller) => suffix == caller,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use tempfile::TempDir;

    #[test]
    fn stamped_row_reads_its_own_owner() {
        let meta = SessionMetadata {
            owner_user_id: Some("u-alice".to_string()),
            ..Default::default()
        };
        assert_eq!(effective_owner(&meta), "u-alice");
    }

    #[test]
    fn legacy_row_reads_as_owner_by_absence() {
        let meta = SessionMetadata::default();
        assert_eq!(effective_owner(&meta), OWNER_USER_ID);
    }

    fn owned_by(owner: &str) -> SessionMetadata {
        SessionMetadata {
            owner_user_id: Some(owner.to_string()),
            ..Default::default()
        }
    }

    /// The full (caller task-local, meta.owner) → expected matrix pinned by
    /// the task brief. Each case scopes `CALLER_USER` around the read so the
    /// task-local state never leaks between cases.
    #[tokio::test]
    async fn session_visible_matrix() {
        // (None-unset, any) -> true: unrestricted internal caller.
        assert!(session_visible(&owned_by("u-alice")));
        assert!(session_visible(&SessionMetadata::default()));

        // (u-alice, Some(u-alice)) -> true.
        assert!(
            CALLER_USER
                .scope(Some("u-alice".to_string()), async {
                    session_visible(&owned_by("u-alice"))
                })
                .await
        );

        // (u-alice, Some(u-bob)) -> false.
        assert!(
            !CALLER_USER
                .scope(Some("u-alice".to_string()), async {
                    session_visible(&owned_by("u-bob"))
                })
                .await
        );

        // (u-alice, None) -> false: legacy rows belong to the owner, not alice.
        assert!(
            !CALLER_USER
                .scope(Some("u-alice".to_string()), async {
                    session_visible(&SessionMetadata::default())
                })
                .await
        );

        // (u-owner, None) -> true: the owner IS the legacy row's effective owner.
        assert!(
            CALLER_USER
                .scope(Some(OWNER_USER_ID.to_string()), async {
                    session_visible(&SessionMetadata::default())
                })
                .await
        );
    }

    #[tokio::test]
    async fn visible_owner_filter_is_none_outside_a_caller_scope() {
        assert_eq!(visible_owner_filter(), None);
    }

    #[tokio::test]
    async fn visible_owner_filter_carries_the_scoped_caller() {
        let seen = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                visible_owner_filter()
            })
            .await;
        assert_eq!(seen.as_deref(), Some("u-alice"));
    }

    fn store(temp: &TempDir) -> FileSessionStore {
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: temp.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap()
    }

    /// `chat.send`'s canonical deny case: alice already owns this session key,
    /// bob addresses it by name — must be refused before a run ever starts.
    #[tokio::test]
    async fn existing_session_is_visible_denies_a_foreign_owner() {
        let temp = TempDir::new().unwrap();
        let s = store(&temp);
        let key = SessionKey::main("chat-foreign");
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("u-alice")),
            s.get_or_create(&key),
        )
        .await
        .unwrap();

        let visible = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                existing_session_is_visible(&s, &key).await
            })
            .await;
        assert!(!visible, "bob must not be able to address alice's session");
    }

    /// A brand-new key (nothing created yet) is NOT a denial — this is the
    /// "first message of a new conversation" case chat.send hits on every
    /// fresh session, and it must proceed so the run can create+stamp it.
    #[tokio::test]
    async fn existing_session_is_visible_is_true_for_a_not_yet_created_key() {
        let temp = TempDir::new().unwrap();
        let s = store(&temp);
        let key = SessionKey::main("chat-brand-new");

        let visible = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                existing_session_is_visible(&s, &key).await
            })
            .await;
        assert!(
            visible,
            "a session that doesn't exist yet is never a denial"
        );
    }

    /// The owner can always address their own session.
    #[tokio::test]
    async fn existing_session_is_visible_allows_the_owner() {
        let temp = TempDir::new().unwrap();
        let s = store(&temp);
        let key = SessionKey::main("chat-own");
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("u-alice")),
            s.get_or_create(&key),
        )
        .await
        .unwrap();

        let visible = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                existing_session_is_visible(&s, &key).await
            })
            .await;
        assert!(visible);
    }

    #[test]
    fn not_found_response_carries_resource_not_found() {
        let resp = not_found_response(Some(serde_json::json!(1)));
        let err = resp.error.expect("error response");
        assert_eq!(err.code, RESOURCE_NOT_FOUND);
        assert_eq!(err.message, "session not found");
    }

    /// Global Constraint 4 in one assertion: a deny and a genuinely missing
    /// key must be byte-identical on the wire, so serialize both and compare.
    #[test]
    fn not_found_response_is_byte_identical_regardless_of_cause() {
        let missing = not_found_response(Some(serde_json::json!(7)));
        let denied = not_found_response(Some(serde_json::json!(7)));
        assert_eq!(
            serde_json::to_string(&missing).unwrap(),
            serde_json::to_string(&denied).unwrap()
        );
    }

    /// The full partition matrix pinned by the Task 7 brief: (suffix family,
    /// caller) → expected. Each case scopes `CALLER_USER` around the read so
    /// task-local state never leaks between cases.
    #[tokio::test]
    async fn partition_visible_matrix() {
        // No suffix at all: org layer, shared by design — visible to everyone,
        // scoped or not.
        assert!(partition_visible("main"));
        assert!(
            CALLER_USER
                .scope(Some("u-alice".to_string()), async {
                    partition_visible("main")
                })
                .await
        );

        // `proj-*`: legacy project-directory feature, org-tier — visible to
        // any scoped caller, not just its creator.
        assert!(
            CALLER_USER
                .scope(Some("u-alice".to_string()), async {
                    partition_visible("main__proj-deadbeef")
                })
                .await
        );
        assert!(
            CALLER_USER
                .scope(Some("u-bob".to_string()), async {
                    partition_visible("main__proj-deadbeef")
                })
                .await
        );

        // `u-*` (personal scope): visible to its own owner...
        assert!(
            CALLER_USER
                .scope(Some("u-alice".to_string()), async {
                    partition_visible("main__u-alice")
                })
                .await
        );
        // ...invisible to a different member...
        assert!(
            !CALLER_USER
                .scope(Some("u-bob".to_string()), async {
                    partition_visible("main__u-alice")
                })
                .await
        );
        // ...and visible to an unrestricted (internal/cron) caller.
        assert!(partition_visible("main__u-alice"));

        // Unknown suffix family (not `proj-`, not the caller's own id): fails
        // closed for a scoped member even though it superficially "looks
        // like" a partition suffix (e.g. a future `p-*` project scope before
        // P2 wires membership).
        assert!(
            !CALLER_USER
                .scope(Some("u-alice".to_string()), async {
                    partition_visible("main__p-somewhere")
                })
                .await
        );
        // ...but an unrestricted caller still sees it (zero-change guarantee
        // for internal/cron callers, matching every other predicate here).
        assert!(partition_visible("main__p-somewhere"));
    }
}
