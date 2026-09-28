//! Fire-time authority: the one answer to "may this background work run now,
//! and as whom?".
//!
//! Every background executor (cron, heartbeat, goal/loop continuation, the
//! team dispatcher, boot resume, announce delivery, busy-queue re-injection)
//! used to decide this itself, at creation time or never — so a principal
//! deactivated or demoted AFTER scheduling kept running with the authority
//! they had when they scheduled. This module re-derives it at TRIGGER time
//! from the users table, once, for all of them (spec r11 §3.1; qm
//! `cron/authority.ts`).
//!
//! Rules (rulings R-a / R-b / R-d):
//! - The person checked is the AUTHOR (original initiator) when one is
//!   carried, else the row's OWNER. A room's creator being walled does not
//!   stop a member's work in it; the member being walled does.
//! - The role only ever goes DOWN: an operator carry (absent or
//!   `"operator"`) held by a person who is now a Member is capped at
//!   `"member"`; any other carry (`"guest"`, `"member"`) is left as it is,
//!   and an active admin gets no role stamp at all — its authority is
//!   unchanged. It still gets the scope / author attribution stamps: on a
//!   single-user install the loopback-owned rows name `u-owner`, which
//!   resolves `Granted`, not `Legacy`.
//! - A users-store READ error is `Unknown`, never `Refused`: "I could not
//!   look" is not evidence the person is gone (判据 §8 / §15). Callers skip
//!   this fire and keep the work armed.

use std::collections::HashMap;

use crate::gateway::security::store::slot::UsersAuthority;
use crate::gateway::security::store::{SecurityStore, UserRole, UserStatus, OWNER_USER_ID};
use crate::scope::{CarriedAttribution, ScopeAttribution};

/// Why a fire was refused. Both are CONFIRMED facts read from the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    /// The person's row exists and is deactivated.
    Deactivated,
    /// The person's id names no row at all (dangling reference).
    Gone,
}

/// What a trigger knows about the work it is about to fire.
#[derive(Debug, Clone, Copy, Default)]
pub struct FireSubject<'a> {
    /// `owner_user_id` of the job / task / team / session row.
    pub owner: Option<&'a str>,
    /// `scope_id` as persisted beside `owner`.
    pub scope: Option<&'a str>,
    /// The original initiator (`AUTHOR_USER_KEY`), when the work carries one.
    pub author: Option<&'a str>,
    /// The `caller_role` carried in the work's metadata, when it carries one.
    pub carried_role: Option<&'a str>,
}

/// The attribution a permitted fire runs under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Granted {
    pub scope: Option<ScopeAttribution>,
    pub author: Option<String>,
    /// `Some("member")` only when downgrading an operator carry.
    pub role_ceiling: Option<&'static str>,
    /// The carried role, kept so [`Self::carried`] can re-establish a
    /// non-operator carry (`"guest"`) that `role_ceiling` deliberately does
    /// not repeat. Private: without it `carried()` would publish `None` —
    /// which `role_is_operator` reads as operator — for a guest's work.
    carried_role: Option<String>,
}

/// The resolver's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireAuthority {
    /// Nobody to check (no owner and no author: an unattributed row written
    /// before rows carried an owner), or no users store installed. Not a
    /// synonym for "single-user install": rows owned by `u-owner` resolve
    /// `Granted` (an active admin — attribution stamps, no ceiling). Behave
    /// exactly as before this module existed — see [`Granted::legacy`].
    Legacy,
    Granted(Granted),
    /// Confirmed walled or dangling person: pause / refuse the work.
    Refused(RefusalReason),
    /// The store could not be read: skip THIS fire, keep the work armed,
    /// ask again next time (R-a).
    Unknown(String),
}

impl Granted {
    /// The attribution a [`FireAuthority::Legacy`] verdict runs under:
    /// exactly the persisted pair, the carried author and the carried
    /// role, with no ceiling — what every executor stamped before this
    /// resolver existed. Every executor answers `Legacy` with this, so
    /// "legacy" stays one definition rather than one per executor.
    #[must_use]
    pub fn legacy(subject: &FireSubject<'_>) -> Self {
        Self {
            scope: ScopeAttribution::from_persisted(subject.owner, subject.scope),
            author: subject.author.map(str::to_string),
            role_ceiling: None,
            carried_role: subject.carried_role.map(str::to_string),
        }
    }

