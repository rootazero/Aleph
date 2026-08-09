//! Caller-visibility predicates for session ownership (P1 data isolation,
//! spec §5.4 唯一强制点).
//!
//! `SessionMetadata::owner_user_id` is `None` on legacy/pre-P1 rows and on
//! rows created outside any dispatch scope (cron, internal, A2A). Those rows
//! read as owned by the org-era single operator — adoption-by-absence, not a
//! missing value. [`owner_or_legacy`] is the one place that rule is encoded;
//! [`owner_and_scope_visible_to`] (the body behind both `SessionStore`
//! backends' `SessionFilter::owner_visible_to` filter AND behind the event
//! bus's per-frame check — see [`crate::gateway::event_visibility`]) and
//! [`stamped_owner_visible`] (used by every non-session P1-stamped record)
//! both call it, so the fallback can never drift between them.
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

/// Adoption-by-absence, encoded exactly once: a stamped `owner_user_id`, or
/// [`OWNER_USER_ID`] when the record predates P1 / was created outside any
/// dispatch scope.
///
/// Every other predicate in this module resolves ownership through this
/// function. A second `unwrap_or(OWNER_USER_ID)` anywhere is a second
/// derivation of the legacy rule — it agrees today and silently diverges the
/// day that rule changes.
#[must_use]
pub fn owner_or_legacy(owner_user_id: Option<&str>) -> &str {
    owner_user_id.unwrap_or(OWNER_USER_ID)
}

