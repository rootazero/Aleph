//! Scope vocabulary and ambient attribution for data isolation.
//!
//! This module implements the spec §5.1 vocabulary for scope taxonomy:
//! - `Personal(String)`: personal scope for a user (ref format: `u-<uuid>` from users.rs:162)
//! - `Project(String)`: project scope (ref format: `p-…`, P2 project rooms)
//! - `Org`: organization-wide scope — **vocabulary only, no producer.** No
//!   production site constructs this variant and nothing writes the string
//!   `"org"` into a `scope_id` stamp, so no live session, loop, cron job or
//!   memory partition is org-scoped. The only way to obtain one is
//!   [`ScopeId::parse`] reading a persisted column, i.e. a hand-edited row. It
//!   is kept on purpose — the 3-kind taxonomy is the spec's deliberate
//!   vocabulary and the whole cost is this file's own `render`/`parse` arms.
//!   Do not read its presence as a shipped feature, and do not delete it as
//!   dead code.
//!
//! The `Personal` and `Project` refs are the P0 id formats verbatim, so the
//! sites that compose partitions destructure the enum and hand the ref straight
//! to `project_scope::scoped_agent_id` (`session_write_id` / `session_read_ids`
//! / `profile_floor_id`). The three suffix families — `proj-*` (legacy directory
//! feature), `u-*` (personal), `p-*` (project) — are siblings, never nested.
//!
//! The task-local follows `src/projects/run_context.rs`'s contract verbatim:
//! children spawned via `tokio::spawn` MUST capture the attribution before the
//! spawn boundary (use [`current_scope`] at the call site, then feed the captured
//! `ScopeAttribution` back into the new request). Synchronous await chains inherit
//! the scope automatically.
//!
//! Metadata round-tripping via [`stamp_metadata`] and [`scope_from_metadata`]
//! requires BOTH keys to be present and coherent; if either is missing or invalid,
//! `scope_from_metadata` returns `None` (fail-closed, legacy compat).

use aleph_protocol::scope as wire;
use std::collections::HashMap;

pub mod carried;
pub mod directory;

pub use carried::CarriedAttribution;

/// A scope identifier representing the visibility boundary for an agent or resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeId {
    /// Organization-wide scope. Vocabulary only — no producer; see the module
    /// doc before treating a match on this arm as reachable.
    Org,
    /// Personal scope for a specific user. Ref is the user ID (e.g., `u-alice`).
    Personal(String),
    /// Project scope. Ref is the project ID (e.g., `p-x7f2`).
    Project(String),
}

impl ScopeId {
    /// Render the scope to its canonical string form.
    /// - `Org` → `"org"`
    /// - `Personal(u)` → `"personal:<u>"`
    /// - `Project(p)` → `"project:<p>"`
    pub fn render(&self) -> String {
        match self {
            ScopeId::Org => wire::ORG.to_string(),
            ScopeId::Personal(ref_id) => format!("{}{}", wire::PERSONAL_PREFIX, ref_id),
            ScopeId::Project(ref_id) => wire::project_scope_id(ref_id),
        }
    }

    /// Parse a scope from its rendered form. Returns `None` if the input is
    /// invalid or refers to an unknown scope kind (fail-closed).
    ///
    /// Both directions are spelled in terms of [`aleph_protocol::scope`], the
    /// shared home of this vocabulary, so a client that constructs one of
    /// these strings cannot be reading a prefix core no longer emits. Keeping
    /// the *typed* enum here is deliberate: a client may name a scope, it may
    /// not decide what one permits.
    pub fn parse(s: &str) -> Option<ScopeId> {
        if s == wire::ORG {
            return Some(ScopeId::Org);
        }
        if let Some(ref_id) = wire::project_id_of(s) {
            return Some(ScopeId::Project(ref_id.to_string()));
        }
        s.strip_prefix(wire::PERSONAL_PREFIX)
            .filter(|ref_id| !ref_id.is_empty())
            .map(|ref_id| ScopeId::Personal(ref_id.to_string()))
    }
}