    /// Write this attribution into a run's metadata: the two scope keys
    /// (via [`crate::scope::stamp_metadata`]), `AUTHOR_USER_KEY` when an
    /// author is carried, and `caller_role` ONLY when capping — an
    /// uncapped fire leaves whatever `caller_role` the metadata already
    /// carries untouched.
    pub fn stamp(&self, metadata: &mut HashMap<String, String>) {
        if let Some(attr) = &self.scope {
            crate::scope::stamp_metadata(metadata, attr);
        }
        if let Some(author) = &self.author {
            metadata.insert(
                crate::gateway::execution_engine::AUTHOR_USER_KEY.to_string(),
                author.clone(),
            );
        }
        if let Some(role) = self.role_ceiling {
            metadata.insert("caller_role".to_string(), role.to_string());
        }
    }

    /// The task-local carrier for work that runs through `tokio::spawn`
    /// instead of a metadata map. The role published is the ceiling when
    /// one applies, else the carried role — never "absent" for a carry
    /// that was present.
    #[must_use]
    pub fn carried(&self) -> CarriedAttribution {
        let role = self
            .role_ceiling
            .map(str::to_string)
            .or_else(|| self.carried_role.clone());
        CarriedAttribution::from_parts(self.scope.clone(), None, role, self.author.clone())
    }
}

impl FireAuthority {
    /// Human-readable reason for a verdict that stops a fire; `None` for
    /// verdicts that let it run. Written verbatim into fire logs / tick
    /// results — a stop that does not say why is fail-dead (判据 §14).
    ///
    /// "principal", not "owner": the person checked is the author when one
    /// is carried, so for a member's work in someone else's room the owner
    /// is the wrong person to name.
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::Refused(RefusalReason::Deactivated) => Some("principal deactivated".to_string()),
            Self::Refused(RefusalReason::Gone) => Some("principal gone".to_string()),
            Self::Unknown(err) => Some(format!("authority unknown: {err}")),
            Self::Legacy | Self::Granted(_) => None,
        }
    }
}

/// Who [`resolve_with`] checks for `subject`, and whether that person is
/// the carried author (`true`) or the owner (`false`): the author when one is
/// carried, else the owner, an empty id read as absent. `None` = nobody to
/// check (`Legacy`).
///
/// The one derivation of "the person checked": `resolve_with` asks it, and so
/// does every refusal text that has to name that person, so the name printed
/// can never be a different person from the one refused.
#[must_use]
pub fn checked_person<'a>(subject: &FireSubject<'a>) -> Option<(&'a str, bool)> {
    let author = subject.author.filter(|s| !s.is_empty());
    let owner = subject.owner.filter(|s| !s.is_empty());
    match author {
        Some(a) => Some((a, true)),
        None => owner.map(|o| (o, false)),
    }
}

/// Resolve against an explicit store. `None` store ⇒ `Legacy` — the
/// `FailsOpen` contract of `security/users-store`, stated at the slot.
/// A store handed in here is taken as the durable table; the degraded
/// (in-memory fallback) state is [`resolve_with_users`]'s.
///
/// An empty author or owner is read as absent: a malformed
/// `AUTHOR_USER_KEY` of `""` falls back to the owner instead of being looked
/// up (and refused as `Gone`), and both empty is `Legacy`.
#[must_use]
pub fn resolve_with(users: Option<&SecurityStore>, subject: &FireSubject<'_>) -> FireAuthority {
    resolve_with_users(UsersAuthority::from_store(users), subject)
}