/// The user who effectively owns `meta` for visibility purposes: its stamped
/// `owner_user_id`, or [`OWNER_USER_ID`] for a legacy/pre-P1 row with no
/// scope stamp.
#[must_use]
pub fn effective_owner(meta: &SessionMetadata) -> &str {
    owner_or_legacy(meta.owner_user_id.as_deref())
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
///
/// # Owner-only is the P2 boundary here, not an unfinished predicate
///
/// This predicate and [`ambient_owner_visible`] are **owner-only by design**,
/// and that is a product boundary rather than a gap: **a project room does not
/// own its members' background work.** A loop, goal, cron job or group-chat
/// session created inside a room stays visible to the person who started it,
/// never to the room's roster. SECURITY.md's P2 section enumerates exactly four
/// roster-answered questions — session visibility, memory-partition visibility,
/// event delivery, RPC access — and background work is deliberately not among
/// them. Ruled 2026-08-07.
///
/// The scope fact needed to answer otherwise is already **persisted, and
/// deliberately unread by these predicates**: `looping::LoopState::scope_id`,
/// `goal::Goal::scope_id` and `tasks::cron::CronJob::scope_id` all carry
/// `project:<id>` for a unit started inside a room (loops and goals stamp it in
/// `with_owner_scope` from `scope::current_scope()` at creation). It
/// has execution-side readers — `goal_wait::rehydrate_owner_scope` and
/// `cron::executor` both rebuild the run's scope from it through
/// `ScopeAttribution::from_persisted` — so it is a live field, not a stub. The
/// loop's copy currently has NO reader at all; it is carried forward across
/// state transitions (`looping::mod`) and read by nobody, which is the one most
/// likely to be mistaken for a broken wire.
///
/// None of that is a severed wire, and none of it should be "reconnected" here.
/// Widening these predicates would make one member's pursuit appear in another
/// member's list — a change to what a room means, needing a product ruling
/// first. If one ever arrives, the shape is ONE scope-aware sibling in this
/// module taking `(owner_user_id, scope_id)` and delegating to the same
/// [`crate::projects::roster::is_member`] call [`project_visible`] makes —
/// never a second inlined membership check at the call sites.
///
/// Note the direction of failure: owner-only is fail-CLOSED. Someone reading
/// "the room can't see my loop" as a bug is reading a decision.
#[must_use]
pub fn stamped_owner_visible(owner_user_id: Option<&str>) -> bool {
    match visible_owner_filter() {
        None => true,
        Some(caller) => caller == owner_or_legacy(owner_user_id),
    }
}

/// [`stamped_owner_visible`]'s sibling for records reachable from BOTH the
/// gateway and an agent run's tools — currently team ownership.
///
/// Same rule, same [`owner_or_legacy`] derivation; the only difference is the
/// resolver: [`crate::scope::ambient_owner`] instead of
/// [`visible_owner_filter`]. That difference is load-bearing and deliberate.
/// `CALLER_USER` is dead inside a spawned run, so a team predicate built on
/// [`stamped_owner_visible`] would be fail-open for every `team_*` tool call —
/// the Panel half would be enforced and the chat half wide open, which is not
/// "tightened", it is a different-looking hole. Records reachable ONLY through
/// the gateway keep using [`stamped_owner_visible`]: the connection identity is
/// the stronger claim, and widening every predicate to accept a run-seeded
/// scope would be a silent semantics change to surfaces that never needed it.
#[must_use]
pub fn ambient_owner_visible(owner_user_id: Option<&str>) -> bool {
    match crate::scope::ambient_owner() {
        None => true,
        Some(actor) => actor == owner_or_legacy(owner_user_id),
    }
}

/// The user a visibility check made from INSIDE an agent run acts for — the
/// run-side twin of [`visible_owner_filter`].
///
/// `CALLER_USER` is dead past the `tokio::spawn` every run crosses, so a
/// predicate built on [`visible_owner_filter`] alone is fail-OPEN for every
/// tool call (see [`ambient_owner_visible`], which exists for the same reason).
/// The resolution order is the one the rest of the crate already established:
///
/// 1. [`crate::scope::ambient_room_author`] — this turn's SPEAKER, when the run
///    is a project room. In a room the ambient scope's `owner_user_id` names the
///    room's CREATOR, identically for every member (that is what shares the
///    memory partition), so asking the roster about it answers "does this room
///    still exist" rather than "may this person read". The speaker is the actor.
/// 2. [`crate::scope::ambient_owner`] — the gateway caller when one is live,
///    else the run's ambient owner. `None` outside a room, so this is the
///    ordinary answer for a personal / org run.
///
/// `None` means "no ambient actor" and is deliberately unrestricted (cron,
/// background sweeps, in-process tests), matching every other predicate here.
#[must_use]
pub fn ambient_actor() -> Option<String> {
    crate::scope::ambient_room_author().or_else(crate::scope::ambient_owner)
}

/// Whether the current gateway caller may see records scoped to `project_id`
/// (P2 project rooms).
///
/// **Membership IS the authorization** (spec §6.1) — v1 has no per-resource
/// grants, so "is the actor on this room's roster" is the whole predicate.
/// The unrestricted arm comes first, exactly as in every other predicate in
/// this module, so cron / A2A / in-process callers are unchanged.
#[must_use]
pub fn project_visible(project_id: &str) -> bool {
    project_visible_to(project_id, visible_owner_filter().as_deref())
}

/// Whether `meta` is visible to `actor` — the EXPLICIT-actor form of
/// [`session_visible`].
///
/// List filtering receives the actor as a
/// [`SessionFilter::owner_visible_to`](crate::gateway::session_store::types::SessionFilter)
/// string rather than through the task-local, and both `SessionStore` backends
/// filter in memory, so this is the one predicate they share. Keeping it here
/// rather than at the two `retain` sites is what stops the project rule from
/// being enforced on one backend and not the other.
///
/// A project-scoped session is a shared room: its `owner_user_id` records WHO
/// CREATED IT, which is not the visibility question. Ask the roster instead —
/// otherwise a member sees the room's messages (they can address the session)
/// but the room never appears in their session list, with no error anywhere.
#[must_use]
pub fn session_visible_to(meta: &SessionMetadata, actor: &str) -> bool {
    owner_and_scope_visible_to(
        meta.owner_user_id.as_deref(),
        meta.scope_id.as_deref(),
        actor,
    )
}

/// [`session_visible_to`]'s rule with its two inputs passed explicitly, for the
/// one caller that holds those fields without a [`SessionMetadata`] around
/// them: the event-delivery path
/// ([`crate::gateway::event_visibility`]), whose per-session cache stores this
/// pair rather than a whole metadata row.
///
/// Widening the signature rather than letting that path re-derive the rule is
/// the point. Event delivery kept the pre-P2 owner-equality rule for exactly
/// as long as it had its own copy of it, and nothing failed — a room's live
/// frames simply never reached anyone but its creator. With the rule in one
/// body, a third input to the room-vs-personal decision is a compile error at
/// both call sites instead of a silent divergence.
#[must_use]
pub fn owner_and_scope_visible_to(
    owner_user_id: Option<&str>,
    scope_id: Option<&str>,
    actor: &str,
) -> bool {
    if let Some(crate::scope::ScopeId::Project(p)) = scope_id.and_then(crate::scope::ScopeId::parse)
    {
        return crate::projects::roster::is_member(&p, actor);
    }
    owner_or_legacy(owner_user_id) == actor
}

/// Whether `meta` is visible to the current caller.
///
/// An unrestricted caller (see [`visible_owner_filter`]) sees every session.
/// A scoped caller sees their own sessions — legacy rows
/// (`owner_user_id: None`) read as owned by [`OWNER_USER_ID`], so only that
/// user (or an unrestricted caller) sees them, matching Global Constraint 2 —
/// plus every session in a project room they are on the roster of.
#[must_use]
pub fn session_visible(meta: &SessionMetadata) -> bool {
    match visible_owner_filter() {
        None => true,
        Some(caller) => session_visible_to(meta, &caller),
    }
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

/// Whether the TRANSCRIPT of `session_key` may be read by the current agent
/// run — the predicate every cross-session search surface owes.
///
/// Shape notes, each of which is a defect if taken from somewhere else:
///
/// - **Resolver is [`ambient_actor`], not [`visible_owner_filter`].** The only
///   caller is a builtin tool, i.e. the far side of the run's `tokio::spawn`,
///   where `CALLER_USER` is `None` — a gate built on it would be constantly
///   true, which is the same as not having one.
/// - **A key that does not parse, a row that is gone, and a store error are all
///   DENIALS.** This answers "may I quote this transcript", so "I cannot work
///   out whose it is" must never read as "it is everyone's" — the opposite
///   direction from [`existing_session_is_visible`], which answers "may I
///   address this key" and where a not-yet-created session is the ordinary
///   first message of a new conversation.
/// - **The rule itself is [`session_visible_to`]**, the same body both
///   `SessionStore` backends' list filter goes through, so a room's transcript
///   follows the roster here exactly as it does in `sessions.list`.
pub async fn ambient_transcript_visible(store: &dyn SessionStore, session_key: &str) -> bool {
    let Some(actor) = ambient_actor() else {
        return true;
    };
    let Some(key) = SessionKey::from_key_string(session_key) else {
        return false;
    };
    match store.get_metadata(&key).await {
        Ok(Some(meta)) => session_visible_to(&meta, &actor),
        Ok(None) | Err(_) => false,
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
/// Unknown suffix families (anything that is not `proj-`, not `p-`, and does
/// not match the caller) fail closed for a scoped caller — there is no
/// positive-match arm for them.
#[must_use]
pub fn partition_visible(partition_id: &str) -> bool {
    partition_visible_to(partition_id, visible_owner_filter().as_deref())
}

/// [`partition_visible`]'s twin for surfaces reached from INSIDE an agent run
/// — memory assembly, the builtin tools — where `CALLER_USER` is dead and
/// [`visible_owner_filter`] would therefore be a constantly-true predicate.
/// Resolver: [`ambient_actor`]. Same rule, one body.
#[must_use]
pub fn ambient_partition_visible(partition_id: &str) -> bool {
    partition_visible_to(partition_id, ambient_actor().as_deref())
}

/// [`partition_visible`] with the actor supplied explicitly — the twin of
/// [`session_visible_to`], and needed for the same reason one level down.
///
/// `partition_visible` resolves its actor through [`visible_owner_filter`],
/// which reads only `CALLER_USER`. That task-local is **dead inside a spawned
/// run** (`scope/mod.rs` says so in as many words), and every tool call happens
/// inside one — so a tool author reaching for the obvious predicate would get a
/// silent always-true. The gateway gates ~20 caller-supplied partition ids with
/// it; the tool face gated none, and `memory_search`'s `workspace` /
/// `workspaces` / `cross_workspace` arguments are model-supplied and reach
/// every partition on disk.
///
/// Tools must therefore pass [`ambient_actor`]. Note the resolver is
/// `ambient_actor`, **not** [`crate::scope::ambient_owner`]: the latter falls
/// back to the run-seeded `ScopeAttribution`, whose `owner_user_id` inside a
/// project room names the room's CREATOR identically for every member. A tool
/// handed that value asks "may the room's creator read this", which admits the
/// creator's PERSONAL partitions to every member of the room and filters out
/// the member's own. `ambient_actor` prefers the turn's speaker in a room and
/// is byte-identical to `ambient_owner` everywhere else.
///
/// `None` = unrestricted (internal / cron / A2A), matching every other
/// predicate in this module.
#[must_use]
pub fn partition_visible_to(partition_id: &str, actor: Option<&str>) -> bool {
    let Some((_base, suffix)) = partition_id.split_once(crate::memory::project_scope::NS_SEP)
    else {
        return true;
    };
    if suffix.starts_with("proj-") {
        return true;
    }
    // `p-*` (project scope): the roster decides. Checked AFTER `proj-` so the
    // legacy directory family keeps its org-tier ruling — note `"proj-…"` does
    // not start with `"p-"`, so the two families cannot collide (pinned by
    // `projects::store::tests::the_project_id_family_cannot_collide_with_the_legacy_directory_family`).
    if suffix.starts_with("p-") {
        return project_visible_to(suffix, actor);
    }
    match actor {
        None => true,
        Some(caller) => suffix == caller,
    }
}

/// [`project_visible`] with the actor supplied explicitly. Same split, same
/// reason — see [`partition_visible_to`].
#[must_use]
pub fn project_visible_to(project_id: &str, actor: Option<&str>) -> bool {
    match actor {
        None => true,
        Some(caller) => crate::projects::roster::is_member(project_id, caller),
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

    // ── P2 project rooms ────────────────────────────────────────────────

    /// Build a room owned by alice with `extra` also on the roster.
    ///
    /// Holds `roster::TEST_GUARD` for the caller's lifetime — `publish`
    /// replaces the process-global snapshot, so parallel roster tests erase
    /// each other without it.
    fn room_with(extra: &[&str]) -> (crate::projects::Project, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store =
            crate::projects::ProjectStore::new(rusqlite::Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let p = store.create("room", Some("u-alice"), None).unwrap();
        for u in extra {
            store.add_member(&p.id, u).unwrap();
        }
        (p, guard)
    }

    fn project_session(project_id: &str, creator: &str) -> SessionMetadata {
        SessionMetadata {
            owner_user_id: Some(creator.to_string()),
            scope_id: Some(crate::scope::ScopeId::Project(project_id.to_string()).render()),
            ..Default::default()
        }
    }

    /// The whole point of a room: a member who did NOT create the session can
    /// still see it. Ownership answers "who made this", not "who may look".
    #[tokio::test]
    async fn a_project_session_is_visible_to_every_member_not_just_its_creator() {
        let (p, _guard) = room_with(&["u-bob"]);
        let meta = project_session(&p.id, "u-alice");

        assert!(
            CALLER_USER
                .scope(Some("u-bob".to_string()), async { session_visible(&meta) })
                .await,
            "membership, not ownership, decides for a project session"
        );
        assert!(
            !CALLER_USER
                .scope(Some("u-carol".to_string()), async {
                    session_visible(&meta)
                })
                .await,
            "a non-member sees nothing"
        );
        assert!(
            session_visible(&meta),
            "unrestricted internal callers are unchanged"
        );
    }

    /// spec §10: 移出项目成员立即失去可见性. No cache to invalidate, no
    /// reconnect required.
    #[tokio::test]
    async fn removing_a_member_revokes_visibility_immediately() {
        let guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store =
            crate::projects::ProjectStore::new(rusqlite::Connection::open_in_memory().unwrap());
        store.create_schema().unwrap();
        let p = store.create("room", Some("u-alice"), None).unwrap();
        store.add_member(&p.id, "u-bob").unwrap();
        let meta = project_session(&p.id, "u-alice");

        assert!(
            CALLER_USER
                .scope(Some("u-bob".to_string()), async { session_visible(&meta) })
                .await
        );
        store.remove_member(&p.id, "u-bob").unwrap();
        assert!(
            !CALLER_USER
                .scope(Some("u-bob".to_string()), async { session_visible(&meta) })
                .await,
            "revocation must be immediate"
        );
        drop(guard);
    }

    /// The list path and the addressed path must agree. They are different
    /// functions (`session_visible_to` takes the actor explicitly because both
    /// store backends filter in memory from a `SessionFilter` string), so this
    /// pins that they cannot drift.
    #[tokio::test]
    async fn the_explicit_actor_form_agrees_with_the_task_local_form() {
        let (p, _guard) = room_with(&["u-bob"]);
        let meta = project_session(&p.id, "u-alice");

        for (actor, expected) in [("u-bob", true), ("u-carol", false)] {
            assert_eq!(session_visible_to(&meta, actor), expected);
            assert_eq!(
                CALLER_USER
                    .scope(Some(actor.to_string()), async { session_visible(&meta) })
                    .await,
                expected,
                "actor {actor}"
            );
        }
    }

    /// A personal session inside a member's account is NOT visible to their
    /// room-mates. Sharing a project does not share everything else.
    #[tokio::test]
    async fn sharing_a_room_does_not_share_personal_sessions() {
        let (_p, _guard) = room_with(&["u-bob"]);
        let personal = SessionMetadata {
            owner_user_id: Some("u-alice".to_string()),
            scope_id: Some(crate::scope::ScopeId::Personal("u-alice".into()).render()),
            ..Default::default()
        };
        assert!(
            !CALLER_USER
                .scope(Some("u-bob".to_string()), async {
                    session_visible(&personal)
                })
                .await
        );
    }

    /// The memory partition follows the same roster.
    #[tokio::test]
    async fn a_project_partition_follows_the_roster() {
        let (p, _guard) = room_with(&[]);
        let partition = format!("main__{}", p.id);

        assert!(
            CALLER_USER
                .scope(Some("u-alice".to_string()), async {
                    partition_visible(&partition)
                })
                .await
        );
        assert!(
            !CALLER_USER
                .scope(Some("u-bob".to_string()), async {
                    partition_visible(&partition)
                })
                .await
        );
        // The legacy project-DIRECTORY family is a different thing and keeps
        // its org-tier ruling — it must not be swept up by the new `p-` arm.
        assert!(
            CALLER_USER
                .scope(Some("u-bob".to_string()), async {
                    partition_visible("main__proj-deadbeef")
                })
                .await
        );
    }

    // ── run-side resolver (memory assembly, builtin tools) ──────────────

    /// The probe every run-side gate depends on, pinned: inside an agent run
    /// `CALLER_USER` is dead, so a predicate resolved through
    /// [`visible_owner_filter`] is CONSTANTLY TRUE there — the textbook
    /// "恒真的谓词等于没判". [`ambient_actor`] is the resolver that is live.
    #[tokio::test]
    async fn the_run_side_resolver_is_live_where_the_gateway_one_is_dead() {
        let alice = crate::scope::ScopeAttribution::personal("u-alice");
        let (gateway_side, run_side) = crate::scope::with_scope(Some(alice.clone()), async {
            (visible_owner_filter(), ambient_actor())
        })
        .await;
        assert_eq!(
            gateway_side, None,
            "CALLER_USER does not survive the run's spawn"
        );
        assert_eq!(run_side.as_deref(), Some("u-alice"));

        // The consequence, spelled out on a real partition: from inside
        // alice's run the gateway-side predicate admits BOB's partition.
        let (gateway_says, ambient_says) = crate::scope::with_scope(Some(alice), async {
            (
                partition_visible("main__u-bob"),
                ambient_partition_visible("main__u-bob"),
            )
        })
        .await;
        assert!(gateway_says, "…which is why it must not be used here");
        assert!(!ambient_says);
    }

    /// In a room the ambient scope's owner is the room's CREATOR, identically
    /// for every member, so the roster question has to be asked about the
    /// SPEAKER. `ambient_actor` prefers `scope::ambient_room_author` for
    /// exactly this reason.
    #[tokio::test]
    async fn the_run_side_resolver_prefers_the_rooms_speaker_over_its_creator() {
        let in_room = crate::scope::ScopeAttribution {
            owner_user_id: "u-alice".to_string(),
            scope: crate::scope::ScopeId::Project("p-room".to_string()),
        };
        let seen = crate::scope::with_scope(
            Some(in_room),
            crate::scope::with_room_author(Some("u-bob".to_string()), async { ambient_actor() }),
        )
        .await;
        assert_eq!(seen.as_deref(), Some("u-bob"));
    }

    /// `session_search`'s gate. Note the direction of the last two cases: an
    /// input this cannot attribute is a DENIAL, the opposite of
    /// [`existing_session_is_visible`].
    #[tokio::test]
    async fn ambient_transcript_visible_denies_a_foreign_owner_inside_a_run() {
        let temp = TempDir::new().unwrap();
        let s = store(&temp);
        let key = SessionKey::main("transcript-alice");
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("u-alice")),
            s.get_or_create(&key),
        )
        .await
        .unwrap();
        let key_string = key.to_key_string();

        let as_run = |user: &str, target: &str| {
            let attr = crate::scope::ScopeAttribution::personal(user);
            let store = &s;
            let target = target.to_string();
            // No `CALLER_USER` — that is the shape a run actually has.
            crate::scope::with_scope(Some(attr), async move {
                ambient_transcript_visible(store, &target).await
            })
        };

        assert!(
            !as_run("u-bob", &key_string).await,
            "bob's run must not read alice's transcript"
        );
        assert!(as_run("u-alice", &key_string).await, "…hers still is");
        assert!(
            !as_run("u-alice", "not-a-session-key").await,
            "an unparseable key is a denial, not a pass"
        );
        assert!(
            !as_run(
                "u-alice",
                &SessionKey::main("never-created").to_key_string()
            )
            .await,
            "a row we cannot attribute is a denial, not a pass"
        );
    }

    /// An unknown room id is invisible, not an error and not a grant.
    #[tokio::test]
    async fn an_unknown_project_is_invisible_to_a_scoped_caller() {
        assert!(
            !CALLER_USER
                .scope(Some("u-alice".to_string()), async {
                    project_visible("p-never-created")
                })
                .await
        );
        assert!(
            project_visible("p-never-created"),
            "…but an unrestricted caller is unchanged"
        );
    }

    /// The full partition matrix pinned by the Task 7 brief: (suffix family,
    /// caller) → expected. Each case scopes `CALLER_USER` around the read so
    /// task-local state never leaks between cases.
    /// The twin exists because `CALLER_USER` is dead inside a spawned run, and
    /// every tool call happens inside one — so a tool reaching for
    /// `partition_visible` would get a silent always-true. This asserts the two
    /// forms agree when the task-local IS set, and that the explicit form keeps
    /// working when it is not (the case that matters).
    #[tokio::test]
    async fn the_explicit_actor_twin_answers_where_the_task_local_is_dead() {
        // No task-local anywhere — a spawned run. The task-local form cannot
        // tell alice's partition from bob's; the explicit form can.
        assert!(
            partition_visible("main__u-bob"),
            "the task-local form is unrestricted with no CALLER_USER — this is \
             the fail-open a tool would have inherited"
        );
        assert!(!partition_visible_to("main__u-bob", Some("u-alice")));
        assert!(partition_visible_to("main__u-alice", Some("u-alice")));

        // `None` actor stays unrestricted, matching every sibling predicate
        // (cron / A2A / internal).
        assert!(partition_visible_to("main__u-bob", None));

        // Org and legacy-project families are unaffected by the actor.
        for id in ["main", "main__proj-deadbeef"] {
            assert!(partition_visible_to(id, Some("u-alice")));
            assert!(partition_visible_to(id, Some("u-bob")));
        }

        // And the two forms agree when the task-local IS the actor.
        for id in ["main", "main__proj-x", "main__u-alice", "main__u-bob"] {
            let via_task_local = CALLER_USER
                .scope(Some("u-alice".to_string()), async { partition_visible(id) })
                .await;
            assert_eq!(
                via_task_local,
                partition_visible_to(id, Some("u-alice")),
                "the two forms must not drift for {id}"
            );
        }
    }

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

        // A `p-` suffix that names no room on this caller's roster fails
        // closed — `partition_visible` never grants on the shape of a suffix,
        // only on the roster behind it.
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

    // -- the actor census ---------------------------------------------------

    /// Every file that resolves an actor through [`crate::scope::ambient_owner`]
    /// directly, with the reason it is allowed to.
    ///
    /// The two questions this module answers look identical at a call site and
    /// are not:
    ///
    /// - **"who owns this row"** — stamping `owner_user_id` on something being
    ///   created, or reading it back to compare against a stored owner.
    ///   `ambient_owner` is right, and inside a room it deliberately names the
    ///   room (that is what makes members share one partition).
    /// - **"who is asking"** — every visibility predicate. `ambient_owner` is
    ///   WRONG here: in a project room it resolves to the room's CREATOR for
    ///   every member, so the predicate answers about the wrong human — the
    ///   creator's personal data is admitted to every member and the member's
    ///   own is filtered out. The resolver is [`ambient_actor`].
    ///
    /// Four predicates acquired that defect independently before 2026-08-09,
    /// each by grepping for precedent and finding a sentence that said
    /// `ambient_owner` was the speaker. A new entry here is not automatically
    /// wrong — it just has to say which of the two questions it is answering.
    const AMBIENT_OWNER_CENSUS: &[(&str, &str)] = &[
        (
            "src/scope/mod.rs",
            "defines it, and defines `ambient_room_author` next to it",
        ),
        (
            "src/gateway/visibility.rs",
            "`ambient_owner_visible` (owner-keyed background work) and `ambient_actor`'s fallback arm",
        ),
        (
            "src/projects/store.rs",
            "who-owns: stamps / matches the directory-catalogue row's owner",
        ),
        (
            "src/gateway/handlers/projects.rs",
            "who-owns: stamps a new project row; runs in gateway dispatch where CALLER_USER is live",
        ),
        (
            "src/teams/store.rs",
            "who-owns: stamps a new team's owner_user_id",
        ),
        (
            "src/gateway/voice/streaming/relay.rs",
            "who-owns: mints and matches a voice stream's owner; runs in gateway dispatch where CALLER_USER is live",
        ),
        (
            "src/gateway/execution_engine/run_loop/inner.rs",
            "who-owns: directory-catalogue registration for this run's owner (room turns short-circuit above it)",
        ),
    ];

    /// Walk `src/` and return `(repo-relative path, contents)` for every `.rs`
    /// file, mirroring `utils::paths`'s source-level guard.
    fn all_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(files.len() > 100, "walk found suspiciously few sources");
        files
            .into_iter()
            .filter_map(|file| {
                let rel = file
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .replace('\\', "/");
                std::fs::read_to_string(&file).ok().map(|text| (rel, text))
            })
            .collect()
    }

    /// Source-level, because the distinction is not visible at runtime: a
    /// predicate handed the room's creator answers a perfectly well-formed
    /// question about the wrong person, and returns `true`/`false` exactly as
    /// confidently as the right one would.
    #[test]
    fn every_ambient_owner_call_site_is_in_the_census() {
        let mut offenders: Vec<String> = Vec::new();
        for (rel, text) in all_sources() {
            if AMBIENT_OWNER_CENSUS.iter().any(|(f, _)| *f == rel) {
                continue;
            }
            let sites: Vec<String> = text
                .lines()
                .enumerate()
                .filter(|(_, line)| {
                    let code = line.trim_start();
                    !code.starts_with("//") && !code.starts_with('*')
                })
                .filter(|(_, line)| line.contains("ambient_owner()"))
                .map(|(i, line)| format!("{rel}:{}: {}", i + 1, line.trim()))
                .collect();
            if !sites.is_empty() {
                offenders.push(sites.join("\n    "));
            }
        }

        assert!(
            offenders.is_empty(),
            "these resolve an actor through `scope::ambient_owner()` without being in \
             AMBIENT_OWNER_CENSUS. If the question is \"who is asking\" (any visibility \
             predicate), the resolver must be `visibility::ambient_actor()` — inside a \
             project room `ambient_owner()` names the room's CREATOR for every member. \
             If the question is \"who owns this row\", add the file to the census with \
             that reason:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// The census is only worth having if it cannot rot into a list of files
    /// that no longer mention the thing.
    #[test]
    fn the_census_names_no_file_that_stopped_using_it() {
        let sources: std::collections::HashMap<String, String> = all_sources().into_iter().collect();
        for (file, reason) in AMBIENT_OWNER_CENSUS {
            let text = sources
                .get(*file)
                .unwrap_or_else(|| panic!("census names a file that does not exist: {file}"));
            assert!(
                text.contains("ambient_owner()"),
                "census entry is stale — {file} no longer calls ambient_owner() ({reason})"
            );
        }
    }

    /// The behavioural half of the fix: outside a room the two resolvers are
    /// the same answer, so switching a predicate from one to the other is a
    /// no-op for every personal / org / cron caller.
    #[tokio::test]
    async fn ambient_actor_equals_ambient_owner_outside_a_room() {
        let personal = crate::scope::ScopeAttribution::personal("u-alice");
        let same = crate::scope::with_scope(Some(personal), async {
            (crate::scope::ambient_owner(), ambient_actor())
        })
        .await;
        assert_eq!(same.0, same.1, "outside a room the resolvers must agree");
        assert_eq!(same.1.as_deref(), Some("u-alice"));

        // And with no scope at all, both are the unrestricted `None`.
        assert_eq!(crate::scope::ambient_owner(), None);
        assert_eq!(ambient_actor(), None);
    }
}
