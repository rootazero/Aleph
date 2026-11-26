//! Resolve the channel delivery route `(ChannelId, ConversationId)` from a
//! structured `SessionKey`.
//!
//! Approval delivery must never fall back to scanning a string `session_key`.
//! A per-peer DM (`DmScope::PerPeer`, `agent:{a}:peer:{p}`) is one conversation
//! across channels, so its key carries no channel and yields no route here; the
//! gateway path routes it from the turn's own channel (`TURN_CONTEXT`).

use crate::gateway::channel::{ChannelId, ConversationId};
use crate::routing::session_key::SessionKey;
use tracing::warn;

/// Maximum nesting depth for subagent parent-key chains.
const MAX_SUBAGENT_DEPTH: usize = 16;

/// Resolve the channel and conversation where an approval notice should be
/// delivered.
///
/// `Main` / `Task` / `Ephemeral` have no channel source → `None` (callers use
/// this to explicitly Deny). `Subagent` recurses into its `parent_key`, bounded
/// by [`MAX_SUBAGENT_DEPTH`] to prevent stack overflow.
pub(crate) fn channel_route(key: &SessionKey) -> Option<(ChannelId, ConversationId)> {
    channel_route_inner(key, 0)
}

fn channel_route_inner(key: &SessionKey, depth: usize) -> Option<(ChannelId, ConversationId)> {
    if depth >= MAX_SUBAGENT_DEPTH {
        warn!(
            "Subagent nesting depth reached {MAX_SUBAGENT_DEPTH}, stopping channel route resolution"
        );
        return None;
    }
    match key {
        SessionKey::DirectMessage { channel, .. } if channel.is_empty() => None,
        SessionKey::DirectMessage {
            channel, peer_id, ..
        }
        | SessionKey::Group {
            channel, peer_id, ..
        } => Some((
            ChannelId::new(channel.clone()),
            ConversationId::new(peer_id.clone()),
        )),
        SessionKey::Subagent { parent_key, .. } => channel_route_inner(parent_key, depth + 1),
        SessionKey::Main { .. } | SessionKey::Task { .. } | SessionKey::Ephemeral { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::{DmScope, PeerKind, SessionKey};

    #[test]
    fn dm_per_peer_has_no_channel_to_route_to() {
        let key = SessionKey::dm("main", "telegram", "123456", DmScope::PerPeer);
        assert!(channel_route(&key).is_none());
    }

    #[test]
    fn dm_per_channel_peer_routes() {
        let key = SessionKey::dm("main", "telegram", "u1", DmScope::PerChannelPeer);
        assert!(channel_route(&key).is_some());
    }

    #[test]
    fn group_routes_to_channel() {
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "g1");
        let (ch, conv) = channel_route(&key).expect("Group must route");
        assert_eq!(ch.as_str(), "telegram");
        assert_eq!(conv.as_str(), "g1");
    }

    #[test]
    fn main_session_has_no_route() {
        assert!(channel_route(&SessionKey::main("main")).is_none());
    }

    #[test]
    fn ephemeral_session_has_no_route() {
        assert!(channel_route(&SessionKey::ephemeral("main")).is_none());
    }

    #[test]
    fn task_session_has_no_route() {
        assert!(channel_route(&SessionKey::task("main", "cron", "daily")).is_none());
    }

    #[test]
    fn subagent_recurses_to_parent() {
        let parent = SessionKey::dm("main", "telegram", "777", DmScope::PerChannelPeer);
        let key = SessionKey::Subagent {
            parent_key: Box::new(parent),
            subagent_id: "coding".to_string(),
        };
        let (ch, conv) = channel_route(&key).expect("Subagent must recurse");
        assert_eq!(ch.as_str(), "telegram");
        assert_eq!(conv.as_str(), "777");
    }

    #[test]
    fn subagent_of_main_has_no_route() {
        let key = SessionKey::Subagent {
            parent_key: Box::new(SessionKey::main("main")),
            subagent_id: "x".to_string(),
        };
        assert!(channel_route(&key).is_none());
    }
}