/// Ambient scope attribution: combines the owning user ID and the scope boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeAttribution {
    /// The user ID of the owner (e.g., `u-alice`).
    pub owner_user_id: String,
    /// The scope boundary (org, personal, or project).
    pub scope: ScopeId,
}

impl ScopeAttribution {
    /// Create a personal scope attribution for the given user.
    pub fn personal(user_id: &str) -> Self {
        ScopeAttribution {
            owner_user_id: user_id.to_string(),
            scope: ScopeId::Personal(user_id.to_string()),
        }
    }

    /// Reconstruct a `ScopeAttribution` from a session row's persisted
    /// `owner_user_id`/`scope_id` columns (`gateway::session_store::types::
    /// SessionMetadata`).
    ///
    /// Used on paths where the ambient task-local ([`current_scope`]) is not
    /// reliably live — e.g. session-close, which can run outside the run's
    /// task tree (a background sweep, a `sessions.delete` RPC, a spawned
    /// task with no re-threaded scope) — so those callers derive scope from
    /// the durable row instead of guessing at the task-local. Same
    /// fail-closed contract as [`scope_from_metadata`]: requires both
    /// fields present and a parseable, coherent scope, else `None`.
    #[must_use]
    pub fn from_persisted(owner_user_id: Option<&str>, scope_id: Option<&str>) -> Option<Self> {
        let owner_user_id = owner_user_id?.to_string();
        let scope = ScopeId::parse(scope_id?)?;
        Some(ScopeAttribution {
            owner_user_id,
            scope,
        })
    }
}

/// Metadata key for the owning user ID.
pub const OWNER_META_KEY: &str = "scope_owner_user_id";

/// Metadata key for the scope ID (rendered form).
pub const SCOPE_META_KEY: &str = "scope_id";

tokio::task_local! {
    static CURRENT_ATTRIBUTION: Option<ScopeAttribution>;
    static CURRENT_ROOM_AUTHOR: Option<String>;
}

/// Run `fut` with the given scope attribution visible to [`current_scope`] for
/// the lifetime of the future. The scope is bounded by the future, so once it
/// resolves the task-local goes back to whatever the parent stack had.
pub async fn with_scope<F, T>(attr: Option<ScopeAttribution>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_ATTRIBUTION.scope(attr, fut).await
}

/// Read the active scope attribution, if any. Returns `None` outside a
/// [`with_scope`] scope or when the surrounding scope explicitly set `None`.
#[must_use]
pub fn current_scope() -> Option<ScopeAttribution> {
    CURRENT_ATTRIBUTION
        .try_with(|attr| attr.clone())
        .ok()
        .flatten()
}

/// The user the current execution context acts for, across BOTH ambient
/// mechanisms — or `None` for an unrestricted internal caller.
///
/// Two task-locals carry attribution and neither one covers both surfaces:
///
/// - `CALLER_USER` ([`crate::gateway::caller_identity`]) is scoped around
///   every gateway dispatch, so it is live for an RPC handler and dead
///   everywhere else.
/// - [`current_scope`] is re-seeded across every `tokio::spawn` boundary a run
///   crosses, so it is live inside an agent run's tools and dead for a bare
///   RPC that never starts a run.
///
/// A predicate that reads only the first is fail-open for every tool call; one
/// that reads only the second is fail-open for every RPC. Reading the gateway
/// identity first matters when both are live: it is the resolved *connection*
/// identity, checked at `connect` against the device binding, while the scope
/// is whatever the run was seeded with.
///
/// `None` means "no ambient owner" and is deliberately unrestricted — cron,
/// background sweeps, A2A and in-process tests behave exactly as they did
/// before P1 (zero-change guarantee), matching
/// [`crate::gateway::visibility::visible_owner_filter`].
#[must_use]
pub fn ambient_owner() -> Option<String> {
    crate::gateway::caller_identity::current_caller_user()
        .or_else(|| current_scope().map(|attr| attr.owner_user_id))
}

