//! Principal management contract — `users.{me,list,create,update}`.
//!
//! # Why these types live here
//!
//! This family has **three** hand-written spellings of one wire shape: the
//! handler's `#[derive(Serialize)] UserView`, the CLI's `u.get("display_name")`
//! string lookups, and the Panel's `UserInfo` DTO. Three authors for one fact
//! is the shape that made every `aleph workspace create` fail with
//! `INVALID_PARAMS` for months while both sides' tests stayed green, and the
//! shape that printed a column of dashes for `providers list`. One type makes
//! a rename a compile error in all three places.
//!
//! # Why the update receipt is typed and not a `json!` literal
//!
//! `users.update` grew a real receipt: how many devices were revoked, how many
//! goals/loops/crons were frozen, which channel senders were withdrawn, and —
//! on reactivation — an explicit list of what did **not** come back. It was
//! assembled as a `json!` literal, and the only client printed none of it,
//! substituting a hard-coded sentence claiming devices had been revoked whether
//! any had or not. A receipt that no client can render is a receipt that does
//! not exist; a typed one is one the client cannot silently drop a field from
//! without the compiler noticing something changed.

use serde::{Deserialize, Serialize};

/// A principal as every surface renders it. `role` and `status` are their wire
/// strings so no client ever touches the server's enum representation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserView {
    pub user_id: String,
    pub display_name: String,
    /// `"admin"` or `"member"`.
    pub role: String,
    /// `"active"` or `"deactivated"`.
    pub status: String,
}

/// The wire spelling of a principal a roster may currently name.
///
/// One spelling, not two: `alephcore`'s `UserStatus::as_str` returns THIS
/// constant, so a rename is a compile error on both sides rather than a
/// silently non-matching comparison here. A string compared against a literal
/// typed in the reader is the shape that lets a status filter quietly stop
/// filtering.
pub const ACTIVE_STATUS: &str = "active";

impl UserView {
    /// Whether this principal is one a roster may currently name.
    ///
    /// This is a **narrowing** predicate, not the authorization one: the
    /// server still asks `projects::authz::is_active_principal` — which reads
    /// the store rather than a projection of it — before any grant lands. A
    /// view can be stale; the store cannot.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == ACTIVE_STATUS
    }
}

/// What a display name resolved to.
///
/// Three answers, deliberately, because a caller that collapses them is the
/// defect: `Ambiguous` folded into "take the first" seats the wrong person,
/// and `None` folded into "pass the name through as an id" turns "I do not
/// know" into an id and seats a row nobody owns (criterion #8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one active principal bears this name.
    One(UserView),
    /// More than one does. Every candidate, so the caller can re-ask by id.
    Ambiguous(Vec<UserView>),
    /// Nobody a roster may name does — including the case where the only
    /// bearers are deactivated. A deactivated principal reads as absent on
    /// purpose: telling a caller "they exist but are switched off" is an
    /// existence oracle over the principal directory.
    None,
}

/// Fold a name to the form two spellings of the same person share.
fn folded(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Resolve a display name to the one active principal that bears it.
///
/// Case- and surrounding-whitespace-insensitive, because the name reaches
/// here through a model relaying what a human said, and `"bob"` for `"Bob"`
/// is not a different person. Two principals whose names differ only in case
/// are `Ambiguous`, not a winner and a loser: picking one would make the
/// answer depend on which of them was created first.
///
/// The empty name resolves to `None` rather than to whoever happens to have a
/// blank display name — an absent argument must not address anybody.
#[must_use]
pub fn resolve(name: &str, users: &[UserView]) -> Resolution {
    let needle = folded(name);
    if needle.is_empty() {
        return Resolution::None;
    }
    let mut hits: Vec<UserView> = users
        .iter()
        .filter(|u| u.is_active() && folded(&u.display_name) == needle)
        .cloned()
        .collect();
    match hits.len() {
        0 => Resolution::None,
        1 => Resolution::One(hits.remove(0)),
        _ => Resolution::Ambiguous(hits),
    }
}

/// Parameters for `users.create`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCreateParams {
    pub display_name: String,
    /// Absent means the server's own default (`member`). Clients must omit it
    /// rather than sending `null` or guessing the default themselves — the
    /// default is one decision and it belongs to the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Parameters for `users.update`. Every optional field is a patch: absent
/// means "leave alone", which is why none of them may be sent as `null`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserUpdateParams {
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// `"active"` or `"deactivated"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// `users.me` → the caller's own record.
///
/// `user: None` is not an error: a loopback / unrestricted connection with no
/// P1 identity attached has no principal to report, and collapsing that into an
/// error would make every single-user surface handle a failure that is really
/// an absence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMeResult {
    #[serde(default)]
    pub user: Option<UserView>,
}

