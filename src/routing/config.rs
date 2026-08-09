//! Configuration structures for routing.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::identity_links::validate_identity_links;
use super::session_key::DmScope;

fn deserialize_identity_links_with_validation<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let links = HashMap::<String, Vec<String>>::deserialize(deserializer)?;
    validate_identity_links(&links).map_err(serde::de::Error::custom)?;
    Ok(links)
}

/// Session configuration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionConfig {
    /// DM isolation strategy
    #[serde(default)]
    pub dm_scope: DmScope,

    /// Cross-channel identity links: `canonical_name` -> [channel:id, ...]
    ///
    /// Note: this only takes effect on the configured-bindings routing path
    /// (`resolve_route` / `[[bindings]]`). The zero-config fallback
    /// (`resolve_session_key_with_agent`) does not consult it — a deployment
    /// relying on identity links must configure bindings.
    #[serde(
        default,
        deserialize_with = "deserialize_identity_links_with_validation"
    )]
    pub identity_links: HashMap<String, Vec<String>>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            dm_scope: DmScope::PerPeer,
            identity_links: HashMap::new(),
        }
    }
}

/// Route binding configuration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RouteBinding {
    pub agent_id: String,
    #[serde(rename = "match")]
    pub match_rule: MatchRule,
}

/// Match rule for route binding
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct MatchRule {
    /// Channel to match (telegram, discord, slack, ...)
    pub channel: Option<String>,
    /// API account ID (supports "*" wildcard)
    pub account_id: Option<String>,
    /// Peer match (specific user/group)
    pub peer: Option<PeerMatchConfig>,
    /// Discord guild ID
    pub guild_id: Option<String>,
    /// Slack team ID
    pub team_id: Option<String>,
    /// Workspace to auto-route to when this binding matches. When set, the
    /// execution engine uses this workspace instead of the user's active
    /// workspace. Must be an **absolute directory path**; a relative path or
    /// missing dir is ignored (falls back to the channel/agent default) with
    /// a startup warning.
    pub workspace: Option<String>,
}

/// Peer match configuration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PeerMatchConfig {
    pub kind: String,
    pub id: String,
}