/// Who to stamp as the author of a `SessionEvent::UserMessage` written under
/// `scope` — or `None` when the message cannot have a second possible author.
///
/// Single source for spec §6.2's "only project rooms are labelled". A personal
/// or org session has exactly one human in it, so an author stamp there is
/// noise that buys nothing and costs prompt bytes on every replayed message.
///
/// `author` is this turn's speaker, carried on the request as
/// [`crate::gateway::execution_engine::AUTHOR_USER_KEY`] and stamped by
/// `handlers::agent::build_run_request` from the authenticated caller. It is
/// passed explicitly because neither ambient mechanism can answer the question
/// at an emission site: every emission runs inside a spawned run, where
/// `CALLER_USER` is dead, and `scope.owner_user_id` names the ROOM's owner —
/// that is precisely what makes the members share one memory partition, so
/// reading it labels every member's message with whoever created the session.
///
/// `attr.owner_user_id` remains the fallback for a turn that carries no author
/// at all: a legacy row, or a channel-driven run whose inbound router stamps
/// the scope but not the speaker.
#[must_use]
pub fn room_author(scope: Option<&ScopeAttribution>, author: Option<&str>) -> Option<String> {
    let attr = scope?;
    if !matches!(attr.scope, ScopeId::Project(_)) {
        return None;
    }
    Some(author.map_or_else(|| attr.owner_user_id.clone(), str::to_string))
}

/// [`room_author`] for a caller holding the request's metadata map — the shape
/// `fast_path`, `SimpleExecutionEngine` and the steering writer need, none of
/// which enters the run's task-local nest.
///
/// Reads BOTH facts out of the one map, so a fifth emission site cannot pick up
/// the scope and forget the speaker — which is exactly how the label came to
/// name the session's creator on every turn.
#[must_use]
pub fn room_author_from_metadata(meta: &HashMap<String, String>) -> Option<String> {
    room_author(
        scope_from_metadata(meta).as_ref(),
        meta.get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
            .map(String::as_str),
    )
}

/// Run `fut` with `author` visible to [`ambient_room_author`].
///
/// **Seeded at exactly the two places [`with_scope`] is, and it must stay that
/// way.** The author and the scope answer two halves of one question — who is
/// speaking, and which room they are speaking in — so a boundary that carries
/// one without the other produces a confidently wrong label rather than a
/// missing one:
///
/// 1. `run_loop::with_request_scope`, from the request's `AUTHOR_USER_KEY`.
/// 2. `orchestrator::dispatch`, re-seeded inside its `tokio::spawn` from a
///    value captured on the caller's side — task-locals do not cross a spawn,
///    and the main path's user-message writer
///    (`harness_bridge::session_seed`) lives on the far side of that one.
///
/// A new spawn boundary between a seeding point and an emission site owes the
/// same capture-and-re-seed pair. Getting it wrong is silent: see
/// [`room_author`] for what the fallback then reports.
pub async fn with_room_author<F, T>(author: Option<String>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_ROOM_AUTHOR.scope(author, fut).await
}

/// This turn's speaker, if one was seeded by [`with_room_author`].
#[must_use]
pub fn current_room_author() -> Option<String> {
    CURRENT_ROOM_AUTHOR.try_with(Clone::clone).ok().flatten()
}

/// Ambient-shaped twin of [`room_author`], for emission sites that run inside
/// the run's task-local nest (everything under
/// `gateway::execution_engine::run_loop::with_request_scope`, which is where
/// `harness_bridge::session_seed` writes the main path's user message).
///
/// ⚠️ **A caller outside that nest reads `None` forever and the label silently
/// never appears.** `fast_path` and `SimpleExecutionEngine` are separate
/// engines that never enter it; they hold both facts in `request.metadata` and
/// must go through [`room_author`] with [`scope_from_metadata`] instead. This
/// is the same two-shapes split as
/// `memory::project_scope::{profile_floor_id, partition_is_shared_room}`: one
/// question, two call sites, two different things in hand.
#[must_use]
pub fn ambient_room_author() -> Option<String> {
    room_author(current_scope().as_ref(), current_room_author().as_deref())
}

