//! `[mcp_server]` — what the MCP face exposes, and whether it is mounted.
//!
//! Named `McpFaceConfig`, not `McpServerConfig`: `crate::config::McpServerConfig` is the MCP client's server entry.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[mcp_server]` in `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpFaceConfig {
    /// Mount `POST/GET/DELETE /mcp` on the gateway listener.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Tool names an MCP client may list and call. A whitelist: a name that
    /// is not here is "unknown tool" on the wire, whatever the caller's role.
    /// Absent ⇒ [`default_expose`] (side-effect-free builtins only).
    /// Applies live (`ReloadImpact` subsection `mcp_server.expose`, P6.9);
    /// `enabled` does not — the route is mounted at boot.
    #[serde(default = "default_expose")]
    pub expose: Vec<String>,
}

impl Default for McpFaceConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            expose: default_expose(),
        }
    }
}

const fn default_enabled() -> bool {
    true
}

/// Read-only builtins the predicate WOULD admit and the default nonetheless
/// withholds, each with the reason a foreign agent should not get it unasked.
/// A rule (`is_default_exposable`) decides everything else; this list is only
/// for names the rule cannot tell apart from their neighbours. Every entry is
/// checked by `every_exclusion_is_load_bearing_and_reasoned`: a name the rule
/// already rejects is a stale line and turns that test red.
pub const DEFAULT_EXPOSE_EXCLUDES: &[(&str, &str)] = &[
    (
        "tool_usage",
        "has a `forget_orphans: true` write arm; its READ_ONLY_TOOLS entry says so",
    ),
    (
        "config_audit",
        "discloses this server's own configuration (provider names, key presence, \
         gateway posture) to whatever agent mounts Aleph",
    ),
    (
        "node_list",
        "discloses the cluster's machines; fleet reads are for the operator's own agents, \
         not a default for a foreign one",
    ),
    (
        "user_profile",
        "discloses the operator's personal profile; a foreign agent gets it only when the \
         operator adds it to `expose`",
    ),
];

/// The default exposure, DERIVED from the tables that already answer "is this
/// tool side-effect-free / operator-only / a builtin", minus the reasoned
/// by-name exclusions:
/// `READ_ONLY_TOOLS ∩ BUILTIN_TOOL_DEFINITIONS − OPERATOR_TOOLS − desktop_* − DEFAULT_EXPOSE_EXCLUDES`.
///
/// Why each subtraction: operator-tier names (`terminal`) are gated on a card
/// this face cannot always raise; `desktop_*` are registered only where a
/// desktop bridge exists AND disclose the host's live UI tree to a foreign
/// agent; [`DEFAULT_EXPOSE_EXCLUDES`] carries its own reasons. Tools outside
/// `BUILTIN_TOOL_DEFINITIONS` (`tool_search`, `get_tool_schema`, `mcp_*`,
/// `channel_directory`) are registered per request or conditionally and are
/// not a compile-time fact.
///
/// Pinned name-by-name in this module's tests: growing `READ_ONLY_TOOLS`
/// turns that test red on purpose, so admission here is a decision.
#[must_use]
pub fn default_expose() -> Vec<String> {
    crate::tools::adapters::registry_adapter::READ_ONLY_TOOLS
        .iter()
        .copied()
        .filter(|name| is_default_exposable(name))
        .map(str::to_string)
        .collect()
}

/// The predicate behind [`default_expose`]. Public so the doctor / tests can
/// explain WHY a name is absent from the default.
#[must_use]
pub fn is_default_exposable(name: &str) -> bool {
    crate::executor::BUILTIN_TOOL_DEFINITIONS
        .iter()
        .any(|d| d.name == name)
        && !crate::gateway::method_authz::tool_requires_operator(name)
        && !name.starts_with("desktop_")
        && !DEFAULT_EXPOSE_EXCLUDES.iter().any(|(n, _)| *n == name)
}