/// `users.list` → every principal this core knows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserListResult {
    pub users: Vec<UserView>,
}

/// `users.create` → the principal that now exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCreateResult {
    pub user: UserView,
}

/// One channel sender approval withdrawn by a deactivation.
///
/// A person is bound to this server by **two** independent credentials — a
/// device ticket and a channel sender approval — and deactivation must cut
/// both. Naming each withdrawn binding is what lets an operator verify that it
/// did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokedChannelSender {
    pub channel: String,
    pub sender_id: String,
}

/// The four background legs a deactivation freezes.
///
/// Heartbeat was the fourth and it arrived a round late. Until then this doc
/// read "the three background legs a deactivation freezes" — a complete
/// inventory, in the voice of the thing that would know — while a deactivated
/// second admin's heartbeat tasks kept firing and kept delivering. A receipt
/// that names three of four legs is worse than one that names none: the
/// operator reads it as coverage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenBackgroundWork {
    pub goals: usize,
    pub loops: usize,
    pub crons: usize,
    /// `None` means this leg was **not measured**: the heartbeat service is
    /// not running in this process (`[heartbeat] enabled = false`, or its
    /// store failed to open), so any heartbeat task the principal owns is
    /// still armed and the deactivation did not reach it.
    ///
    /// A fail-closed answer is only allowed to say "I do not know". Folding it
    /// into `0` would make it read as "they owned none" — the same shape that
    /// let this whole struct under-report for a round.
    ///
    /// Absent on the wire rather than `null`, matching `revoked_devices`: a
    /// measured zero and an absent measurement are different answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeats: Option<usize>,
}

impl FrozenBackgroundWork {
    /// Whether every leg that was **measured** came back zero — so a receipt
    /// can stay quiet rather than printing four zeros.
    ///
    /// An unmeasured heartbeat leg (`heartbeats: None`) does NOT make this
    /// false. It is not a freeze that found nothing, it is a freeze that never
    /// ran, and the renderer gives it its own sentence rather than letting it
    /// ride inside this one.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.goals == 0
            && self.loops == 0
            && self.crons == 0
            && matches!(self.heartbeats, None | Some(0))
    }
}

/// What reactivation did **not** restore.
///
/// Reactivation is a single column write: devices stay revoked, channel senders
/// stay withdrawn, background work stays paused. `status: "active"` on its own
/// reads as if the principal were whole again, so each field carries the
/// server's own recovery verb. The strings are server-authored on purpose —
/// letting the client word them would give one fact two authors, and the client
/// is the half that cannot know which subsystems exist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactivationEffects {
    pub devices: String,
    pub channel_senders: String,
    pub background_work: String,
}

