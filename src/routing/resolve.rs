//! Hierarchical route resolution.
//!
//! Resolves incoming requests to agents using binding match priority:
//! peer → guild → team → account → channel → default.

use std::collections::HashMap;

use super::config::{MatchRule, RouteBinding, SessionConfig};
use super::identity_links::resolve_linked_peer_id;
use super::session_key::{normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_MAIN_KEY};

/// Input for route resolution
#[derive(Debug, Clone)]
pub struct RouteInput {
    pub channel: String,
    pub account_id: Option<String>,
    pub peer: Option<RoutePeer>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
}

/// Peer information for routing
#[derive(Debug, Clone)]
pub struct RoutePeer {
    pub kind: RoutePeerKind,
    pub id: String,
}

/// Peer kind for routing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutePeerKind {
    Dm,
    Group,
    Channel,
}

/// Resolved route result
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub agent_id: String,
    pub channel: String,
    pub account_id: String,
    pub session_key: SessionKey,
    pub main_session_key: SessionKey,
    pub matched_by: MatchedBy,
    /// Workspace from route binding (if set). When present, the execution engine
    /// uses this workspace instead of the user's active workspace.
    pub workspace: Option<String>,
}

/// How the route was matched
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchedBy {
    Peer,
    Guild,
    Team,
    Account,
    Channel,
    Default,
}

/// Resolve an agent route from input
#[must_use]
pub fn resolve_route(
    bindings: &[RouteBinding],
    session_cfg: &SessionConfig,
    default_agent: &str,
    input: &RouteInput,
) -> ResolvedRoute {
    let channel = input.channel.trim().to_lowercase();
    let account_id = input
        .account_id
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("default");

    // Filter bindings matching channel and account
    let candidates: Vec<&RouteBinding> = bindings
        .iter()
        .filter(|b| matches_channel(&b.match_rule, &channel))
        .filter(|b| matches_account(&b.match_rule, account_id))
        .collect();

    // Select the highest-priority matching binding, if any.
    let mut matched: Option<(&RouteBinding, MatchedBy)> = None;

    if let Some(peer) = &input.peer {
        matched = candidates
            .iter()
            .find(|b| matches_peer(&b.match_rule, peer))
            .map(|&b| (b, MatchedBy::Peer));
    }

    if matched.is_none() {
        if let Some(guild_id) = &input.guild_id {
            matched = candidates
                .iter()
                .find(|b| matches_guild(&b.match_rule, guild_id))
                .map(|&b| (b, MatchedBy::Guild));
        }
    }

    if matched.is_none() {
        if let Some(team_id) = &input.team_id {
            matched = candidates
                .iter()
                .find(|b| matches_team(&b.match_rule, team_id))
                .map(|&b| (b, MatchedBy::Team));
        }
    }

    if matched.is_none() {
        matched = candidates
            .iter()
            .find(|b| {
                b.match_rule.account_id.as_ref().is_some_and(|a| a != "*")
                    && b.match_rule.peer.is_none()
                    && b.match_rule.guild_id.is_none()
                    && b.match_rule.team_id.is_none()
            })
            .map(|&b| (b, MatchedBy::Account));
    }

    if matched.is_none() {
        matched = candidates
            .iter()
            .find(|b| {
                b.match_rule.account_id.as_ref().is_none_or(|a| a == "*")
                    && b.match_rule.peer.is_none()
                    && b.match_rule.guild_id.is_none()
                    && b.match_rule.team_id.is_none()
            })
            .map(|&b| (b, MatchedBy::Channel));
    }

    let build =
        |agent_id: &str, matched_by: MatchedBy, workspace: Option<String>| -> ResolvedRoute {
            let agent_id = normalize_agent_id(agent_id);
            let session_key = build_session_key(
                &agent_id,
                &channel,
                input.peer.as_ref(),
                session_cfg.dm_scope,
                &session_cfg.identity_links,
            );
            ResolvedRoute {
                agent_id: agent_id.clone(),
                channel,
                account_id: account_id.to_string(),
                session_key,
                main_session_key: SessionKey::Main {
                    agent_id,
                    main_key: DEFAULT_MAIN_KEY.to_string(),
                    epoch: 0,
                },
                matched_by,
                workspace,
            }
        };

    match matched {
        Some((b, matched_by)) => build(&b.agent_id, matched_by, b.match_rule.workspace.clone()),
        None => build(default_agent, MatchedBy::Default, None),
    }
}

fn build_session_key(
    agent_id: &str,
    channel: &str,
    peer: Option<&RoutePeer>,
    dm_scope: DmScope,
    identity_links: &HashMap<String, Vec<String>>,
) -> SessionKey {
    let Some(peer) = peer else {
        return SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: DEFAULT_MAIN_KEY.to_string(),
            epoch: 0,
        };
    };

    match peer.kind {
        RoutePeerKind::Dm => {
            let linked = resolve_linked_peer_id(identity_links, channel, &peer.id);
            let peer_id = linked.as_deref().unwrap_or(peer.id.as_str());

            SessionKey::dm(agent_id, channel, peer_id, dm_scope)
        }
        RoutePeerKind::Group => SessionKey::group(agent_id, channel, PeerKind::Group, &peer.id),
        RoutePeerKind::Channel => SessionKey::group(agent_id, channel, PeerKind::Channel, &peer.id),
    }
}