/// Reconstruct a `ScopeAttribution` from metadata.
/// Requires BOTH `OWNER_META_KEY` and `SCOPE_META_KEY` to be present and
/// coherent; returns `None` if either is missing or the scope fails to parse
/// (fail-closed, for legacy compat).
pub fn scope_from_metadata(meta: &HashMap<String, String>) -> Option<ScopeAttribution> {
    let owner_user_id = meta.get(OWNER_META_KEY)?.clone();
    let scope_str = meta.get(SCOPE_META_KEY)?;
    let scope = ScopeId::parse(scope_str)?;
    Some(ScopeAttribution {
        owner_user_id,
        scope,
    })
}

/// Write a `ScopeAttribution` into metadata as key-value pairs.
pub fn stamp_metadata(meta: &mut HashMap<String, String>, attr: &ScopeAttribution) {
    meta.insert(OWNER_META_KEY.to_string(), attr.owner_user_id.clone());
    meta.insert(SCOPE_META_KEY.to_string(), attr.scope.render());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_parse_round_trips_all_three_kinds() {
        for s in [
            ScopeId::Org,
            ScopeId::Personal("u-alice".into()),
            ScopeId::Project("p-x7f2".into()),
        ] {
            assert_eq!(ScopeId::parse(&s.render()), Some(s.clone()));
        }
        assert_eq!(ScopeId::parse("personal:"), None, "empty ref is invalid");
        assert_eq!(
            ScopeId::parse("group:x"),
            None,
            "unknown kind is invalid — fail closed"
        );
    }

    #[tokio::test]
    async fn task_local_scopes_and_does_not_cross_spawn() {
        // mirror src/projects/run_context.rs::task_local_does_not_cross_spawn_boundary
        let attr = ScopeAttribution::personal("u-alice");
        with_scope(Some(attr), async {
            assert_eq!(current_scope().unwrap().owner_user_id, "u-alice");
            let handle = tokio::spawn(async { current_scope() });
            assert!(
                handle.await.unwrap().is_none(),
                "task-locals must not cross spawn"
            );
        })
        .await;
        assert!(current_scope().is_none(), "scope pops on future completion");
    }

    #[test]
    fn metadata_round_trip() {
        let mut m = HashMap::new();
        stamp_metadata(&mut m, &ScopeAttribution::personal("u-alice"));
        let back = scope_from_metadata(&m).unwrap();
        assert_eq!(back.owner_user_id, "u-alice");
        assert_eq!(back.scope, ScopeId::Personal("u-alice".into()));
        assert!(
            scope_from_metadata(&HashMap::new()).is_none(),
            "absent keys → None (legacy)"
        );
        // Corrupt scope_id with a present owner: fail closed to None, never guess.
        let mut bad = HashMap::new();
        bad.insert(OWNER_META_KEY.to_string(), "u-alice".into());
        bad.insert(SCOPE_META_KEY.to_string(), "garbage".into());
        assert!(scope_from_metadata(&bad).is_none());
    }

    #[test]
    fn from_persisted_reconstructs_personal_scope() {
        let attr = ScopeAttribution::from_persisted(Some("u-alice"), Some("personal:u-alice"))
            .expect("both columns present and coherent");
        assert_eq!(attr.owner_user_id, "u-alice");
        assert_eq!(attr.scope, ScopeId::Personal("u-alice".into()));
    }

    #[test]
    fn from_persisted_reconstructs_org_scope_too() {
        // Not personal-only: any coherent scope round-trips (mirrors
        // `scope_from_metadata`). Callers that only care about personal
        // scope filter on `ScopeId::Personal` themselves — this helper's
        // job is fidelity to the persisted row, not policy.
        let attr = ScopeAttribution::from_persisted(Some("u-alice"), Some("org"))
            .expect("org is a coherent scope");
        assert_eq!(attr.scope, ScopeId::Org);
    }

    fn attr(owner: &str, scope: ScopeId) -> ScopeAttribution {
        ScopeAttribution {
            owner_user_id: owner.to_string(),
            scope,
        }
    }

    #[test]
    fn only_a_project_room_stamps_an_author() {
        // The predicate is "can this session have a second speaker", NOT "is
        // there a scope" — the latter is structurally true for every P1 session
        // and would put a redundant label on every personal message, on every
        // turn, forever.
        assert_eq!(
            room_author(
                Some(&attr("u-alice", ScopeId::Project("p-room".into()))),
                None
            ),
            Some("u-alice".to_string())
        );
        assert_eq!(
            room_author(
                Some(&attr("u-alice", ScopeId::Personal("u-alice".into()))),
                Some("u-alice")
            ),
            None
        );
        assert_eq!(
            room_author(Some(&attr("u-alice", ScopeId::Org)), Some("u-alice")),
            None
        );
        assert_eq!(room_author(None, Some("u-alice")), None);
    }

    /// The label names the SPEAKER. Every run in a room carries the room's
    /// attribution (that is what shares the memory partition), so deriving the
    /// author from the scope labels every member's message with the session's
    /// creator — the exact defect this signature exists to make impossible.
    #[test]
    fn the_author_is_the_speaker_not_the_scopes_owner() {
        assert_eq!(
            room_author(
                Some(&attr("u-alice", ScopeId::Project("p-room".into()))),
                Some("u-bob")
            ),
            Some("u-bob".to_string()),
            "bob typed it; alice merely created the room"
        );
    }

    /// A turn that names no author at all (a legacy row, or a channel-driven
    /// run whose router stamps the scope but not the speaker) still gets a
    /// label rather than none — just the room owner's.
    #[test]
    fn an_unstamped_turn_falls_back_to_the_rooms_owner() {
        assert_eq!(
            room_author(
                Some(&attr("u-alice", ScopeId::Project("p-room".into()))),
                None
            ),
            Some("u-alice".to_string())
        );
    }

    #[tokio::test]
    async fn the_ambient_twin_reads_none_outside_a_scope() {
        // Documented trap: `fast_path` and `SimpleExecutionEngine` never enter
        // the run's scope nest, so they must go through `room_author` with
        // `scope_from_metadata` instead. If this ever starts returning a value
        // outside a scope, that documented warning has gone stale.
        assert_eq!(ambient_room_author(), None);
        let inside = with_scope(
            Some(attr("u-alice", ScopeId::Project("p-room".into()))),
            async { ambient_room_author() },
        )
        .await;
        assert_eq!(
            inside,
            Some("u-alice".to_string()),
            "no author seeded → the room owner"
        );
    }

    /// The ambient twin must carry the SPEAKER, not the scope owner, all the
    /// way to an emission site on the far side of a spawn.
    ///
    /// Deliberately crosses a real `tokio::spawn` with the capture-and-re-seed
    /// pair `orchestrator::dispatch` uses, because the same assertion nested in
    /// ONE task passes against a build whose author dies at that boundary —
    /// which is precisely how the first fix round shipped a label naming the
    /// room's creator on every message. `session_seed`, the main path's
    /// user-message writer, sits past exactly such a spawn and has no metadata
    /// map to fall back on.
    #[tokio::test]
    async fn the_ambient_twin_carries_the_speaker_across_a_spawn() {
        let seen = with_scope(
            Some(attr("u-alice", ScopeId::Project("p-room".into()))),
            with_room_author(Some("u-bob".to_string()), async {
                let captured_scope = current_scope();
                let captured_author = current_room_author();
                tokio::spawn(with_scope(
                    captured_scope,
                    with_room_author(captured_author, async { ambient_room_author() }),
                ))
                .await
                .expect("emission task")
            }),
        )
        .await;
        assert_eq!(seen, Some("u-bob".to_string()));
    }

    /// The same nest with the author NOT re-seeded across the spawn — the
    /// exact bug this pair guards. It does not error; it silently reports the
    /// room's owner, which is why no test caught it the first time.
    #[tokio::test]
    async fn dropping_the_author_at_a_spawn_silently_reports_the_room_owner() {
        let seen = with_scope(
            Some(attr("u-alice", ScopeId::Project("p-room".into()))),
            with_room_author(Some("u-bob".to_string()), async {
                let captured_scope = current_scope();
                tokio::spawn(with_scope(captured_scope, async { ambient_room_author() }))
                    .await
                    .expect("emission task")
            }),
        )
        .await;
        assert_eq!(
            seen,
            Some("u-alice".to_string()),
            "carrying the scope without the author is not a missing label, it \
             is a confidently wrong one"
        );
    }

    /// The author task-local obeys the same spawn rule as the scope one, so a
    /// new background producer cannot silently inherit a stale speaker.
    #[tokio::test]
    async fn the_author_task_local_does_not_cross_spawn() {
        with_room_author(Some("u-bob".to_string()), async {
            assert_eq!(current_room_author().as_deref(), Some("u-bob"));
            let handle = tokio::spawn(async { current_room_author() });
            assert!(handle.await.unwrap().is_none());
        })
        .await;
    }

    #[test]
    fn from_persisted_fails_closed_on_absence_or_garbage() {
        // Legacy/pre-P1 rows: both columns NULL.
        assert!(ScopeAttribution::from_persisted(None, None).is_none());
        // Unparseable scope_id: fail closed, never guess.
        assert!(ScopeAttribution::from_persisted(Some("u-alice"), Some("garbage")).is_none());
    }
}

