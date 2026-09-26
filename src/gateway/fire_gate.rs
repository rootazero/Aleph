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
//! added by T09) checks for it per file. The one resolve that lives HERE is
//! [`session_may_act`], a pre-check that returns no grant — see its doc for
//! why it must not sit in the producer's own file.

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
///
/// `Legacy` stamps nothing here, while cron and heartbeat run `Legacy` under
/// `Granted::legacy` (which stamps the persisted pair). The two agree in
/// production: boot always installs the users store, so `Legacy` there only
/// means "nobody to check" — no owner and no author — and `Granted::legacy`
/// of such a subject has no pair and no author to stamp either. They differ
/// only where no store is installed (tests, minimal servers), where an owned
/// row resolves `Legacy` and cron/heartbeat still stamp its pair.
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
/// A refusal names the person that was checked and in which capacity
/// (e.g. `principal deactivated — author u-bob`), from
/// [`crate::scope::authority::checked_person`], so a tombstone, a resume
/// refusal or an announce log line says WHO may no longer act, not only
/// whether they were deactivated or are gone.
///
/// The resolver is a parameter so the whole mapping — including WHICH grant
/// lands on `metadata` — is driven by tests with `resolve_with` (no lib test
/// may install the process-global users store). Production passes
/// `|subject| crate::scope::authority::resolve(&subject)`, spelled at each
/// executor's own site because the `RunRequest` producer census greps it there.
///
/// `multi_user` is the server's mode for the room floor
/// ([`row_is_project_room`]), injected for the same reason: production passes
/// `crate::gateway::security::store::slot::multi_user`, tests pass the mode
/// they mean. It is called only for a room row with no known initiator whose
/// run proceeds, so every other fire pays no users read.
#[must_use]
pub(crate) fn authorize_session_run<R>(
    resolve: R,
    multi_user: impl FnOnce() -> bool,
    row: Option<&crate::gateway::session_store::types::SessionMetadata>,
    metadata: &mut HashMap<String, String>,
) -> FireVerdict
where
    R: FnOnce(FireSubject<'_>) -> FireAuthority,
{
    // An empty author is absent, exactly as the resolver reads it.
    let initiator_unknown_in_room = row_is_project_room(row)
        && metadata
            .get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
            .is_none_or(String::is_empty)
        && !metadata.contains_key("caller_role");
    let subject = subject_for_session_row(row, metadata);
    let checked = crate::scope::authority::checked_person(&subject).map(|(id, is_author)| {
        let capacity = if is_author { "author" } else { "session owner" };
        format!("{capacity} `{id}`")
    });
    let authority = resolve(subject);
    match apply(authority, metadata) {
        FireVerdict::Proceed => {
            if initiator_unknown_in_room && multi_user() {
                // Role only goes DOWN: absent reads as operator, and a
                // ceiling the grant already stamped is kept.
                metadata
                    .entry("caller_role".to_string())
                    .or_insert_with(|| {
                        crate::gateway::security::store::UserRole::Member
                            .wire_role()
                            .to_string()
                    });
            }
            FireVerdict::Proceed
        }
        FireVerdict::Refused(reason) => FireVerdict::Refused(match checked {
            Some(who) => format!("{reason} — {who}"),
            None => reason,
        }),
        other => other,
    }
}

/// The two stopping arms of [`FireVerdict`]: what a session-row admission
/// returns instead of the metadata to run with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FireStop {
    /// Settled (deactivated / deleted): stop this work and say why.
    Refused(String),
    /// The users store could not answer (R-a): skip this fire only.
    Unknown(String),
}

