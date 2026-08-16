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
}

/// Resolved route result
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub agent_id: String,
    pub channel: String,
    pub session_key: SessionKey,
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

    // Candidates: channel and account gate first, then every *other* scope the
    // rule declares must also hold. Filtering on the full conjunction here (not
    // per-tier below) is what makes `{ team_id, peer }` mean "this peer in that
    // team" rather than "this peer anywhere, or that team anywhere".
    let candidates: Vec<&RouteBinding> = bindings
        .iter()
        .filter(|b| matches_channel(&b.match_rule, &channel))
        .filter(|b| matches_account(&b.match_rule, account_id))
        .filter(|b| scope_satisfied(&b.match_rule, input))
        .collect();

    // Select the highest-priority matching binding, if any. The tiers now only
    // decide *specificity* — every candidate already satisfies its whole rule.
    let mut matched: Option<(&RouteBinding, MatchedBy)> = None;

    if input.peer.is_some() {
        matched = candidates
            .iter()
            .find(|b| b.match_rule.peer.is_some())
            .map(|&b| (b, MatchedBy::Peer));
    }

    if matched.is_none() && input.guild_id.is_some() {
        matched = candidates
            .iter()
            .find(|b| b.match_rule.guild_id.is_some())
            .map(|&b| (b, MatchedBy::Guild));
    }

    if matched.is_none() && input.team_id.is_some() {
        matched = candidates
            .iter()
            .find(|b| b.match_rule.team_id.is_some())
            .map(|&b| (b, MatchedBy::Team));
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

    // The agent id is reported *as configured*. Normalisation belongs to the
    // session-key namespace (where it keeps keys filesystem- and wire-safe), not
    // to agent identity: the registry stores config-declared ids verbatim, so
    // normalising here turned a perfectly legal `[[agents.list]] id = "Work_Bot"`
    // into a lookup for `work_bot`, which the existence gate reported as a
    // deleted agent — with a message telling the operator to fix a `[[bindings]]`
    // entry whose spelling already matched the agent exactly. `session_keys_for`
    // still normalises internally, so keys are unchanged.
    let build =
        |agent_id: &str, matched_by: MatchedBy, workspace: Option<String>| -> ResolvedRoute {
            let trimmed = agent_id.trim();
            // An empty id still falls back to the default agent rather than routing
            // to "".
            let agent_id = if trimmed.is_empty() {
                normalize_agent_id(trimmed)
            } else {
                trimmed.to_string()
            };
            let (session_key, _) =
                session_keys_for(&agent_id, &channel, input.peer.as_ref(), session_cfg);
            ResolvedRoute {
                agent_id,
                channel,
                session_key,
                matched_by,
                workspace,
            }
        };

    match matched {
        Some((b, matched_by)) if !b.agent_id.trim().is_empty() => {
            build(&b.agent_id, matched_by, b.match_rule.workspace.clone())
        }
        // A binding whose `agent_id` is empty/whitespace matches nothing it can
        // name: fall through to the unmatched default path (`MatchedBy::Default`)
        // instead of silently resolving to the default agent as if the operator
        // had deliberately written `agent_id = "main"`.
        _ => build(default_agent, MatchedBy::Default, None),
    }
}

/// The `(conversation, main)` session-key pair an agent would use for this
/// channel/peer.
///
/// Split out of [`resolve_route`] so a caller that *changes* the agent after
/// resolution — the runtime overlay in
/// [`overlay`](crate::routing::overlay), where an `agent_switch` binding or a
/// dropped ghost binding redirects the message — can recompute the keys for the
/// agent that actually serves it, from the same code. Handing back the
/// config-resolved agent's key would address a different conversation.
///
/// `agent_id` is normalised here, so callers may pass a raw id.
#[must_use]
pub fn session_keys_for(
    agent_id: &str,
    channel: &str,
    peer: Option<&RoutePeer>,
    session_cfg: &SessionConfig,
) -> (SessionKey, SessionKey) {
    let agent_id = normalize_agent_id(agent_id);
    let session_key = build_session_key(
        &agent_id,
        channel,
        peer,
        session_cfg.dm_scope,
        &session_cfg.identity_links,
    );
    let main_session_key = SessionKey::Main {
        agent_id,
        main_key: DEFAULT_MAIN_KEY.to_string(),
        epoch: 0,
    };
    (session_key, main_session_key)
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
    }
}

