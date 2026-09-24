//! The one way a gateway background executor turns a fire-time
//! [`FireAuthority`] into its run's metadata (spec §3.1–§3.2, round 11).
//!
//! Every executor that starts a run nobody is watching asks
//! `scope::authority::resolve` at the moment it fires, not when the work was
//! created. That answer has four shapes. The gateway executor families
//! (goal/loop continuations, boot resume, announce delivery, busy-queue
//! reinjection) need the same two things from it: the metadata stamped when
//! the answer is `Granted`, and a verdict that keeps "refused" apart from
//! "unknown". This module is that single mapping, so the executors cannot
//! each grow a slightly different one.
//!
//! The `resolve` call itself stays at each executor's own site on purpose: the
//! `RunRequest` producer census
//! (`execution_engine::run_loop::tests::every_run_producer_answers_the_fire_time_authority_question`,
//! added by T09) checks for it per file.

use std::collections::HashMap;

use crate::scope::authority::{FireAuthority, FireSubject};

/// What an executor does with one fire, after the authority answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FireVerdict {
    /// Execute. On `Granted` the metadata was stamped; on `Legacy` it is
    /// byte-identical to what the executor built.
    Proceed,
    /// A settled answer (deactivated / deleted): stop this work and say why.
    Refused(String),
    /// The users store could not answer (ruling R-a): skip THIS fire only and
    /// keep the work enabled. Never read as `Refused` (criterion §8).
    Unknown(String),
}

/// Map `authority` onto `metadata` and a verdict. `metadata` is changed only
/// on `Granted` (`Granted::stamp`: scope keys, author, and `caller_role` only
/// when it downgrades an operator carry).
#[must_use]
pub(crate) fn apply(
    authority: FireAuthority,
    metadata: &mut HashMap<String, String>,
) -> FireVerdict {
    let reason = authority.reason();
    match authority {
        FireAuthority::Legacy => FireVerdict::Proceed,
        FireAuthority::Granted(granted) => {
            granted.stamp(metadata);
            FireVerdict::Proceed
        }
        FireAuthority::Refused(_) => {
            FireVerdict::Refused(reason.unwrap_or_else(|| "authority refused".to_string()))
        }
        FireAuthority::Unknown(_) => {
            FireVerdict::Unknown(reason.unwrap_or_else(|| "authority unknown".to_string()))
        }
    }
}

/// The subject of a run whose owner, scope, author and role all ride its own
/// metadata: an autonomous continuation, whose map is
/// `execute::carry_policy_metadata` of the run that spawned it.
#[must_use]
pub(crate) fn subject_from_metadata(metadata: &HashMap<String, String>) -> FireSubject<'_> {
    FireSubject {
        owner: metadata
            .get(crate::scope::OWNER_META_KEY)
            .map(String::as_str),
        scope: metadata
            .get(crate::scope::SCOPE_META_KEY)
            .map(String::as_str),
        author: metadata
            .get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
            .map(String::as_str),
        carried_role: metadata.get("caller_role").map(String::as_str),
    }
}

/// The subject of a run rebuilt for an existing SESSION (boot resume, announce
/// delivery, busy-queue reinjection): owner/scope from the durable session
/// row, author and role from what the request carries. `row = None` (no row)
/// contributes no owner — Legacy unless an author rides the metadata.
#[must_use]
pub(crate) fn subject_for_session_row<'a>(
    row: Option<&'a crate::gateway::session_store::types::SessionMetadata>,
    metadata: &'a HashMap<String, String>,
) -> FireSubject<'a> {
    FireSubject {
        owner: row.and_then(|r| r.owner_user_id.as_deref()),
        scope: row.and_then(|r| r.scope_id.as_deref()),
        author: metadata
            .get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
            .map(String::as_str),
        carried_role: metadata.get("caller_role").map(String::as_str),
    }
}