/// Problems that make a binding unable to route anything, in operator terms.
///
/// A misconfigured `[[bindings]]` entry has exactly one symptom today — "my
/// routing config does nothing" — because a rule that can never match is
/// indistinguishable at match time from a rule that simply did not apply. These
/// checks turn the three ways to write an unmatchable binding into a startup
/// log line naming the entry and the fix.
///
/// Returns one message per problem, in binding order. Empty = nothing to say.
/// Deliberately *reporting* rather than rejecting: a bad binding must not stop
/// the daemon from booting, and the rest of the table still routes.
#[must_use]
pub fn binding_problems(bindings: &[RouteBinding]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, b) in bindings.iter().enumerate() {
        let who = if b.agent_id.trim().is_empty() {
            format!("[[bindings]] #{i}")
        } else {
            format!("[[bindings]] #{i} (agent_id = \"{}\")", b.agent_id)
        };
        if b.agent_id.trim().is_empty() {
            out.push(format!(
                "{who}: agent_id is empty — this binding routes to the default agent, \
                 which is what happens with no binding at all"
            ));
        }
        let r = &b.match_rule;
        if let Some(peer) = &r.peer {
            let kind = peer.kind.trim();
            if !["dm", "group"].contains(&kind.to_ascii_lowercase().as_str()) {
                out.push(format!(
                    "{who}: match.peer.kind = \"{}\" is not one of dm|group — \
                     this binding can never match",
                    peer.kind
                ));
            }
            if peer.id.trim().is_empty() {
                out.push(format!(
                    "{who}: match.peer.id is empty — this binding can never match"
                ));
            }
        }
        if let Some(acct) = r.account_id.as_deref().map(str::trim) {
            if !acct.is_empty() && acct != "*" && acct != "default" {
                out.push(format!(
                    "{who}: match.account_id = \"{acct}\" is not fed by any channel — \
                     the gateway always resolves account_id as \"default\" (multi-account \
                     not yet wired inbound); this binding can never match real traffic"
                ));
            }
        }
        if r.channel.is_none()
            && r.peer.is_none()
            && r.guild_id.is_none()
            && r.team_id.is_none()
            && r.account_id
                .as_deref()
                .map(str::trim)
                .is_none_or(|a| a == "*")
        {
            out.push(format!(
                "{who}: matches every message on every channel — it shadows every \
                 binding after it; put it last or give it a scope"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_config_default() {
        let cfg = SessionConfig::default();
        assert_eq!(cfg.dm_scope, DmScope::PerPeer);
        assert!(cfg.identity_links.is_empty());
    }

    #[test]
    fn test_session_config_deserialize() {
        let toml_str = r#"
            dm_scope = "per-channel-peer"

            [identity_links]
            john = ["telegram:123", "discord:456"]
        "#;
        let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.dm_scope, DmScope::PerChannelPeer);
        assert_eq!(cfg.identity_links["john"].len(), 2);
    }

    #[test]
    fn test_route_binding_deserialize() {
        let toml_str = r#"
            agent_id = "work"
            [match]
            channel = "slack"
            team_id = "T12345"
        "#;
        let binding: RouteBinding = toml::from_str(toml_str).unwrap();
        assert_eq!(binding.agent_id, "work");
        assert_eq!(binding.match_rule.channel.as_deref(), Some("slack"));
        assert_eq!(binding.match_rule.team_id.as_deref(), Some("T12345"));
        assert!(binding.match_rule.workspace.is_none());
    }

    #[test]
    fn test_route_binding_with_workspace() {
        let toml_str = r#"
            agent_id = "main"
            [match]
            channel = "telegram"
            workspace = "crypto"
        "#;
        let binding: RouteBinding = toml::from_str(toml_str).unwrap();
        assert_eq!(binding.agent_id, "main");
        assert_eq!(binding.match_rule.channel.as_deref(), Some("telegram"));
        assert_eq!(binding.match_rule.workspace.as_deref(), Some("crypto"));
    }

    #[test]
    fn session_config_rejects_duplicate_identity_link_ids() {
        // The same channel:id appearing under two canonicals would let the
        // tie-break flip on every reload, leaking messages between users.
        let toml_str = r#"
            [identity_links]
            alice = ["telegram:123"]
            bob = ["telegram:123"]
        "#;
        let err = toml::from_str::<SessionConfig>(toml_str).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("telegram:123"),
            "error should mention the conflicting ID; got: {msg}"
        );
    }

    #[test]
    fn session_config_accepts_unique_identity_links() {
        let toml_str = r#"
            [identity_links]
            alice = ["telegram:123", "discord:456"]
            bob = ["telegram:789"]
        "#;
        let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.identity_links.len(), 2);
    }

    #[test]
    fn binding_problems_flags_unwired_account_id() {
        let bindings = vec![RouteBinding {
            agent_id: "main".to_string(),
            match_rule: MatchRule {
                channel: Some("telegram".to_string()),
                account_id: Some("botA".to_string()),
                ..Default::default()
            },
        }];
        let problems = binding_problems(&bindings);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("account_id") && p.contains("never match")),
            "a specific account_id that no channel feeds must be flagged; got {problems:?}"
        );

        // "default" / "*" / omitted are the only values that can match inbound.
        for acct in [Some("default"), Some("*"), None] {
            let bindings = vec![RouteBinding {
                agent_id: "main".to_string(),
                match_rule: MatchRule {
                    channel: Some("telegram".to_string()),
                    account_id: acct.map(str::to_string),
                    ..Default::default()
                },
            }];
            let problems = binding_problems(&bindings);
            assert!(
                !problems.iter().any(|p| p.contains("account_id")),
                "account_id={acct:?} must not be flagged; got {problems:?}"
            );
        }
    }
}