fn matches_channel(rule: &MatchRule, channel: &str) -> bool {
    rule.channel
        .as_ref()
        .is_some_and(|c| c.eq_ignore_ascii_case(channel))
}

fn matches_account(rule: &MatchRule, account_id: &str) -> bool {
    match &rule.account_id {
        None => account_id == "default",
        Some(a) if a == "*" => true,
        Some(a) => a == account_id,
    }
}

fn matches_peer(rule: &MatchRule, peer: &RoutePeer) -> bool {
    rule.peer.as_ref().is_some_and(|p| {
        let kind_matches = match peer.kind {
            RoutePeerKind::Dm => p.kind.eq_ignore_ascii_case("dm"),
            RoutePeerKind::Group => p.kind.eq_ignore_ascii_case("group"),
            RoutePeerKind::Channel => p.kind.eq_ignore_ascii_case("channel"),
        };
        kind_matches && p.id.eq_ignore_ascii_case(&peer.id)
    })
}

fn matches_guild(rule: &MatchRule, guild_id: &str) -> bool {
    rule.guild_id
        .as_ref()
        .is_some_and(|g| g.eq_ignore_ascii_case(guild_id))
}

fn matches_team(rule: &MatchRule, team_id: &str) -> bool {
    rule.team_id
        .as_ref()
        .is_some_and(|t| t.eq_ignore_ascii_case(team_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::config::PeerMatchConfig;

    fn default_session_cfg() -> SessionConfig {
        SessionConfig::default()
    }

    fn telegram_binding(agent_id: &str) -> RouteBinding {
        RouteBinding {
            agent_id: agent_id.to_string(),
            match_rule: MatchRule {
                channel: Some("telegram".to_string()),
                account_id: Some("*".to_string()),
                ..Default::default()
            },
        }
    }

    fn slack_team_binding(agent_id: &str, team_id: &str) -> RouteBinding {
        RouteBinding {
            agent_id: agent_id.to_string(),
            match_rule: MatchRule {
                channel: Some("slack".to_string()),
                account_id: Some("*".to_string()),
                team_id: Some(team_id.to_string()),
                ..Default::default()
            },
        }
    }

    fn peer_binding(agent_id: &str, channel: &str, peer_kind: &str, peer_id: &str) -> RouteBinding {
        RouteBinding {
            agent_id: agent_id.to_string(),
            match_rule: MatchRule {
                channel: Some(channel.to_string()),
                account_id: Some("*".to_string()),
                peer: Some(PeerMatchConfig {
                    kind: peer_kind.to_string(),
                    id: peer_id.to_string(),
                }),
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_default_route() {
        let route = resolve_route(
            &[],
            &default_session_cfg(),
            "main",
            &RouteInput {
                channel: "telegram".to_string(),
                account_id: None,
                peer: None,
                guild_id: None,
                team_id: None,
            },
        );
        assert_eq!(route.agent_id, "main");
        assert_eq!(route.matched_by, MatchedBy::Default);
    }

    #[test]
    fn test_channel_match() {
        let bindings = vec![telegram_binding("telegram-agent")];
        let route = resolve_route(
            &bindings,
            &default_session_cfg(),
            "main",
            &RouteInput {
                channel: "telegram".to_string(),
                account_id: None,
                peer: None,
                guild_id: None,
                team_id: None,
            },
        );
        assert_eq!(route.agent_id, "telegram-agent");
        assert_eq!(route.matched_by, MatchedBy::Channel);
    }

    #[test]
    fn test_team_match_higher_than_channel() {
        let bindings = vec![
            telegram_binding("generic"),
            slack_team_binding("work", "T12345"),
        ];
        let route = resolve_route(
            &bindings,
            &default_session_cfg(),
            "main",
            &RouteInput {
                channel: "slack".to_string(),
                account_id: None,
                peer: None,
                guild_id: None,
                team_id: Some("T12345".to_string()),
            },
        );
        assert_eq!(route.agent_id, "work");
        assert_eq!(route.matched_by, MatchedBy::Team);
    }

    #[test]
    fn test_peer_match_highest_priority() {
        let bindings = vec![
            telegram_binding("generic"),
            peer_binding("vip-agent", "telegram", "dm", "user-vip"),
        ];
        let route = resolve_route(
            &bindings,
            &default_session_cfg(),
            "main",
            &RouteInput {
                channel: "telegram".to_string(),
                account_id: None,
                peer: Some(RoutePeer {
                    kind: RoutePeerKind::Dm,
                    id: "user-vip".to_string(),
                }),
                guild_id: None,
                team_id: None,
            },
        );
        assert_eq!(route.agent_id, "vip-agent");
        assert_eq!(route.matched_by, MatchedBy::Peer);
    }

    #[test]
    fn test_dm_scope_per_peer() {
        let route = resolve_route(
            &[],
            &default_session_cfg(),
            "main",
            &RouteInput {
                channel: "telegram".to_string(),
                account_id: None,
                peer: Some(RoutePeer {
                    kind: RoutePeerKind::Dm,
                    id: "user123".to_string(),
                }),
                guild_id: None,
                team_id: None,
            },
        );
        assert_eq!(route.session_key.to_key_string(), "agent:main:dm:user123");
    }

    #[test]
    fn test_dm_scope_per_channel_peer() {
        let cfg = SessionConfig {
            dm_scope: DmScope::PerChannelPeer,
            ..Default::default()
        };
        let route = resolve_route(
            &[],
            &cfg,
            "main",
            &RouteInput {
                channel: "telegram".to_string(),
                account_id: None,
                peer: Some(RoutePeer {
                    kind: RoutePeerKind::Dm,
                    id: "user123".to_string(),
                }),
                guild_id: None,
                team_id: None,
            },
        );
        assert_eq!(
            route.session_key.to_key_string(),
            "agent:main:telegram:dm:user123"
        );
    }

    #[test]
    fn test_dm_scope_main_collapses() {
        let cfg = SessionConfig {
            dm_scope: DmScope::Main,
            ..Default::default()
        };
        let route = resolve_route(
            &[],
            &cfg,
            "main",
            &RouteInput {
                channel: "telegram".to_string(),
                account_id: None,
                peer: Some(RoutePeer {
                    kind: RoutePeerKind::Dm,
                    id: "user123".to_string(),
                }),
                guild_id: None,
                team_id: None,
            },
        );
        assert_eq!(route.session_key.to_key_string(), "agent:main:main");
    }

    #[test]
    fn test_identity_links() {
        let mut links = HashMap::new();
        links.insert(
            "john".to_string(),
            vec!["telegram:123".to_string(), "discord:456".to_string()],
        );
        let cfg = SessionConfig {
            dm_scope: DmScope::PerPeer,
            identity_links: links,
        };
        let route = resolve_route(
            &[],
            &cfg,
            "main",
            &RouteInput {
                channel: "telegram".to_string(),
                account_id: None,
                peer: Some(RoutePeer {
                    kind: RoutePeerKind::Dm,
                    id: "123".to_string(),
                }),
                guild_id: None,
                team_id: None,
            },
        );
        // Should resolve to canonical "john" instead of "123"
        assert_eq!(route.session_key.to_key_string(), "agent:main:dm:john");
    }

    #[test]
    fn test_workspace_from_route_binding() {
        let bindings = vec![RouteBinding {
            agent_id: "main".to_string(),
            match_rule: MatchRule {
                channel: Some("telegram".to_string()),
                account_id: Some("*".to_string()),
                workspace: Some("crypto".to_string()),
                ..Default::default()
            },
        }];
        let route = resolve_route(
            &bindings,
            &default_session_cfg(),
            "main",
            &RouteInput {
                channel: "telegram".to_string(),
                account_id: None,
                peer: None,
                guild_id: None,
                team_id: None,
            },
        );
        assert_eq!(route.workspace.as_deref(), Some("crypto"));
        assert_eq!(route.matched_by, MatchedBy::Channel);
    }

    #[test]
    fn test_default_route_no_workspace() {
        let route = resolve_route(
            &[],
            &default_session_cfg(),
            "main",
            &RouteInput {
                channel: "telegram".to_string(),
                account_id: None,
                peer: None,
                guild_id: None,
                team_id: None,
            },
        );
        assert!(route.workspace.is_none());
    }

    #[test]
    fn test_channel_binding_without_account_id_matches_default() {
        // A binding that omits account_id should still route the default
        // account on that channel (not silently fall through to the default
        // agent).
        let bindings = vec![RouteBinding {
            agent_id: "telegram-agent".to_string(),
            match_rule: MatchRule {
                channel: Some("telegram".to_string()),
                account_id: None,
                ..Default::default()
            },
        }];
        let route = resolve_route(
            &bindings,
            &default_session_cfg(),
            "main",
            &RouteInput {
                channel: "telegram".to_string(),
                account_id: None,
                peer: None,
                guild_id: None,
                team_id: None,
            },
        );
        assert_eq!(route.agent_id, "telegram-agent");
        assert_eq!(route.matched_by, MatchedBy::Channel);
    }

    #[test]
    fn test_group_session_key() {
        let route = resolve_route(
            &[],
            &default_session_cfg(),
            "main",
            &RouteInput {
                channel: "discord".to_string(),
                account_id: None,
                peer: Some(RoutePeer {
                    kind: RoutePeerKind::Group,
                    id: "guild456".to_string(),
                }),
                guild_id: None,
                team_id: None,
            },
        );
        assert_eq!(
            route.session_key.to_key_string(),
            "agent:main:discord:group:guild456"
        );
    }
}
