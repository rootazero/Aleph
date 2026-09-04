//! Paired-device inventory contract — the `gateway.devices.list` response.
//!
//! # Why this type lives here and not next to the handler
//!
//! `gateway.devices.list` is the ONE surface on which an operator sees, and
//! therefore revokes, a paired credential. Its shape used to be written twice:
//! once as a per-row `serde_json::json!` literal in the handler, once as a
//! hand-rolled `d.get("…")?` walk in the Panel. Nothing connected them.
//!
//! Its sibling credential family already ate the bug that arrangement invites
//! — see [`crate::channel_pairing`], where the Panel read an array under a key
//! the server had never emitted and the page rendered "no approved senders" on
//! a channel that had several. Here the two spellings happen to agree today,
//! so this module buys two things rather than a repair: a rename becomes a
//! compile error on both sides instead of a silently thinner list, and
//! over-sending becomes a compile impossibility rather than an untested hope.
//! `created_at` was being sent since the list shipped and rendered by no
//! client in any language; it is gone with the literal it lived in.
//!
//! **Construct the response from these types.** Parsing a hand-written literal
//! *into* them would only ever prove the server sends a superset — the
//! direction `workspace.*` and `channel.pairing.approved` both had to learn.
//!
//! # Why the optionality is asymmetric
//!
//! `last_seen_at` / `user_id` / `display_name` are `Option` + `#[serde(default)]`
//! because over-strictness on a credential inventory blanks the WHOLE list, and
//! a blank inventory is an inventory an operator cannot revoke from — the
//! explicit warning [`crate::channel_pairing::ApprovedSenderRow`] carries.
//!
//! `connected` is deliberately **required** — the only field on a ROW whose
//! absence a lenient default would turn into a claim: `unwrap_or(false)`
//! renders a missing key as "offline", and "offline" is a statement about a
//! device, not an admission that the server did not say. A row that cannot
//! answer it must fail to decode so the client reports a server it cannot
//! describe.
//!
//! The **envelope** `devices` is required for the same reason, one level up.
//! `#[serde(default)]` there would let a reply that never said how many
//! devices exist decode as zero of them, and the page would print "No paired
//! devices." — a count, sourced from a server that gave none. That is exactly
//! the sibling outage above: the channel-pairing page reported "no approved
//! senders" because of a missing ENVELOPE, not a missing row field. Leniency
//! belongs on the columns a row may honestly lack, never on the answer itself.

use serde::{Deserialize, Serialize};

/// One paired remote Panel device, as a client renders it.
///
/// A projection of the security store's `DeviceRow` — deliberately NOT that
/// type, and deliberately not named `DeviceRow`: the store row carries
/// `public_key` / `fingerprint` / `role` / `scopes`, none of which belong on a
/// wire an operator reads. A field here with no renderer is the defect this
/// module exists to prevent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDeviceRow {
    /// The id `gateway.devices.revoke` takes. A device the caller cannot read
    /// back is a device they cannot revoke.
    pub device_id: String,

    /// Operator-visible device label, as the device named itself at pairing.
    pub device_name: String,

    /// Whether a live connection is bound to this device right now — a join
    /// against the presence roster, not a guess.
    ///
    /// Required, and the one field here that is. See the module doc: a default
    /// would let a missing key render as a claim of "offline".
    pub connected: bool,

    /// Epoch millis of the last handshake, or `None` for a device that has
    /// never come back since pairing.
    #[serde(default)]
    pub last_seen_at: Option<i64>,

    /// The principal this device speaks as.
    ///
    /// `None` for a legacy row that was never adopted; every live pairing path
    /// binds one. Until this column reached the wire, an operator offboarding
    /// one of five members saw five rows named "iPhone".
    #[serde(default)]
    pub user_id: Option<String>,

    /// Display name for [`Self::user_id`], resolved server-side through the
    /// same directory projection the room bubbles and the channel-pairing list
    /// use.
    ///
    /// `None` when the device is unbound *or* when the directory has no name
    /// for the principal. A client falls back to the raw `u-` id in both
    /// cases; the distinction is not one an operator can act on.
    #[serde(default)]
    pub display_name: Option<String>,
}

impl PairedDeviceRow {
    /// Whose device this is, as a row prints it: the resolved name when there
    /// is one, otherwise the raw principal id, otherwise nothing to say.
    ///
    /// Lives here rather than in the Panel because the fallback is a fact
    /// about the two fields above (see [`Self::display_name`]), and a fallback
    /// spelled at each call site is a fallback that drifts.
    #[must_use]
    pub fn owner_label(&self) -> Option<&str> {
        // An empty string is not a name, on either field: the fallback has to
        // see through it, or a directory row with a blank label prints as a
        // device with no owner at all.
        fn named(s: &Option<String>) -> Option<&str> {
            s.as_deref().filter(|s| !s.is_empty())
        }
        named(&self.display_name).or_else(|| named(&self.user_id))
    }
}