/// Resolve against the users slot's state — the seam that carries the
/// degraded mark.
///
/// Under [`UsersAuthority::Degraded`] (boot fell back to the in-memory store,
/// which holds only the bootstrap owner) a checked person other than
/// [`OWNER_USER_ID`] is `Unknown`, never `Gone`: that table's missing row is
/// "I could not look", and `Gone` pauses, disables and tombstones work in
/// ways the durable store's return cannot undo (判据 §8, §15). The owner
/// resolves as always — it cannot be deactivated or demoted, so the
/// fallback's answer for it is the real one.
#[must_use]
pub(crate) fn resolve_with_users(
    users: UsersAuthority<'_>,
    subject: &FireSubject<'_>,
) -> FireAuthority {
    let author = subject.author.filter(|s| !s.is_empty());
    let Some((person, _)) = checked_person(subject) else {
        return FireAuthority::Legacy;
    };
    let users = match users {
        UsersAuthority::Absent => return FireAuthority::Legacy,
        UsersAuthority::Durable(store) => store,
        UsersAuthority::Degraded { reason, .. } if person != OWNER_USER_ID => {
            return FireAuthority::Unknown(format!(
                "users store is the in-memory fallback: {reason}"
            ));
        }
        UsersAuthority::Degraded { store, .. } => store,
    };
    let record = match users.get_user(person) {
        Err(e) => return FireAuthority::Unknown(e.to_string()),
        Ok(None) => return FireAuthority::Refused(RefusalReason::Gone),
        Ok(Some(record)) => record,
    };
    match record.status {
        UserStatus::Deactivated => FireAuthority::Refused(RefusalReason::Deactivated),
        UserStatus::Active => {
            let capped = crate::tools::turn_context::role_is_operator(subject.carried_role)
                && record.role == UserRole::Member;
            FireAuthority::Granted(Granted {
                scope: ScopeAttribution::from_persisted(subject.owner, subject.scope),
                // The FILTERED author: an empty `AUTHOR_USER_KEY` was read as
                // absent above, so it must not be stamped back as `""`.
                author: author.map(str::to_string),
                role_ceiling: capped.then(|| UserRole::Member.wire_role()),
                carried_role: subject.carried_role.map(str::to_string),
            })
        }
    }
}