/// [`authorize_session_run`] for an executor that BUILDS its run from the
/// result: `Ok` is `base` as admitted — stamped with the resolved grant and
/// the room floor — and the only metadata the caller has to build from.
/// Taking `base` by value and handing it back is the point (ruling b): a
/// stamp that landed on any other map would be a stamp the run never sees.
///
/// Each session-row executor wraps this in its own `admit_*` that builds the
/// exact `RunRequest` it executes (`busy_queue::durable::admit_reinjection`,
/// `announce_delivery::admit_announce`, `resume_coordinator::admit_resume`).
pub(crate) fn admit_session_metadata<R>(
    resolve: R,
    multi_user: impl FnOnce() -> bool,
    row: Option<&crate::gateway::session_store::types::SessionMetadata>,
    mut base: HashMap<String, String>,
) -> Result<HashMap<String, String>, FireStop>
where
    R: FnOnce(FireSubject<'_>) -> FireAuthority,
{
    match authorize_session_run(resolve, multi_user, row, &mut base) {
        FireVerdict::Proceed => Ok(base),
        FireVerdict::Refused(reason) => Err(FireStop::Refused(reason)),
        FireVerdict::Unknown(reason) => Err(FireStop::Unknown(reason)),
    }
}

/// Whether `row` is a project ROOM's session. In a room the row's owner is the
/// room's CREATOR, identical for every member, so a run rebuilt from the row
/// with no author and no carried role has an unknown initiator: judged as the
/// creator, an admin creator would run a member's work as operator.
/// [`authorize_session_run`] caps such a run at `member` — role only down —
/// on a MULTI-USER server only. With no person but the machine owner (or no
/// users table at all) there is no member whose work the creator's grant
/// could carry, so a single-user install keeps its grant unchanged. The mode
/// is read at fire time, so the floor applies from the first fire after a
/// second person is added; a failed read or a degraded store counts as
/// multi-user (`slot::multi_user_in`).
///
/// Trade-off, recorded: on a multi-user server an admin's own announcements
/// and resumes in their own room are capped at member too, until the
/// initiator is carried on these paths (announce / resume initiator carry —
/// a ledgered follow-up).
fn row_is_project_room(
    row: Option<&crate::gateway::session_store::types::SessionMetadata>,
) -> bool {
    row.and_then(|r| r.scope_id.as_deref())
        .and_then(crate::scope::ScopeId::parse)
        .is_some_and(|scope| matches!(scope, crate::scope::ScopeId::Project(_)))
}

/// Whether the person behind a session row may act at all — the question a
/// boot or on-demand resume asks BEFORE it repairs the log or spends a
/// crash-loop attempt (ruling a, §15). `Refused` names the person checked,
/// exactly as [`authorize_session_run`] does, because it IS that call.
///
/// Returns a verdict and NO grant, deliberately. The grant a resumed run
/// executes under is resolved and applied once, in
/// `ResumeCoordinator::retrigger`, against the run's FINAL metadata: the
/// carried `caller_role` that `stamp_origin_identity` restores for a channel
/// origin decides the ceiling, and a grant computed here, without it, would
/// stamp `member` over a `guest` — raising it. (`retrigger` resolves; its
/// `admit_resume` applies and builds the request.)
///
/// Lives here rather than in `resume_coordinator.rs` so that file's
/// `authority::resolve(` and `authorize_session_run(` are `retrigger`'s and
/// `admit_resume`'s alone: the producer census reads those tokens per file,
/// and a second spelling beside the one that APPLIES the grant would keep the
/// census green with that call deleted (T09 re-review, N1).
///
/// The room floor stamps only the metadata, which this pre-check discards, so
/// it is handed a constant mode rather than a users read that could change
/// nothing.
#[must_use]
pub(crate) fn session_may_act(
    row: Option<&crate::gateway::session_store::types::SessionMetadata>,
) -> FireVerdict {
    authorize_session_run(
        |subject| crate::scope::authority::resolve(&subject),
        || false,
        row,
        &mut HashMap::new(),
    )
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
        assert!(matches!(
            apply(authority, &mut meta),
            FireVerdict::Refused(_)
        ));
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
            || true,
            Some(&row),
            &mut meta,
        );
        assert_eq!(verdict, FireVerdict::Proceed);
        assert_eq!(asked.as_deref(), Some("u-bob"));
        assert_eq!(meta.get("caller_role").map(String::as_str), Some("member"));
        let scope = crate::scope::scope_from_metadata(&meta).expect("the grant stamps the pair");
        assert_eq!(scope.owner_user_id, "u-bob");
    }

    /// Final review I1: a room row rebuilt with no author and no carried role
    /// (announce delivery, boot resume) would be judged as the room's CREATOR,
    /// and an admin creator's grant stamps no ceiling — operator. On a
    /// multi-user server (the mode read from the same table) the floor caps
    /// it at `member`.
    #[test]
    fn a_room_run_with_no_known_initiator_is_capped_at_member() {
        let store = users();
        let room = room_row();
        let mut meta = HashMap::new();
        let verdict = authorize_session_run(
            |s| resolve_with(Some(&store), &s),
            || crate::gateway::security::store::slot::multi_user_with(Some(&store)),
            Some(&room),
            &mut meta,
        );
        assert_eq!(verdict, FireVerdict::Proceed);
        assert_eq!(meta.get("caller_role").map(String::as_str), Some("member"));
    }

    /// Re-review c4: the floor is a multi-user rule. On a single-user install
    /// (only the machine owner has a row) the owner's own room run keeps the
    /// grant it resolved — byte-identical to the pre-floor behaviour — and a
    /// fire that is not a speakerless room run never asks for the mode.
    #[test]
    fn a_single_user_room_run_keeps_its_grant_and_other_fires_read_no_mode() {
        use crate::gateway::security::store::OWNER_USER_ID;
        let store = SecurityStore::in_memory().unwrap();
        store.ensure_bootstrap_owner().unwrap();
        let room = crate::gateway::session_store::types::SessionMetadata {
            owner_user_id: Some(OWNER_USER_ID.into()),
            scope_id: Some(crate::scope::ScopeId::Project("p-room".into()).render()),
            ..Default::default()
        };
        let asked = std::cell::Cell::new(0u32);
        let mode = || {
            asked.set(asked.get() + 1);
            crate::gateway::security::store::slot::multi_user_with(Some(&store))
        };
        let mut meta = HashMap::new();
        assert_eq!(
            authorize_session_run(
                |s| resolve_with(Some(&store), &s),
                mode,
                Some(&room),
                &mut meta
            ),
            FireVerdict::Proceed
        );
        assert_eq!(asked.get(), 1, "a speakerless room run asks for the mode");
        assert_eq!(
            meta.get("caller_role"),
            None,
            "single-user: the owner's room run keeps its uncapped grant"
        );

        let personal = crate::gateway::session_store::types::SessionMetadata {
            owner_user_id: Some(OWNER_USER_ID.into()),
            scope_id: Some(format!("personal:{OWNER_USER_ID}")),
            ..Default::default()
        };
        let mut authored = HashMap::new();
        authored.insert(
            crate::gateway::execution_engine::AUTHOR_USER_KEY.to_string(),
            OWNER_USER_ID.to_string(),
        );
        for (row, meta) in [(&personal, HashMap::new()), (&room, authored)] {
            let mut meta = meta;
            let before = asked.get();
            let verdict = authorize_session_run(
                |s| resolve_with(Some(&store), &s),
                || {
                    asked.set(asked.get() + 1);
                    true
                },
                Some(row),
                &mut meta,
            );
            assert_eq!(verdict, FireVerdict::Proceed);
            assert_eq!(asked.get(), before, "no floor question, no users read");
        }
    }

    /// The floor only ever lowers: a carried `guest` stays `guest`, and a
    /// personal row (one human, the owner IS the initiator) is untouched.
    #[test]
    fn the_room_floor_never_raises_a_carry_and_leaves_personal_rows_alone() {
        let store = users();
        let mut guest = HashMap::new();
        guest.insert("caller_role".to_string(), "guest".to_string());
        assert_eq!(
            authorize_session_run(
                |s| resolve_with(Some(&store), &s),
                || true,
                Some(&room_row()),
                &mut guest
            ),
            FireVerdict::Proceed
        );
        assert_eq!(guest.get("caller_role").map(String::as_str), Some("guest"));

        let personal = crate::gateway::session_store::types::SessionMetadata {
            owner_user_id: Some("u-alice".into()),
            scope_id: Some("personal:u-alice".into()),
            ..Default::default()
        };
        let mut meta = HashMap::new();
        assert_eq!(
            authorize_session_run(
                |s| resolve_with(Some(&store), &s),
                || true,
                Some(&personal),
                &mut meta
            ),
            FireVerdict::Proceed
        );
        assert_eq!(
            meta.get("caller_role"),
            None,
            "an admin's own personal session keeps its uncapped grant"
        );
    }

    /// Alice's (admin) project room `p-room`.
    fn room_row() -> crate::gateway::session_store::types::SessionMetadata {
        crate::gateway::session_store::types::SessionMetadata {
            owner_user_id: Some("u-alice".into()),
            scope_id: Some(crate::scope::ScopeId::Project("p-room".into()).render()),
            ..Default::default()
        }
    }

    /// T09 fix round 1 (I1/M4): a refusal names WHO was checked and in which
    /// capacity — the resume settle sentence, the busy-queue tombstone and
    /// the announce log line all carry this text — and says deactivated vs
    /// gone.
    #[test]
    fn a_session_refusal_names_the_person_checked() {
        let store = users();
        store
            .update_user("u-bob", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        let bobs = crate::gateway::session_store::types::SessionMetadata {
            owner_user_id: Some("u-bob".into()),
            scope_id: Some("personal:u-bob".into()),
            ..Default::default()
        };
        let verdict = authorize_session_run(
            |s| resolve_with(Some(&store), &s),
            || true,
            Some(&bobs),
            &mut HashMap::new(),
        );
        assert_eq!(
            verdict,
            FireVerdict::Refused("principal deactivated — session owner `u-bob`".into())
        );

        let ghosts = crate::gateway::session_store::types::SessionMetadata {
            owner_user_id: Some("u-alice".into()),
            scope_id: Some(crate::scope::ScopeId::Project("p-room".into()).render()),
            ..Default::default()
        };
        let mut meta = HashMap::new();
        meta.insert(
            crate::gateway::execution_engine::AUTHOR_USER_KEY.to_string(),
            "u-ghost".to_string(),
        );
        let verdict = authorize_session_run(
            |s| resolve_with(Some(&store), &s),
            || true,
            Some(&ghosts),
            &mut meta,
        );
        assert_eq!(
            verdict,
            FireVerdict::Refused("principal gone — author `u-ghost`".into())
        );
    }
}