/// Response of `gateway.devices.list`.
///
/// The envelope is a wire key too — a correct row array under a key the client
/// does not walk is the same outage as a wrong row.
///
/// `devices` therefore carries **no** `#[serde(default)]`: a reply that omits
/// it must fail to decode rather than read as an empty inventory. `{"devices":
/// []}` is the honest empty and still decodes. See the module doc — the
/// attribute is not leniency here, it is a fabricated count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PairedDeviceList {
    /// The paired Panel devices, in store order.
    pub devices: Vec<PairedDeviceRow>,
}

impl PairedDeviceList {
    /// Build the response from the rows.
    #[must_use]
    pub fn new(devices: Vec<PairedDeviceRow>) -> Self {
        Self { devices }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(device_id: &str) -> PairedDeviceRow {
        PairedDeviceRow {
            device_id: device_id.to_string(),
            device_name: "iPhone".to_string(),
            connected: true,
            last_seen_at: Some(1_760_000_000_000),
            user_id: Some("u-bob".to_string()),
            display_name: Some("Bob".to_string()),
        }
    }

    /// The assertion that pins the contract in the direction parsing cannot:
    /// exactly these keys, no more. `created_at` used to be the "no more".
    #[test]
    fn a_row_serializes_exactly_the_declared_wire_keys() {
        let v = serde_json::to_value(row("panel-1")).unwrap();
        let mut keys: Vec<&str> = v
            .as_object()
            .expect("a row is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "connected",
                "device_id",
                "device_name",
                "display_name",
                "last_seen_at",
                "user_id",
            ]
        );
    }

    /// `devices` is the key every client walks, including the QA driver.
    #[test]
    fn the_response_carries_the_key_every_client_walks() {
        let v = serde_json::to_value(PairedDeviceList::new(vec![row("panel-1")])).unwrap();
        let devices = v
            .get("devices")
            .and_then(|d| d.as_array())
            .expect("`devices` is the envelope key");
        assert_eq!(devices[0]["device_id"], "panel-1");
        assert_eq!(devices[0]["user_id"], "u-bob");
    }

    /// A newer server may add a field; that must not blank an operator's
    /// inventory, which is the surface they revoke from.
    #[test]
    fn a_row_ignores_fields_it_does_not_render() {
        let parsed: PairedDeviceRow = serde_json::from_value(serde_json::json!({
            "device_id": "panel-1",
            "device_name": "iPhone",
            "connected": false,
            "internal_row_id": 7,
        }))
        .expect("extra server-side fields must not break a client");
        assert_eq!(parsed.device_id, "panel-1");
        assert!(parsed.last_seen_at.is_none());
        assert!(parsed.user_id.is_none());
    }

    /// The asymmetry, pinned: a row that does not say `connected` must fail to
    /// decode rather than decode as "offline".
    #[test]
    fn a_row_that_does_not_say_connected_fails_rather_than_reading_as_offline() {
        let err = serde_json::from_value::<PairedDeviceRow>(serde_json::json!({
            "device_id": "panel-1",
            "device_name": "iPhone",
        }))
        .expect_err("a missing `connected` is an unknown, never a claim of offline");
        assert!(
            err.to_string().contains("connected"),
            "the decode error must name the field: {err}"
        );
    }

    /// An unbound device must still serialize its principal key, so a client
    /// can tell "no owner" from "the server is too old to say".
    #[test]
    fn an_unbound_device_sends_an_explicit_null_principal() {
        let mut r = row("panel-1");
        r.user_id = None;
        r.display_name = None;
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.as_object().unwrap().contains_key("user_id"));
        assert!(v["user_id"].is_null());
        assert_eq!(r.owner_label(), None);
    }

    #[test]
    fn the_owner_label_prefers_the_resolved_name_and_falls_back_to_the_id() {
        let mut r = row("panel-1");
        assert_eq!(r.owner_label(), Some("Bob"));
        r.display_name = None;
        assert_eq!(r.owner_label(), Some("u-bob"));
        r.display_name = Some(String::new());
        assert_eq!(
            r.owner_label(),
            Some("u-bob"),
            "an empty resolved name is not a name; the doc promises the id"
        );
    }

    /// The envelope is a wire key, and it answers the same question `connected`
    /// does. A reply that never says how many devices there are must fail to
    /// decode rather than read as "none" — the sibling outage this module cites
    /// was a missing ENVELOPE, not a missing per-row field.
    #[test]
    fn a_response_without_the_envelope_key_fails_rather_than_reading_as_empty() {
        let err = serde_json::from_value::<PairedDeviceList>(serde_json::json!({}))
            .expect_err("a missing `devices` key is an unknown, never an empty inventory");
        assert!(
            err.to_string().contains("devices"),
            "the decode error must name the field: {err}"
        );
    }

    /// An explicit empty array is the honest empty, and still decodes.
    #[test]
    fn an_explicitly_empty_inventory_still_decodes() {
        let list: PairedDeviceList =
            serde_json::from_value(serde_json::json!({ "devices": [] })).unwrap();
        assert!(list.devices.is_empty());
    }
}