fn matches_channel(rule: &MatchRule, channel: &str) -> bool {
    rule.channel
        .as_deref()
        .map(str::trim)
        .is_none_or(|c| c == "*" || c.eq_ignore_ascii_case(channel))
}

fn matches_account(rule: &MatchRule, account_id: &str) -> bool {
    match rule.account_id.as_deref().map(str::trim) {
        None => account_id == "default",
        Some("*") => true,
        Some(a) => a.eq_ignore_ascii_case(account_id),
    }
}

fn matches_peer(rule: &MatchRule, peer: &RoutePeer) -> bool {
    rule.peer.as_ref().is_some_and(|p| {
        let kind_matches = match peer.kind {
            RoutePeerKind::Dm => p.kind.trim().eq_ignore_ascii_case("dm"),
            RoutePeerKind::Group => p.kind.trim().eq_ignore_ascii_case("group"),
        };
        kind_matches && p.id.trim().eq_ignore_ascii_case(&peer.id)
    })
}

fn matches_guild(rule: &MatchRule, guild_id: &str) -> bool {
    rule.guild_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|g| g.eq_ignore_ascii_case(guild_id))
}

fn matches_team(rule: &MatchRule, team_id: &str) -> bool {
    rule.team_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|t| t.eq_ignore_ascii_case(team_id))
}

