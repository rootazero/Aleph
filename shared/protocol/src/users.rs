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

/// The three background legs a deactivation freezes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenBackgroundWork {
    pub goals: usize,
    pub loops: usize,
    pub crons: usize,
}

impl FrozenBackgroundWork {
    /// Whether anything was actually frozen — so a receipt can stay quiet
    /// rather than printing three zeros.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.goals == 0 && self.loops == 0 && self.crons == 0
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
            frozen_background_work: Some(FrozenBackgroundWork {
                goals: 1,
                loops: 0,
                crons: 3,
            }),
            reactivation_effects: None,
        };
        let wire = serde_json::to_value(&original).unwrap();
        let back: UserUpdateResult = serde_json::from_value(wire).unwrap();
        assert_eq!(back, original);
    }
}