/// Resolve against the boot-installed users store
/// (`security::store::slot::users_authority`). With none installed — tests,
/// minimal servers — every subject is `Legacy`.
#[must_use]
pub fn resolve(subject: &FireSubject<'_>) -> FireAuthority {
    resolve_with_users(
        crate::gateway::security::store::slot::users_authority(),
        subject,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::execution_engine::AUTHOR_USER_KEY;

    /// `u-owner` (active Admin, bootstrapped), `u-alice` (active Member),
    /// `u-walled` (deactivated Member).
    fn store() -> SecurityStore {
        let s = SecurityStore::in_memory().unwrap();
        s.create_user("u-alice", "Alice", UserRole::Member).unwrap();
        s.create_user("u-walled", "Walled", UserRole::Member)
            .unwrap();
        s.update_user("u-walled", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        s
    }

    /// The in-memory fallback store (boot could not open `security.db`)
    /// holds only the owner: a member's missing row there is `Unknown`, never
    /// `Gone`, while the owner still resolves and an unattributed row stays
    /// `Legacy`. The same table read as durable says `Gone` — which is exactly
    /// what the degraded mark exists to prevent.
    #[test]
    fn a_degraded_store_leaves_members_unknown_and_still_resolves_the_owner() {
        let fallback = SecurityStore::in_memory().unwrap();
        fallback.ensure_bootstrap_owner().unwrap();
        let degraded = UsersAuthority::Degraded {
            store: &fallback,
            reason: "security.db could not be opened",
        };
        let member = FireSubject {
            owner: Some("u-alice"),
            ..FireSubject::default()
        };
        match resolve_with_users(degraded, &member) {
            FireAuthority::Unknown(why) => {
                assert!(why.contains("in-memory fallback"), "{why}");
            }
            other => panic!("a member against the fallback store must be Unknown, got {other:?}"),
        }
        assert_eq!(
            resolve_with(Some(&fallback), &member),
            FireAuthority::Refused(RefusalReason::Gone)
        );
        let owner = FireSubject {
            owner: Some(OWNER_USER_ID),
            ..FireSubject::default()
        };
        assert!(matches!(
            resolve_with_users(degraded, &owner),
            FireAuthority::Granted(_)
        ));
        assert_eq!(
            resolve_with_users(degraded, &FireSubject::default()),
            FireAuthority::Legacy
        );
    }

    fn owned<'a>(owner: &'a str, scope: &'a str) -> FireSubject<'a> {
        FireSubject {
            owner: Some(owner),
            scope: Some(scope),
            ..FireSubject::default()
        }
    }

    fn granted(v: FireAuthority) -> Granted {
        match v {
            FireAuthority::Granted(g) => g,
            other => panic!("expected Granted, got {other:?}"),
        }
    }

    #[test]
    fn no_person_or_no_store_is_legacy() {
        let s = store();
        assert_eq!(
            resolve_with(Some(&s), &FireSubject::default()),
            FireAuthority::Legacy,
            "no owner and no author: nobody to check"
        );
        assert_eq!(
            resolve_with(None, &owned("u-walled", "personal:u-walled")),
            FireAuthority::Legacy,
            "no store installed is Legacy even for a walled owner — FailsOpen"
        );
    }

    #[test]
    fn an_active_admin_stamps_nothing_beyond_the_scope() {
        let subject = owned(OWNER_USER_ID, "personal:u-owner");
        let g = granted(resolve_with(Some(&store()), &subject));
        assert_eq!(g.role_ceiling, None);
        assert_eq!(
            g,
            Granted::legacy(&subject),
            "ruling C3: an active admin's attribution is exactly the legacy one"
        );
        let mut m = HashMap::new();
        g.stamp(&mut m);
        let mut keys: Vec<&str> = m.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![crate::scope::SCOPE_META_KEY, crate::scope::OWNER_META_KEY],
            "an active admin's fire must carry no caller_role and no author"
        );
    }

    #[test]
    fn an_active_member_with_an_operator_carry_is_capped_at_member() {
        let s = store();
        for carry in [None, Some("operator")] {
            let subject = FireSubject {
                carried_role: carry,
                ..owned("u-alice", "personal:u-alice")
            };
            let g = granted(resolve_with(Some(&s), &subject));
            assert_eq!(g.role_ceiling, Some("member"), "carry {carry:?}");
            let mut m = HashMap::from([("caller_role".to_string(), "operator".to_string())]);
            g.stamp(&mut m);
            assert_eq!(m.get("caller_role").map(String::as_str), Some("member"));
        }
    }

    #[test]
    fn a_member_with_a_guest_carry_stays_guest() {
        let subject = FireSubject {
            carried_role: Some("guest"),
            ..owned("u-alice", "personal:u-alice")
        };
        let g = granted(resolve_with(Some(&store()), &subject));
        assert_eq!(g.role_ceiling, None, "the ceiling never raises a guest");
        let mut m = HashMap::from([("caller_role".to_string(), "guest".to_string())]);
        g.stamp(&mut m);
        assert_eq!(m.get("caller_role").map(String::as_str), Some("guest"));
    }

    #[test]
    fn a_deactivated_person_is_refused() {
        let v = resolve_with(Some(&store()), &owned("u-walled", "personal:u-walled"));
        assert_eq!(v, FireAuthority::Refused(RefusalReason::Deactivated));
        assert_eq!(v.reason().as_deref(), Some("principal deactivated"));
    }

    #[test]
    fn a_person_with_no_row_is_refused_as_gone() {
        let v = resolve_with(Some(&store()), &owned("u-ghost", "personal:u-ghost"));
        assert_eq!(v, FireAuthority::Refused(RefusalReason::Gone));
        assert_eq!(v.reason().as_deref(), Some("principal gone"));
    }

    /// R-a: a read error is "I do not know", never "refused".
    #[test]
    fn a_store_read_error_is_unknown_not_refused() {
        let s = store();
        s.conn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .execute("DROP TABLE users", [])
            .unwrap();
        let v = resolve_with(Some(&s), &owned("u-alice", "personal:u-alice"));
        assert!(matches!(v, FireAuthority::Unknown(_)), "got {v:?}");
        assert!(v.reason().unwrap().starts_with("authority unknown: "));
    }

    /// R-b / R-d: the author is the person checked, not the room's owner.
    #[test]
    fn the_author_is_checked_not_the_owner() {
        let s = store();
        let walled_room = FireSubject {
            author: Some("u-alice"),
            ..owned("u-walled", "project:p-room")
        };
        let g = granted(resolve_with(Some(&s), &walled_room));
        assert_eq!(g.author.as_deref(), Some("u-alice"));
        let mut m = HashMap::new();
        g.stamp(&mut m);
        assert_eq!(m.get(AUTHOR_USER_KEY).map(String::as_str), Some("u-alice"));

        let walled_author = FireSubject {
            author: Some("u-walled"),
            ..owned("u-alice", "project:p-room")
        };
        assert_eq!(
            resolve_with(Some(&s), &walled_author),
            FireAuthority::Refused(RefusalReason::Deactivated)
        );
    }

    #[test]
    fn legacy_is_the_persisted_pair_and_nothing_else() {
        let g = Granted::legacy(&owned("u-alice", "personal:u-alice"));
        assert_eq!(g.scope, Some(ScopeAttribution::personal("u-alice")));
        assert_eq!((g.author, g.role_ceiling), (None, None));
        assert_eq!(
            Granted::legacy(&FireSubject::default()).scope,
            None,
            "an unowned subject stamps nothing"
        );
        let guest = Granted::legacy(&FireSubject {
            carried_role: Some("guest"),
            ..owned("u-alice", "personal:u-alice")
        });
        assert_eq!(
            guest.carried_role.as_deref(),
            Some("guest"),
            "legacy must keep the carried role, or carried() publishes an \
             absent (= operator) role for a guest's work"
        );
    }

    /// A malformed empty author is absent, not a person named "": the
    /// owner is checked instead (and is found), rather than `Gone`.
    #[test]
    fn an_empty_author_falls_back_to_the_owner() {
        let s = store();
        let subject = FireSubject {
            author: Some(""),
            ..owned("u-alice", "personal:u-alice")
        };
        let g = granted(resolve_with(Some(&s), &subject));
        assert_eq!(
            g.role_ceiling,
            Some("member"),
            "u-alice (a member) was checked"
        );
        assert_eq!(
            g.author, None,
            "an empty author must not be carried back as \"\""
        );
        let mut m = HashMap::new();
        g.stamp(&mut m);
        assert!(
            !m.contains_key(AUTHOR_USER_KEY),
            "stamp() writes no AUTHOR_USER_KEY for an absent author: {m:?}"
        );
        let walled = FireSubject {
            author: Some(""),
            ..owned("u-walled", "personal:u-walled")
        };
        assert_eq!(
            resolve_with(Some(&s), &walled),
            FireAuthority::Refused(RefusalReason::Deactivated),
            "the owner is the person checked when the author is empty"
        );
        assert_eq!(
            resolve_with(
                Some(&s),
                &FireSubject {
                    owner: Some(""),
                    author: Some(""),
                    ..FireSubject::default()
                }
            ),
            FireAuthority::Legacy,
            "both empty: nobody to check"
        );
    }

    /// Lib tests never install the global store (see the slot's own test),
    /// so `resolve` must read `Legacy` here even for a walled owner.
    #[test]
    fn resolve_reads_legacy_while_no_store_is_installed() {
        assert_eq!(
            resolve(&owned("u-walled", "personal:u-walled")),
            FireAuthority::Legacy
        );
    }

    /// The spawn-path half: `carried()` must re-establish the scope, the
    /// author and the effective role — the ceiling when capped, the carried
    /// role otherwise, never an absent role for a present carry.
    #[tokio::test]
    async fn carried_reestablishes_scope_author_and_role_across_a_spawn() {
        let s = store();
        let capped = granted(resolve_with(
            Some(&s),
            &FireSubject {
                author: Some("u-alice"),
                ..owned("u-alice", "personal:u-alice")
            },
        ));
        let guest = granted(resolve_with(
            Some(&s),
            &FireSubject {
                carried_role: Some("guest"),
                ..owned("u-alice", "personal:u-alice")
            },
        ));
        let probe = || async {
            (
                crate::scope::current_scope(),
                crate::scope::current_room_author(),
                crate::gateway::caller_identity::current_caller_role(),
            )
        };
        let seen = tokio::spawn(capped.carried().reestablish(probe()))
            .await
            .unwrap();
        assert_eq!(
            seen,
            (
                Some(ScopeAttribution::personal("u-alice")),
                Some("u-alice".to_string()),
                Some("member".to_string())
            )
        );
        let seen = tokio::spawn(guest.carried().reestablish(probe()))
            .await
            .unwrap();
        assert_eq!(
            seen.2.as_deref(),
            Some("guest"),
            "a guest carry must not come back as an absent (= operator) role"
        );
    }
}