/// `users.update` → the updated principal **and what the write actually did**.
///
/// The three optional blocks are present only for the transition that produces
/// them, so their absence is information too: no `revoked_devices` key means
/// this was not a deactivation, not that it revoked nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserUpdateResult {
    pub user: UserView,
    /// Always present (possibly empty) — a deactivation that withdrew nothing
    /// and a non-deactivation must not render the same.
    #[serde(default)]
    pub revoked_channel_senders: Vec<RevokedChannelSender>,
    /// Deactivations only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_devices: Option<usize>,
    /// Outstanding bootstrap tickets burned by the deactivation. Deactivations
    /// only.
    ///
    /// A separate count from `revoked_devices` because it names a different
    /// credential at a different stage: `revoked_devices` cuts pairings that
    /// already happened, this cuts the ones that had not happened yet and
    /// would otherwise have produced a brand-new device row **after** the
    /// sweep ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_bootstrap_tickets: Option<usize>,
    /// Deactivations only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_background_work: Option<FrozenBackgroundWork>,
    /// The deactivated→active transition only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reactivation_effects: Option<ReactivationEffects>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(id: &str, name: &str, status: &str) -> UserView {
        UserView {
            user_id: id.to_string(),
            display_name: name.to_string(),
            role: "member".to_string(),
            status: status.to_string(),
        }
    }

    /// The happy path the roster verbs exist for: one bearer, one answer.
    #[test]
    fn one_active_bearer_of_a_name_resolves_to_that_principal() {
        let dir = [
            named("u-bob", "Bob", "active"),
            named("u-ada", "Ada", "active"),
        ];
        assert_eq!(
            resolve("Bob", &dir),
            Resolution::One(named("u-bob", "Bob", "active"))
        );
        assert_eq!(
            resolve("  bob ", &dir),
            Resolution::One(named("u-bob", "Bob", "active")),
            "a relayed name arrives with the human's spacing and casing"
        );
    }

    /// Two bearers is not "the first one". Returning `One` here would seat the
    /// wrong person on a room, and the caller could not tell it had happened.
    #[test]
    fn two_active_bearers_are_ambiguous_and_name_every_candidate() {
        let dir = [
            named("u-bob1", "Bob", "active"),
            named("u-bob2", "bob", "active"),
        ];
        let Resolution::Ambiguous(candidates) = resolve("Bob", &dir) else {
            panic!("two bearers must not resolve to one");
        };
        let ids: Vec<&str> = candidates.iter().map(|c| c.user_id.as_str()).collect();
        assert_eq!(
            ids,
            ["u-bob1", "u-bob2"],
            "every candidate, so the caller can re-ask by id"
        );
    }

    /// A deactivated bearer is not a bearer. The lone-deactivated case must be
    /// indistinguishable from "nobody by that name" — otherwise the refusal is
    /// an existence oracle over the principal directory.
    #[test]
    fn a_deactivated_bearer_is_never_the_answer() {
        let both = [
            named("u-gone", "Bob", "deactivated"),
            named("u-live", "Bob", "active"),
        ];
        assert_eq!(
            resolve("Bob", &both),
            Resolution::One(named("u-live", "Bob", "active")),
            "the homonym who is switched off must not make this ambiguous"
        );
        assert_eq!(
            resolve("Bob", &[named("u-gone", "Bob", "deactivated")]),
            Resolution::None,
            "a lone deactivated bearer reads exactly like nobody"
        );
    }

    /// An absent argument must not address anybody — including a principal
    /// whose display name is itself blank.
    #[test]
    fn the_empty_name_addresses_nobody() {
        let dir = [named("u-blank", "   ", "active")];
        assert_eq!(resolve("", &dir), Resolution::None);
        assert_eq!(resolve("   ", &dir), Resolution::None);
    }

    /// `is_active` is compared against the shared constant, not a literal
    /// re-typed here — the server's `UserStatus::as_str` returns that same
    /// constant, so the two halves cannot drift into never matching.
    #[test]
    fn active_is_one_spelling_shared_with_the_server() {
        assert_eq!(ACTIVE_STATUS, "active");
        assert!(named("u-x", "X", ACTIVE_STATUS).is_active());
        assert!(!named("u-x", "X", "deactivated").is_active());
    }

    fn view() -> UserView {
        UserView {
            user_id: "u-alice".to_string(),
            display_name: "Alice".to_string(),
            role: "member".to_string(),
            status: "active".to_string(),
        }
    }

    #[test]
    fn an_absent_role_is_omitted_rather_than_nulled() {
        let wire = serde_json::to_value(UserCreateParams {
            display_name: "Alice".to_string(),
            role: None,
        })
        .unwrap();
        assert_eq!(wire, serde_json::json!({"display_name": "Alice"}));
    }

    #[test]
    fn an_update_patch_sends_only_the_fields_it_changes() {
        let wire = serde_json::to_value(UserUpdateParams {
            user_id: "u-alice".to_string(),
            status: Some("deactivated".to_string()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"user_id": "u-alice", "status": "deactivated"}),
            "a null display_name would be a request to rename to nothing"
        );
    }

    /// A plain rename must not carry the deactivation blocks — their absence is
    /// how a client knows this write was not a deactivation.
    #[test]
    fn a_non_deactivating_update_omits_the_deactivation_blocks() {
        let wire = serde_json::to_value(UserUpdateResult {
            user: view(),
            revoked_channel_senders: vec![],
            revoked_devices: None,
            revoked_bootstrap_tickets: None,
            frozen_background_work: None,
            reactivation_effects: None,
        })
        .unwrap();
        // Set, not sequence: `serde_json::Map` is a `BTreeMap` here, so key
        // order is the map's property and not the contract's. Asserting the
        // order would pin a fact this type does not promise.
        let keys: std::collections::BTreeSet<_> =
            wire.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            ["revoked_channel_senders".to_string(), "user".to_string()]
                .into_iter()
                .collect()
        );
    }

    /// Zero is a measurement, `None` is a different question. A deactivation
    /// that found no devices must still say so.
    #[test]
    fn a_deactivation_that_revoked_nothing_still_reports_zero() {
        let wire = serde_json::to_value(UserUpdateResult {
            user: view(),
            revoked_channel_senders: vec![],
            revoked_devices: Some(0),
            revoked_bootstrap_tickets: Some(0),
            frozen_background_work: Some(FrozenBackgroundWork::default()),
            reactivation_effects: None,
        })
        .unwrap();
        assert_eq!(wire["revoked_devices"], 0);
        assert!(
            wire.get("frozen_background_work").is_some(),
            "a measured zero and an absent measurement are different answers"
        );
    }

    #[test]
    fn a_result_round_trips_through_the_wire() {
        let original = UserUpdateResult {
            user: view(),
            revoked_channel_senders: vec![RevokedChannelSender {
                channel: "telegram".to_string(),
                sender_id: "12345".to_string(),
            }],
            revoked_devices: Some(2),
            revoked_bootstrap_tickets: Some(1),
            frozen_background_work: Some(FrozenBackgroundWork {
                goals: 1,
                loops: 0,
                crons: 3,
                heartbeats: Some(2),
            }),
            reactivation_effects: None,
        };
        let wire = serde_json::to_value(&original).unwrap();
        let back: UserUpdateResult = serde_json::from_value(wire).unwrap();
        assert_eq!(back, original);
    }

    /// A burned bootstrap ticket is a credential the deactivation cut, and it
    /// is a DIFFERENT credential from a revoked device: the device existed,
    /// the ticket had not been redeemed yet and would have produced a fresh
    /// device row after the sweep. Folding it into `revoked_devices` would
    /// make the two indistinguishable on the wire.
    #[test]
    fn burned_bootstrap_tickets_are_their_own_count_on_the_receipt() {
        let wire = serde_json::to_value(UserUpdateResult {
            user: view(),
            revoked_channel_senders: vec![],
            revoked_devices: Some(1),
            revoked_bootstrap_tickets: Some(2),
            frozen_background_work: None,
            reactivation_effects: None,
        })
        .unwrap();
        assert_eq!(wire["revoked_devices"], 1);
        assert_eq!(wire["revoked_bootstrap_tickets"], 2);
    }

    /// A deactivation that found no outstanding ticket must still say so —
    /// zero is a measurement, absence says "this was not a deactivation".
    #[test]
    fn a_deactivation_that_burned_no_ticket_still_reports_zero() {
        let wire = serde_json::to_value(UserUpdateResult {
            user: view(),
            revoked_channel_senders: vec![],
            revoked_devices: Some(0),
            revoked_bootstrap_tickets: Some(0),
            frozen_background_work: None,
            reactivation_effects: None,
        })
        .unwrap();
        assert_eq!(wire["revoked_bootstrap_tickets"], 0);
    }

    /// The heartbeat leg is a fourth count on the same receipt, not a separate
    /// block — a client that renders three of four numbers is the #17 defect
    /// this field exists to close.
    #[test]
    fn a_measured_heartbeat_leg_is_a_number_on_the_wire() {
        let wire = serde_json::to_value(FrozenBackgroundWork {
            goals: 0,
            loops: 0,
            crons: 0,
            heartbeats: Some(1),
        })
        .unwrap();
        assert_eq!(wire["heartbeats"], 1);
    }

    /// The fail-closed half (criterion #8): a heartbeat leg that could not run
    /// says nothing at all, so no reader can mistake it for "they owned none".
    /// `null` would be a third spelling of the same doubt; absence is the one
    /// `revoked_devices` already established.
    #[test]
    fn an_unmeasured_heartbeat_leg_is_absent_not_zero_and_not_null() {
        let wire = serde_json::to_value(FrozenBackgroundWork::default()).unwrap();
        assert!(
            wire.get("heartbeats").is_none(),
            "an unmeasured leg must not serialize at all, got {wire}"
        );
        let back: FrozenBackgroundWork = serde_json::from_value(wire).unwrap();
        assert_eq!(back.heartbeats, None);
    }

    /// `is_empty` decides whether the receipt stays quiet. A frozen heartbeat
    /// task must break that silence even when the other three legs are zero —
    /// otherwise the field exists and nothing ever renders it.
    #[test]
    fn a_receipt_with_only_heartbeats_frozen_is_not_empty() {
        assert!(
            !FrozenBackgroundWork {
                goals: 0,
                loops: 0,
                crons: 0,
                heartbeats: Some(3),
            }
            .is_empty(),
            "3 disabled heartbeat tasks must not render as 'they owned nothing'"
        );
        assert!(FrozenBackgroundWork {
            goals: 0,
            loops: 0,
            crons: 0,
            heartbeats: Some(0),
        }
        .is_empty());
        assert!(
            FrozenBackgroundWork::default().is_empty(),
            "an unmeasured leg is not a freeze; it gets its own sentence, so it \
             must not force the 'work was frozen' branch"
        );
    }
}
