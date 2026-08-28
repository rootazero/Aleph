//! Which channel conversation belongs to which project room.
//!
//! Keyed on the CONVERSATION — `(channel, peer_kind, peer_id)` — and not on the
//! session key, because a session key carries the agent id: an `agent_switch`
//! would mint a different key and silently un-bind the room while every
//! surface kept showing it as bound.
//!
//! `peer_kind` is part of the key because `SessionKey::Group` carries both
//! `PeerKind::Group` and `PeerKind::Thread`, whose `peer_id` namespaces are not
//! guaranteed disjoint.
//!
//! `peer_kind` is typed as [`BindingPeerKind`] on the wire and in
//! [`ChannelBinding`] (Ruling V) rather than a bare `String`: it is part of the
//! table's PRIMARY KEY, so `"Group"` and `"group"` would be two different rows
//! and a typo would never collide with the already-bound conflict guard. There
//! is exactly one conversion in each direction between the routing
//! [`PeerKind`] / the wire `BindingPeerKind` and the column's stored spelling —
//! [`to_wire`] / [`wire_str`] / [`parse_wire`] — so no third place ever writes
//! its own `match` on the string.
//!
//! `channel_id` and `peer_id` are normalized through [`normalize_component`]
//! before they ever reach the table (Ruling AD). A live `SessionKey` always
//! carries the normalized spelling — `SessionKey::group`/`SessionKey::dm` run
//! every component through `sanitize_component` at construction — so a
//! binding stored under an operator's raw, un-normalized spelling (mixed
//! case, punctuation) would sit in `project_channel_bindings`, appear in every
//! `projects.channel.list`, and never once match a lookup derived from a real
//! conversation. The operator's original spelling is not lost: it belongs in
//! [`ChannelBinding::label`], which exists for exactly this, so normalizing
//! the key components costs the user nothing in display.

use aleph_protocol::projects::BindingPeerKind;

use crate::routing::session_key::{sanitize_component, PeerKind, SessionKey};

/// One room ⟷ conversation binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelBinding {
    pub project_id: String,
    /// Normalized via [`normalize_component`] — not necessarily what the
    /// operator typed. See [`Self::label`] for the display spelling.
    pub channel_id: String,
    pub peer_kind: BindingPeerKind,
    /// Normalized via [`normalize_component`] — not necessarily what the
    /// operator typed. See [`Self::label`] for the display spelling.
    pub peer_id: String,
    /// The operator who bound it. `None` only for an unrestricted in-process
    /// caller; the RPC face always resolves one.
    pub bound_by: Option<String>,
    pub bound_at: i64,
    /// Human label for the conversation, as the operator named it. Purely for
    /// rendering — never used to address anything. This is where the
    /// operator's original, un-normalized spelling belongs.
    pub label: Option<String>,
}

/// Normalize a channel or peer id the same way a live [`SessionKey`] does.
///
/// The only `&str -> normalized String` conversion `ProjectStore`'s binding
/// methods use — calls [`sanitize_component`] directly rather than
/// reimplementing it, so the write path (an operator naming a conversation)
/// and the read path (a `SessionKey` derived from an inbound message via
/// [`conversation_of`]) cannot drift apart. See the module doc for why they
/// must not.
#[must_use]
pub fn normalize_component(s: &str) -> String {
    sanitize_component(s)
}

/// Convert a routing [`PeerKind`] to the wire [`BindingPeerKind`].
///
/// The only `PeerKind -> BindingPeerKind` conversion in the tree —
/// [`peer_kind_str`] and [`conversation_of`] both go through this rather than
/// each writing their own `match`.
#[must_use]
pub const fn to_wire(kind: PeerKind) -> BindingPeerKind {
    match kind {
        PeerKind::Group => BindingPeerKind::Group,
        PeerKind::Thread => BindingPeerKind::Thread,
    }
}

/// Stable storage spelling for a peer kind.
///
/// The only `BindingPeerKind -> &str` conversion in the tree: everything that
/// stores a binding row goes through this rather than writing its own `match`.
#[must_use]
pub const fn wire_str(kind: BindingPeerKind) -> &'static str {
    match kind {
        BindingPeerKind::Group => "group",
        BindingPeerKind::Thread => "thread",
    }
}