/// Whether every *scope* field the rule declares is satisfied by this input.
///
/// A `MatchRule` reads as a conjunction — `{ channel, team_id, peer }` means
/// "this peer, in that team, on that channel" — but the tier walk only ever
/// tested the one field belonging to the tier it was evaluating, so the others
/// were silently inert. A rule scoped to a Slack workspace fired for the same
/// conversation id in a *different* workspace, and Slack/Discord ids are not
/// unique across the installations a multi-workspace deployment bridges.
///
/// Only the scope fields are checked here (`guild_id` / `team_id` / `peer`);
/// channel and account are already filtered upstream. A field the rule omits
/// imposes no constraint — omission is the wildcard.
fn scope_satisfied(rule: &MatchRule, input: &RouteInput) -> bool {
    if rule.guild_id.is_some()
        && !input
            .guild_id
            .as_deref()
            .is_some_and(|g| matches_guild(rule, g))
    {
        return false;
    }
    if rule.team_id.is_some()
        && !input
            .team_id
            .as_deref()
            .is_some_and(|t| matches_team(rule, t))
    {
        return false;
    }
    if rule.peer.is_some() && !input.peer.as_ref().is_some_and(|p| matches_peer(rule, p)) {
        return false;
    }
    true
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

    /// A binding whose rule is a *conjunction* of a team and a group peer —
    /// "this channel, in that workspace".
    fn scoped_peer_binding(agent_id: &str, team_id: &str, peer_id: &str) -> RouteBinding {
        RouteBinding {
            agent_id: agent_id.to_string(),
            match_rule: MatchRule {
                channel: Some("slack".to_string()),
                account_id: Some("*".to_string()),
                team_id: Some(team_id.to_string()),
                peer: Some(PeerMatchConfig {
                    kind: "group".to_string(),
                    id: peer_id.to_string(),
                }),
                ..Default::default()
            },
        }
    }

    fn slack_input(team_id: Option<&str>, peer_id: &str) -> RouteInput {
        RouteInput {
            channel: "slack".to_string(),
            account_id: None,
            peer: Some(RoutePeer {
                kind: RoutePeerKind::Group,
                id: peer_id.to_string(),
            }),
            guild_id: None,
            team_id: team_id.map(str::to_string),
        }
    }

    #[test]
    fn scope_fields_are_conjunctive_not_alternative() {
        // The rule says "group C0A1 **in team T_ACME**". A message from the same
        // group id in a different workspace must NOT match: conversation ids are
        // not unique across the installations a multi-workspace deployment
        // bridges, and the tier walk used to test only the peer field.
        let bindings = vec![scoped_peer_binding("vip", "T_ACME", "C0A1")];
        let hit = resolve_route(
            &bindings,
            &default_session_cfg(),
            "main",
            &slack_input(Some("T_ACME"), "C0A1"),
        );
        assert_eq!(hit.agent_id, "vip");
        assert_eq!(hit.matched_by, MatchedBy::Peer);

        let other_workspace = resolve_route(
            &bindings,
            &default_session_cfg(),
            "main",
            &slack_input(Some("T_OTHER"), "C0A1"),
        );
        assert_eq!(other_workspace.agent_id, "main");
        assert_eq!(other_workspace.matched_by, MatchedBy::Default);

        // A message carrying no team at all cannot satisfy a team-scoped rule.
        let no_team = resolve_route(
            &bindings,
            &default_session_cfg(),
            "main",
            &slack_input(None, "C0A1"),
        );
        assert_eq!(no_team.matched_by, MatchedBy::Default);
    }

    #[test]
    fn omitted_channel_matches_any_channel() {
        // "this person always gets the vip agent, wherever they write" — the
        // natural reading of an omitted `channel`, which used to filter the
        // binding out before any tier ran.
        let bindings = vec![RouteBinding {
            agent_id: "vip".to_string(),
            match_rule: MatchRule {
                peer: Some(PeerMatchConfig {
                    kind: "dm".to_string(),
                    id: "user-vip".to_string(),
                }),
                ..Default::default()
            },
        }];
        for channel in ["telegram", "discord"] {
            let route = resolve_route(
                &bindings,
                &default_session_cfg(),
                "main",
                &RouteInput {
                    channel: channel.to_string(),
                    account_id: None,
                    peer: Some(RoutePeer {
                        kind: RoutePeerKind::Dm,
                        id: "user-vip".to_string(),
                    }),
                    guild_id: None,
                    team_id: None,
                },
            );
            assert_eq!(route.agent_id, "vip", "channel {channel}");
            assert_eq!(route.matched_by, MatchedBy::Peer);
        }
    }

    #[test]
    fn channel_wildcard_matches_any_channel() {
        let bindings = vec![RouteBinding {
            agent_id: "everywhere".to_string(),
            match_rule: MatchRule {
                channel: Some("*".to_string()),
                ..Default::default()
            },
        }];
        let route = resolve_route(
            &bindings,
            &default_session_cfg(),
            "main",
            &RouteInput {
                channel: "discord".to_string(),
                account_id: None,
                peer: None,
                guild_id: None,
                team_id: None,
            },
        );
        assert_eq!(route.agent_id, "everywhere");
        assert_eq!(route.matched_by, MatchedBy::Channel);
    }

    #[test]
    fn account_match_is_case_and_whitespace_insensitive() {
        // Every other matcher is; this one used to be an untrimmed `==`, so the
        // first operator to wire multi-account support would have inherited a
        // silent mismatch.
        let bindings = vec![RouteBinding {
            agent_id: "acme".to_string(),
            match_rule: MatchRule {
                channel: Some("slack".to_string()),
                account_id: Some(" ACME ".to_string()),
                ..Default::default()
            },
        }];
        let route = resolve_route(
            &bindings,
            &default_session_cfg(),
            "main",
            &RouteInput {
                channel: "slack".to_string(),
                account_id: Some("acme".to_string()),
                peer: None,
                guild_id: None,
                team_id: None,
            },
        );
        assert_eq!(route.agent_id, "acme");
        assert_eq!(route.matched_by, MatchedBy::Account);
    }

    #[test]
    fn agent_id_is_reported_as_configured_while_the_key_stays_normalised() {
        // `[[agents.list]] id = "Work_Bot"` is registered verbatim, so a route
        // that normalised the id resolved to an agent nobody had — and the
        // existence gate reported it as deleted while the two strings in the
        // file matched exactly. The session key keeps its normalised namespace.
        let bindings = vec![RouteBinding {
            agent_id: "Work_Bot".to_string(),
            match_rule: MatchRule {
                channel: Some("telegram".to_string()),
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
        assert_eq!(route.agent_id, "Work_Bot");
        assert!(
            route.session_key.to_key_string().contains("work_bot"),
            "session key should stay normalised, got {}",
            route.session_key.to_key_string()
        );
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
