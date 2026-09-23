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
//! (`execution_engine::run_loop::tests::every_run_producer_answers_the_fire_time_authority_question`)
//! checks for it per file.

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
}
