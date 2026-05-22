//! 从结构化 `SessionKey` 解出通道投递路由 `(ChannelId, ConversationId)`。
//!
//! 替代 `ChannelApprovalBridge::parse_session_key` 对字符串 session_key 的
//! 有损扫描 —— 后者对默认 `DmScope::PerPeer`（`agent:{a}:dm:{p}`，不含通道名）
//! 静默返回 `None`。结构化 `SessionKey` 直接携带 `channel` 字段，无歧义。

use crate::gateway::channel::{ChannelId, ConversationId};
use crate::routing::session_key::SessionKey;
use tracing::warn;

/// Maximum nesting depth for subagent parent-key chains.
const MAX_SUBAGENT_DEPTH: usize = 16;

/// 解出审批提示应投递到的通道与会话。
///
/// `Main` / `Task` / `Ephemeral` 无通道来源 → `None`（调用方据此明确 Denied）。
/// `Subagent` 递归其 `parent_key`，深度受 [`MAX_SUBAGENT_DEPTH`] 限制以防止栈溢出。
pub(crate) fn channel_route(key: &SessionKey) -> Option<(ChannelId, ConversationId)> {
    channel_route_inner(key, 0)
}

fn channel_route_inner(key: &SessionKey, depth: usize) -> Option<(ChannelId, ConversationId)> {
    if depth > MAX_SUBAGENT_DEPTH {
        warn!(
            "Subagent nesting depth exceeded {MAX_SUBAGENT_DEPTH}, stopping channel route resolution"
        );
        return None;
    }
    match key {
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
    fn dm_per_peer_routes_to_channel() {
        let key = SessionKey::dm("main", "telegram", "123456", DmScope::PerPeer);
        let (ch, conv) = channel_route(&key).expect("DM must route");
        assert_eq!(ch.as_str(), "telegram");
        assert_eq!(conv.as_str(), "123456");
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
        let parent = SessionKey::dm("main", "telegram", "777", DmScope::PerPeer);
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