/// G5's runtime half: every configured name that the live catalogue does not
/// know. `known` is whatever the caller can execute right now (builtins +
/// active plugin tools + bridged MCP tools at boot). Empty means "all good";
/// the caller decides between a warning and a refusal.
#[must_use]
pub fn unknown_expose_names<'a>(expose: &'a [String], known: &BTreeSet<String>) -> Vec<&'a str> {
    expose
        .iter()
        .map(String::as_str)
        .filter(|name| !known.contains(*name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The default exposure at HEAD, spelled out once so that admitting a
    /// tool to the MCP default surface is a conscious edit here, not a side
    /// effect of adding it to `READ_ONLY_TOOLS`. A red here means one of the
    /// four sources moved (`READ_ONLY_TOOLS`, `BUILTIN_TOOL_DEFINITIONS`,
    /// `OPERATOR_TOOLS`, `DEFAULT_EXPOSE_EXCLUDES`) — decide, then update this list.
    const PINNED_DEFAULT: &[&str] = &[
        "agent_info",
        "agent_list",
        "list_models",
        "read_config_guide",
        "search",
        "web_fetch",
        "ctx_search",
        "document_extract",
        "file_read",
        "grep",
        "find",
        "memory_search",
        "memory_browse",
        "memory_explore",
        "memory_timeline",
        "memory_trace",
        "recall_context",
        "recall_events",
        "governance_metrics",
        "session_list",
        "session_read",
        "session_search",
        "task_list",
        "task_read_artifact",
        "team_status",
        "team_digest",
        "team_usage",
        "heartbeat_list",
        "skill_list",
        "skill_read",
        "skill_status",
        "note_orient",
        "note_graph_query",
        "hub_catalog_search",
        "hub_resolve_spec",
        "hub_fetch_docs",
    ];

    #[test]
    fn the_default_exposure_is_exactly_the_pinned_list() {
        let derived = default_expose();
        let pinned: Vec<String> = PINNED_DEFAULT.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(
            derived, pinned,
            "default_expose() drifted from the pinned list"
        );
    }

    #[test]
    fn every_default_name_is_a_compile_time_builtin() {
        // G5, unit half: the default can never name a tool the executor does
        // not know. The runtime (host-dependent) half is `unknown_expose_names`.
        for name in default_expose() {
            assert!(
                crate::executor::BUILTIN_TOOL_DEFINITIONS
                    .iter()
                    .any(|d| d.name == name),
                "{name} is not in BUILTIN_TOOL_DEFINITIONS"
            );
        }
    }

    #[test]
    fn the_default_never_exposes_an_operator_tier_desktop_or_excluded_tool() {
        for name in default_expose() {
            assert!(
                !crate::gateway::method_authz::tool_requires_operator(&name),
                "{name}"
            );
            assert!(!name.starts_with("desktop_"), "{name}");
            assert!(
                !DEFAULT_EXPOSE_EXCLUDES.iter().any(|(n, _)| *n == name),
                "{name} is on the exclusion list and still exposed"
            );
        }
    }

    /// A by-name exclusion is a list, and a list rots: an entry the predicate
    /// would no longer admit anyway (renamed tool, moved to OPERATOR_TOOLS,
    /// dropped from READ_ONLY_TOOLS) is a stale line that reads like a live
    /// decision. Every exclusion must be one the predicate WOULD admit
    /// without it, and must carry a reason.
    #[test]
    fn every_exclusion_is_load_bearing_and_reasoned() {
        for (name, reason) in DEFAULT_EXPOSE_EXCLUDES {
            assert!(
                !reason.trim().is_empty(),
                "{name}: exclusion without a reason"
            );
            assert!(
                crate::tools::adapters::registry_adapter::READ_ONLY_TOOLS.contains(name),
                "{name} is not read-only; the predicate already rejects it — delete the exclusion"
            );
            assert!(
                crate::executor::BUILTIN_TOOL_DEFINITIONS
                    .iter()
                    .any(|d| d.name == *name),
                "{name} is not a compile-time builtin; the predicate already rejects it"
            );
            assert!(
                !crate::gateway::method_authz::tool_requires_operator(name),
                "{name} is operator-tier; the predicate already rejects it"
            );
            assert!(
                !name.starts_with("desktop_"),
                "{name}: the desktop_ prefix rule already covers it"
            );
        }
    }

    #[test]
    fn default_names_stay_under_dshs_truncation_threshold() {
        // dsh: `mcp__<server>__<raw>` > 64 chars is truncated to 51 + hash.
        // `mcp__aleph__` is 12 chars, so raw names ≤ 52 never hash; pin 51.
        for name in default_expose() {
            assert!(name.len() <= 51, "{name} would be hashed by dsh");
        }
    }

    #[test]
    fn unknown_expose_names_reports_exactly_the_strangers() {
        let known: BTreeSet<String> = ["grep", "file_read"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let expose = vec![
            "grep".to_string(),
            "no_such_tool".to_string(),
            "file_read".to_string(),
        ];
        assert_eq!(unknown_expose_names(&expose, &known), vec!["no_such_tool"]);
        assert!(unknown_expose_names(&[], &known).is_empty());
    }

    #[test]
    fn the_section_round_trips_and_defaults() {
        let parsed: McpFaceConfig = toml::from_str("").unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.expose, default_expose());

        let parsed: McpFaceConfig =
            toml::from_str("enabled = false\nexpose = [\"grep\"]\n").unwrap();
        assert!(!parsed.enabled);
        assert_eq!(parsed.expose, vec!["grep".to_string()]);

        let text = toml::to_string(&McpFaceConfig::default()).unwrap();
        let back: McpFaceConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, McpFaceConfig::default());
    }

    #[test]
    fn the_main_config_carries_the_section_under_mcp_server() {
        let cfg: crate::Config = toml::from_str("[mcp_server]\nexpose = [\"grep\"]\n").unwrap();
        assert!(cfg.mcp_server.enabled);
        assert_eq!(cfg.mcp_server.expose, vec!["grep".to_string()]);
        let cfg: crate::Config = toml::from_str("").unwrap();
        assert_eq!(cfg.mcp_server, McpFaceConfig::default());
    }
}
