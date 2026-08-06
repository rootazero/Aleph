//! Scope vocabulary and ambient attribution for data isolation.
//!
//! This module implements the spec §5.1 vocabulary for scope taxonomy:
//! - `Org`: organization-wide scope
//! - `Personal(String)`: personal scope for a user (ref format: `u-<uuid>` from users.rs:162)
//! - `Project(String)`: project scope (ref format: `p-…` reserved for P2)
//!
//! The `Personal` and `Project` refs are the P0 id formats verbatim, so
//! `partition_suffix` composes directly with `project_scope::scoped_agent_id`.
//! The three suffix families — `proj-*` (legacy directory feature), `u-*` (personal),
//! `p-*` (project) — are siblings, never nested.
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

use std::collections::HashMap;

pub mod directory;

/// A scope identifier representing the visibility boundary for an agent or resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeId {
    /// Organization-wide scope; visible to all members.
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
            ScopeId::Org => "org".to_string(),
            ScopeId::Personal(ref_id) => format!("personal:{}", ref_id),
            ScopeId::Project(ref_id) => format!("project:{}", ref_id),
        }
    }

    /// Parse a scope from its rendered form. Returns `None` if the input is
    /// invalid or refers to an unknown scope kind (fail-closed).
    pub fn parse(s: &str) -> Option<ScopeId> {
        match s {
            "org" => Some(ScopeId::Org),
            _ => {
                if let Some((kind, ref_id)) = s.split_once(':') {
                    match kind {
                        "personal" if !ref_id.is_empty() => {
                            Some(ScopeId::Personal(ref_id.to_string()))
                        }
                        "project" if !ref_id.is_empty() => {
                            Some(ScopeId::Project(ref_id.to_string()))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
        }
    }

    /// Extract the partition suffix (the ref) from this scope.
    /// - `Org` → `None`
    /// - `Personal(u)` → `Some(u)`
    /// - `Project(p)` → `Some(p)`
    pub fn partition_suffix(&self) -> Option<&str> {
        match self {
            ScopeId::Org => None,
            ScopeId::Personal(ref_id) => Some(ref_id),
            ScopeId::Project(ref_id) => Some(ref_id),
        }
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
/// The author is the *speaker*, which is why this reads [`ambient_owner`] and
/// not `scope.owner_user_id`: since P2 those two are different facts. Every run
/// in a room carries the ROOM's attribution — that is precisely what makes the
/// members share one memory partition — so the scope's owner tells you whose
/// turn it is only by accident. `owner_user_id` is the fallback for the paths
/// that have a scope but no live caller identity (a channel-driven run).
#[must_use]
pub fn room_author(scope: Option<&ScopeAttribution>) -> Option<String> {
    let attr = scope?;
    if !matches!(attr.scope, ScopeId::Project(_)) {
        return None;
    }
    Some(ambient_owner().unwrap_or_else(|| attr.owner_user_id.clone()))
}

/// Ambient-shaped twin of [`room_author`], for emission sites that run inside
/// the run's task-local scope (everything under
/// `gateway::execution_engine::run_loop::with_request_scope`, which is where
/// `harness_bridge::session_seed` writes the main path's user message).
///
/// ⚠️ **A caller outside that nest reads `None` forever and the label silently
/// never appears.** `fast_path` and `SimpleExecutionEngine` are separate
/// engines that never enter it; they hold the same fact in
/// `request.metadata` and must go through [`room_author`] with
/// [`scope_from_metadata`] instead. This is the same two-shapes split as
/// `memory::project_scope::{profile_floor_id, partition_is_shared_room}`: one
/// question, two call sites, two different things in hand.
#[must_use]
pub fn ambient_room_author() -> Option<String> {
    room_author(current_scope().as_ref())
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

    #[test]
    fn partition_suffix_is_the_ref_verbatim() {
        assert_eq!(ScopeId::Org.partition_suffix(), None);
        assert_eq!(
            ScopeId::Personal("u-alice".into()).partition_suffix(),
            Some("u-alice")
        );
        assert_eq!(
            ScopeId::Project("p-x7f2".into()).partition_suffix(),
            Some("p-x7f2")
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
            room_author(Some(&attr("u-alice", ScopeId::Project("p-room".into())))),
            Some("u-alice".to_string())
        );
        assert_eq!(
            room_author(Some(&attr("u-alice", ScopeId::Personal("u-alice".into())))),
            None
        );
        assert_eq!(room_author(Some(&attr("u-alice", ScopeId::Org))), None);
        assert_eq!(room_author(None), None);
    }

    #[tokio::test]
    async fn the_author_is_the_speaker_not_the_scopes_owner() {
        // Since P2 those are different facts: every run in a room carries the
        // ROOM's attribution (that is what shares the memory partition), so
        // reading `owner_user_id` when a caller identity is live would label
        // Bob's message with whoever the run was seeded for.
        use crate::gateway::caller_identity::CALLER_USER;
        let stamped = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                room_author(Some(&attr("u-alice", ScopeId::Project("p-room".into()))))
            })
            .await;
        assert_eq!(stamped, Some("u-bob".to_string()));
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
        assert_eq!(inside, Some("u-alice".to_string()));
    }

    #[test]
    fn from_persisted_fails_closed_on_absence_or_garbage() {
        // Legacy/pre-P1 rows: both columns NULL.
        assert!(ScopeAttribution::from_persisted(None, None).is_none());
        // Unparseable scope_id: fail closed, never guess.
        assert!(ScopeAttribution::from_persisted(Some("u-alice"), Some("garbage")).is_none());
    }
}