/// Resolve and apply fire-time authority for a run rebuilt for an existing
/// session, in one step: [`subject_for_session_row`] → `resolve` → [`apply`].
///
/// The resolver is a parameter so the whole mapping — including WHICH grant
/// lands on `metadata` — is driven by tests with `resolve_with` (no lib test
/// may install the process-global users store). Production passes
/// `|subject| crate::scope::authority::resolve(&subject)`, spelled at each
/// executor's own site because the `RunRequest` producer census greps it there.
#[must_use]
pub(crate) fn authorize_session_run<R>(
    resolve: R,
    row: Option<&crate::gateway::session_store::types::SessionMetadata>,
    metadata: &mut HashMap<String, String>,
) -> FireVerdict
where
    R: FnOnce(FireSubject<'_>) -> FireAuthority,
{
    let authority = resolve(subject_for_session_row(row, metadata));
    apply(authority, metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::{SecurityStore, UserRole, UserStatus};
    use crate::scope::authority::{resolve_with, FireAuthority};

    fn users() -> SecurityStore {
        let store = SecurityStore::in_memory().expect("in-memory security store");
        store
            .create_user("u-alice", "Alice", UserRole::Admin)
            .unwrap();
        store.create_user("u-bob", "Bob", UserRole::Member).unwrap();
        store
    }

    fn room_policy(author: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        crate::scope::stamp_metadata(
            &mut m,
            &crate::scope::ScopeAttribution {
                owner_user_id: "u-alice".into(),
                scope: crate::scope::ScopeId::Project("p-room".into()),
            },
        );
        m.insert(
            crate::gateway::execution_engine::AUTHOR_USER_KEY.to_string(),
            author.to_string(),
        );
        m
    }

    /// R-b + N4: a continuation of Bob's turn in Alice's room acts for BOB,
    /// and Bob is a member — so the run that carried no role (read as
    /// operator) comes out stamped `member`.
    #[test]
    fn a_members_continuation_in_an_admins_room_is_stamped_member() {
        let store = users();
        let mut meta = room_policy("u-bob");
        let authority = resolve_with(Some(&store), &subject_from_metadata(&meta));
        assert_eq!(apply(authority, &mut meta), FireVerdict::Proceed);
        assert_eq!(meta.get("caller_role").map(String::as_str), Some("member"));
        assert_eq!(
            meta.get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
                .map(String::as_str),
            Some("u-bob")
        );
    }

    /// R-d: the room creator being the admin buys the author nothing.
    #[test]
    fn a_deactivated_author_is_refused_even_when_the_room_owner_is_active() {
        let store = users();
        store
            .update_user("u-bob", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        let mut meta = room_policy("u-bob");
        let before = meta.clone();
        let authority = resolve_with(Some(&store), &subject_from_metadata(&meta));
        match apply(authority, &mut meta) {
            FireVerdict::Refused(reason) => assert!(!reason.is_empty()),
            other => panic!("a deactivated author must be refused, got {other:?}"),
        }
        assert_eq!(meta, before, "a refused fire must not stamp anything");
    }

    /// R-a: "I do not know" is its own verdict, never Proceed and never Refused.
    #[test]
    fn an_unknown_authority_is_neither_run_nor_refused() {
        let mut meta = room_policy("u-bob");
        let before = meta.clone();
        let verdict = apply(FireAuthority::Unknown("disk I/O error".into()), &mut meta);
        assert!(matches!(verdict, FireVerdict::Unknown(ref r) if r.contains("disk I/O error")));
        assert_eq!(meta, before);
    }

    /// The role only ever goes DOWN: a guest's continuation stays guest even
    /// though the person behind it is now a Member. Pins that the subject
    /// carries `caller_role` — read as absent, the resolver would take the
    /// carry for operator and stamp `member`, RAISING a guest.
    #[test]
    fn a_guest_carry_is_never_raised_to_the_authors_role() {
        let store = users();
        let mut meta = room_policy("u-bob");
        meta.insert("caller_role".to_string(), "guest".to_string());
        let authority = resolve_with(Some(&store), &subject_from_metadata(&meta));
        assert_eq!(apply(authority, &mut meta), FireVerdict::Proceed);
        assert_eq!(meta.get("caller_role").map(String::as_str), Some("guest"));
    }

    /// The owner fallback: a continuation with no author (a pre-round-11 goal,
    /// or a turn with no room author) is judged against the carried OWNER.
    #[test]
    fn with_no_author_the_deactivated_owner_is_refused() {
        let store = users();
        store
            .update_user("u-alice", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        let mut meta = room_policy("u-bob");
        meta.remove(crate::gateway::execution_engine::AUTHOR_USER_KEY);
        let authority = resolve_with(Some(&store), &subject_from_metadata(&meta));
        assert!(
            matches!(apply(authority, &mut meta), FireVerdict::Refused(_)),
            "the owner is the person checked when no author is carried"
        );
    }

    /// Legacy is byte-identical to HEAD: no key is added or removed.
    #[test]
    fn a_legacy_continuation_is_left_untouched() {
        let mut meta = HashMap::new();
        meta.insert("caller_role".to_string(), "guest".to_string());
        let before = meta.clone();
        let authority = resolve_with(None, &subject_from_metadata(&meta));
        assert_eq!(apply(authority, &mut meta), FireVerdict::Proceed);
        assert_eq!(meta, before);
    }

    /// N8/N10: a session row's owner is the person when no author rides the
    /// metadata; a member's session resumed/reinjected with no role (read as
    /// operator) is stamped `member`.
    #[test]
    fn a_members_session_row_resolves_to_member() {
        let store = users();
        let row = crate::gateway::session_store::types::SessionMetadata {
            owner_user_id: Some("u-bob".into()),
            scope_id: Some("personal:u-bob".into()),
            ..Default::default()
        };
        let mut meta = HashMap::new();
        let authority = resolve_with(Some(&store), &subject_for_session_row(Some(&row), &meta));
        assert_eq!(apply(authority, &mut meta), FireVerdict::Proceed);
        assert_eq!(meta.get("caller_role").map(String::as_str), Some("member"));
    }

    /// N10: the author frozen into a queued payload wins over the row owner.
    #[test]
    fn a_carried_author_outranks_the_session_row_owner() {
        let store = users();
        store
            .update_user("u-bob", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        let row = crate::gateway::session_store::types::SessionMetadata {
            owner_user_id: Some("u-alice".into()),
            scope_id: Some(crate::scope::ScopeId::Project("p-room".into()).render()),
            ..Default::default()
        };
        let mut meta = HashMap::new();
        meta.insert(
            crate::gateway::execution_engine::AUTHOR_USER_KEY.to_string(),
            "u-bob".to_string(),
        );
        let authority = resolve_with(Some(&store), &subject_for_session_row(Some(&row), &meta));
        assert!(matches!(apply(authority, &mut meta), FireVerdict::Refused(_)));
    }

    /// Ruling (b): the grant the executors' shared seam stamps is the one the
    /// resolver returned for THIS row — a member owner comes out `member`
    /// with the row's scope pair, and the resolver was asked about that row.
    #[test]
    fn a_session_run_is_stamped_with_the_resolved_grant() {
        let store = users();
        let row = crate::gateway::session_store::types::SessionMetadata {
            owner_user_id: Some("u-bob".into()),
            scope_id: Some("personal:u-bob".into()),
            ..Default::default()
        };
        let mut asked = None;
        let mut meta = HashMap::new();
        let verdict = authorize_session_run(
            |subject| {
                asked = subject.owner.map(str::to_string);
                resolve_with(Some(&store), &subject)
            },
            Some(&row),
            &mut meta,
        );
        assert_eq!(verdict, FireVerdict::Proceed);
        assert_eq!(asked.as_deref(), Some("u-bob"));
        assert_eq!(meta.get("caller_role").map(String::as_str), Some("member"));
        let scope = crate::scope::scope_from_metadata(&meta).expect("the grant stamps the pair");
        assert_eq!(scope.owner_user_id, "u-bob");
    }
}