/// Parse a stored column value back into a [`BindingPeerKind`].
///
/// `None` for anything that is not exactly `"group"` or `"thread"` — the only
/// `&str -> BindingPeerKind` conversion in the tree. The only writer of this
/// column is [`wire_str`] above, so a `None` here means the row was written by
/// something else (or corrupted); the caller must say so out loud rather than
/// silently dropping the row — see `ProjectStore::bindings_for`.
#[must_use]
pub fn parse_wire(s: &str) -> Option<BindingPeerKind> {
    match s {
        "group" => Some(BindingPeerKind::Group),
        "thread" => Some(BindingPeerKind::Thread),
        _ => None,
    }
}

/// Stable storage spelling for a routing peer kind.
///
/// Derived from [`wire_str`] via [`to_wire`] rather than writing a second,
/// parallel `match` — so the column's contents cannot drift independently of
/// the wire type's own spelling.
#[must_use]
pub const fn peer_kind_str(kind: PeerKind) -> &'static str {
    wire_str(to_wire(kind))
}

/// The conversation a session key addresses, when it addresses one.
///
/// `None` for every other key shape — a DM, a task, a subagent and a main
/// session are not conversations a room can be bound to.
#[must_use]
pub fn conversation_of(key: &SessionKey) -> Option<(String, BindingPeerKind, String)> {
    match key {
        SessionKey::Group {
            channel,
            peer_kind,
            peer_id,
            ..
        } => Some((channel.clone(), to_wire(*peer_kind), peer_id.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_key_resolves_to_its_conversation() {
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C0A1");
        let (channel, kind, peer) = conversation_of(&key).expect("a group key is a conversation");
        assert_eq!(channel, "telegram");
        assert_eq!(kind, BindingPeerKind::Group);
        // SessionKey::group runs the peer id through the same normalization
        // every live session key gets (lowercased, non-alphanumerics folded to
        // '-'), so what conversation_of reports is "c0a1", not the "C0A1" the
        // constructor was called with. That normalization is exactly why
        // ProjectStore's bind/unbind/lookup methods must run the SAME
        // function over their inputs — see
        // `a_binding_written_with_an_operator_spelling_is_found_by_the_session_key_lookup`
        // in `store.rs` for the property this only half-tests.
        assert_eq!(peer, "c0a1");
    }

    #[test]
    fn the_agent_id_is_not_part_of_the_conversation() {
        let a = SessionKey::group("main", "telegram", PeerKind::Group, "C0A1");
        let b = SessionKey::group("coder", "telegram", PeerKind::Group, "C0A1");
        assert_eq!(
            conversation_of(&a),
            conversation_of(&b),
            "agent_switch must not un-bind a room: the binding is on the \
             conversation, not on the session key"
        );
    }

    #[test]
    fn a_dm_key_is_not_bindable() {
        let key = SessionKey::dm(
            "main",
            "telegram",
            "u123",
            crate::routing::session_key::DmScope::PerPeer,
        );
        assert!(
            conversation_of(&key).is_none(),
            "a DM has exactly one human on the far side; binding it to a room \
             would put a shared partition behind a private conversation"
        );
    }

    /// The routing enum and the wire enum must stay in step: a variant added to
    /// one without the other would mint conversations that can be bound and
    /// never matched (or matched and never bound). Written as an exhaustive
    /// match on the ROUTING enum so adding a variant there is a compile error,
    /// not a silently-passing test.
    #[test]
    fn the_routing_and_wire_peer_kinds_agree() {
        use aleph_protocol::projects::BindingPeerKind as Wire;
        for k in [PeerKind::Group, PeerKind::Thread] {
            let wire: Wire = match k {
                PeerKind::Group => Wire::Group,
                PeerKind::Thread => Wire::Thread,
            };
            assert_eq!(peer_kind_str(k), wire_str(wire));
        }
    }
}
