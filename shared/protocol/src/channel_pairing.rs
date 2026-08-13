//! Channel pairing RPC contract — the `channel.pairing.approved` response.
//!
//! # Why this type lives here and not next to the handler
//!
//! `channel.pairing.approved` has one client, and it is in another crate: the
//! Panel's channel settings page (`ChannelPairingSection`). The shape was
//! written twice — once as a `serde_json::json!` literal in the handler, once
//! as a hand-rolled `val.get("…")` walk in the Panel — with nothing connecting
//! them.
//!
//! They disagreed, in the quietest way this class of bug has: the handler sent
//! `{"channel", "approved": ["<sender_id>", …], "count"}`, and the Panel read
//! `val["senders"]` — an array of objects under a key the server has never
//! emitted. `.and_then(|v| v.as_array())` on a missing key is `None`, the
//! `if let` simply did not fire, and the signal left behind was an empty list
//! rendering "no approved senders" on a channel that had several. Not an
//! error, not a refusal, not a red test: a page that looks like a fact.
//!
//! It also disagreed about the *content*. `approved_senders.user_id` — which
//! principal a sender speaks as — has been on the table since P0 and is what
//! `inbound_router::executor` stamps every inbound turn with. The projection
//! dropped it. SECURITY.md names `channel.pairing.revoke{channel, sender_id}`
//! as the way to cut a principal off a chat channel, so an operator has to know
//! which sender is that principal's: **enumerability is the prerequisite for
//! revocability**, and the column was being discarded one layer above the
//! surface that needed it.
//!
//! Sharing one type makes a rename a compile error on both sides, and building
//! the response *from* it (rather than parsing a response *into* it) makes
//! over-sending impossible rather than merely untested — the direction the
//! `workspace.*` contract had to learn the hard way, see [`crate::workspace`].

use serde::{Deserialize, Serialize};

/// One approved channel sender, as a client renders it.
///
/// A projection of the server's `approved_senders` row: exactly the fields
/// something prints. A field here with no renderer is the defect this module
/// exists to prevent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedSenderRow {
    /// Channel-native sender identity (phone number, Telegram id, …),
    /// normalized — and the key `channel.pairing.revoke` takes. A sender the
    /// caller cannot read back is a sender they cannot revoke.
    pub sender_id: String,

    /// The Aleph principal this sender speaks as.
    ///
    /// `None` means unlinked, which the inbound router reads as legacy owner
    /// semantics — so `None` and `Some(owner)` are *different* facts about how
    /// the row got there and must not be collapsed by a client that finds them
    /// equivalent today.
    #[serde(default)]
    pub user_id: Option<String>,

    /// Display name for [`Self::user_id`], resolved server-side through the
    /// same directory projection room bubbles use.
    ///
    /// `None` when the principal is unlinked *or* when the directory has no
    /// name for it. A client renders the id in both cases; it deliberately
    /// cannot tell them apart, because the distinction is not one an operator
    /// can act on.
    #[serde(default)]
    pub display_name: Option<String>,

    /// When the approval was granted, as stored (RFC 3339).
    ///
    /// Kept as an opaque `String` rather than a `DateTime<Utc>` on purpose: a
    /// parse failure on one legacy row would fail the whole response and blank
    /// the list — reproducing, through a stricter type, the exact symptom this
    /// module was written to remove. Nothing renders it as a structured date.
    pub approved_at: String,
}

/// Response of `channel.pairing.approved`.
///
/// Construct it with [`ApprovedSenderList::new`] rather than by struct
/// literal: `approved` and `count` are **projections** of `senders`, and a
/// projection with its own author is a projection that drifts. The constructor
/// is the only place they are derived.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedSenderList {
    /// The channel these senders are approved on.
    pub channel: String,

    /// The approved senders, newest approval first.
    pub senders: Vec<ApprovedSenderRow>,

    /// Sender ids only — the pre-2026-08-13 shape of this response.
    ///
    /// Retained because it is what any client written against the old response
    /// reads, and dropping it would break them to fix a Panel that was already
    /// broken. It is derived from [`Self::senders`] in [`Self::new`] and must
    /// never be assigned independently.
    pub approved: Vec<String>,

    /// `senders.len()`, derived in [`Self::new`].
    pub count: usize,
}

impl ApprovedSenderList {
    /// Build a response from the rows, deriving every projection of them.
    #[must_use]
    pub fn new(channel: impl Into<String>, senders: Vec<ApprovedSenderRow>) -> Self {
        let approved = senders.iter().map(|s| s.sender_id.clone()).collect();
        let count = senders.len();
        Self {
            channel: channel.into(),
            senders,
            approved,
            count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(sender_id: &str, user_id: Option<&str>) -> ApprovedSenderRow {
        ApprovedSenderRow {
            sender_id: sender_id.to_string(),
            user_id: user_id.map(str::to_string),
            display_name: None,
            approved_at: "2026-08-13T09:00:00+00:00".to_string(),
        }
    }

    /// The legacy array and the count are projections, so they can only ever
    /// disagree with `senders` if someone assigns them by hand. Pinning the
    /// derivation is what makes the compatibility key safe to keep.
    #[test]
    fn the_legacy_projection_is_derived_from_the_rows() {
        let list = ApprovedSenderList::new(
            "telegram",
            vec![row("tg-42", Some("u-bob")), row("tg-7", None)],
        );
        assert_eq!(list.approved, vec!["tg-42".to_string(), "tg-7".to_string()]);
        assert_eq!(list.count, 2);
    }

    /// The key the Panel reads must be `senders`. This is the assertion that
    /// would have failed on the day the two shapes diverged.
    #[test]
    fn the_response_carries_the_key_the_panel_reads() {
        let v = serde_json::to_value(ApprovedSenderList::new(
            "telegram",
            vec![row("tg-42", None)],
        ))
        .unwrap();
        let senders = v
            .get("senders")
            .and_then(|s| s.as_array())
            .expect("`senders` is the key the Panel walks");
        assert_eq!(senders[0]["sender_id"], "tg-42");
        assert!(
            senders[0].get("approved_at").is_some(),
            "the Panel renders approved_at from each row"
        );
    }

    /// An unlinked sender must still serialize its key, so a client can tell
    /// "no principal" from "the server is too old to say".
    #[test]
    fn an_unlinked_sender_sends_an_explicit_null_principal() {
        let v = serde_json::to_value(ApprovedSenderList::new("telegram", vec![row("tg-7", None)]))
            .unwrap();
        assert!(v["senders"][0]
            .as_object()
            .expect("row is an object")
            .contains_key("user_id"));
        assert!(v["senders"][0]["user_id"].is_null());
    }

    #[test]
    fn a_row_ignores_fields_it_does_not_render() {
        let parsed: ApprovedSenderRow = serde_json::from_value(serde_json::json!({
            "sender_id": "tg-42",
            "approved_at": "2026-08-13T09:00:00+00:00",
            "internal_row_id": 7,
        }))
        .expect("extra server-side fields must not break a client");
        assert_eq!(parsed.sender_id, "tg-42");
        assert!(parsed.user_id.is_none());
    }
}