#[cfg(test)]
mod wire_reconciliation {
    use super::*;

    /// The typed enum here and the string vocabulary in `aleph_protocol::
    /// scope` are two halves of one contract living in two crates — the shape
    /// this repo has watched drift more than once, because neither half fails
    /// to compile when the other changes its spelling. A client filtering on
    /// a prefix core stopped emitting gets an empty list, and an empty list
    /// renders identically to "this room has nothing in it".
    ///
    /// So: assert both directions against the shared helpers rather than
    /// against literals. A literal here would just be a third copy.
    #[test]
    fn renders_and_parses_the_shared_wire_spelling() {
        let project = ScopeId::Project("p-x7f2".to_string());
        assert_eq!(project.render(), wire::project_scope_id("p-x7f2"));
        assert_eq!(
            ScopeId::parse(&wire::project_scope_id("p-x7f2")),
            Some(project)
        );

        assert_eq!(ScopeId::Org.render(), wire::ORG);
        assert_eq!(ScopeId::parse(wire::ORG), Some(ScopeId::Org));

        let personal = ScopeId::Personal("u-alice".to_string());
        assert!(personal.render().starts_with(wire::PERSONAL_PREFIX));
        assert_eq!(ScopeId::parse(&personal.render()), Some(personal));
    }

    /// `belongs_to_project` is the predicate every client will reach for when
    /// filtering a list by room. It has to agree with what core actually
    /// stamps, so drive it from `render()` rather than from a hand-written
    /// string — including the unstamped case, whose fail-closed direction is
    /// the one a `==` at a call site gets wrong.
    #[test]
    fn the_client_membership_predicate_agrees_with_what_core_stamps() {
        let stamped = ScopeId::Project("p-a".to_string()).render();
        assert!(wire::belongs_to_project(Some(&stamped), "p-a"));
        assert!(!wire::belongs_to_project(Some(&stamped), "p-b"));

        let personal = ScopeId::Personal("u-alice".to_string()).render();
        assert!(!wire::belongs_to_project(Some(&personal), "p-a"));
        assert!(!wire::belongs_to_project(None, "p-a"));
    }

    /// Behaviour-preservation for the rewrite that moved the spelling out:
    /// the pre-existing parser kept everything after the FIRST colon, and
    /// rejected an empty ref and an unknown kind. All three still hold.
    #[test]
    fn parse_keeps_its_pre_existing_edge_behaviour() {
        assert_eq!(
            ScopeId::parse("project:p-a:b"),
            Some(ScopeId::Project("p-a:b".to_string()))
        );
        assert_eq!(ScopeId::parse("project:"), None);
        assert_eq!(ScopeId::parse("personal:"), None);
        assert_eq!(ScopeId::parse("team:t-1"), None);
        assert_eq!(ScopeId::parse("no-colon"), None);
    }
}
