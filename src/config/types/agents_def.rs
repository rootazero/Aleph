//! Agent Definition configuration types
//!
//! Defines the `[agents]` section of the configuration, which declares
//! the set of available agents and their global defaults.
//!
//! - `AgentsConfig`: Top-level `[agents]` section
//! - `AgentDefaults`: Global defaults inherited by all agents
//! - `AgentDefinition`: A single agent declaration
//! - `SubagentPolicy`: Controls which sub-agents an agent may spawn

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::thinker::soul_archetypes::SoulArchetype;

// =============================================================================
// AgentsConfig
// =============================================================================

/// Top-level `[agents]` configuration section
///
/// Contains global defaults and the list of agent definitions.
/// If `list` is empty after deserialization, call `ensure_default()` to
/// guarantee at least a "main" agent exists.
///
/// # Example TOML
/// ```toml
/// [agents.defaults]
/// model = "claude-sonnet-4"
///
/// [[agents.list]]
/// id = "main"
/// default = true
/// name = "Main Agent"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentsConfig {
    /// Global defaults inherited by all agents unless overridden
    #[serde(default)]
    pub defaults: AgentDefaults,

    /// List of agent definitions
    #[serde(default)]
    pub list: Vec<AgentDefinition>,
}

impl AgentsConfig {
    /// Ensure at least one default "main" agent exists.
    ///
    /// If `list` is empty, inserts a minimal agent with `id = "main"` and
    /// `default = true`. If agents are already present, this is a no-op.
    pub fn ensure_default(&mut self) {
        if self.list.is_empty() {
            self.list.push(AgentDefinition {
                id: "main".to_string(),
                default: true,
                name: Some("Main Agent".to_string()),
                ..Default::default()
            });
        }
    }
}

// =============================================================================
// AgentDefaults
// =============================================================================

/// Global agent defaults
///
/// Values here are inherited by every `AgentDefinition` unless that agent
/// overrides them explicitly.
///
/// # Example TOML
/// ```toml
/// [agents.defaults]
/// model = "claude-sonnet-4"
/// workspace_root = "~/workspaces"
/// skills = ["search", "code_review"]
/// dm_scope = "workspace"
/// bootstrap_max_chars = 8000
/// bootstrap_total_max_chars = 32000
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentDefaults {
    /// Default AI model for all agents
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Default workspace root directory
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,

    /// Default agent state root directory (default: ~/.aleph/agents)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents_root: Option<PathBuf>,

    /// Default skills available to all agents
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,

    /// Default skills blacklist (blocked tools) for all agents
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_blacklist: Option<Vec<String>>,

    /// Default DM (domain model) scope
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dm_scope: Option<String>,

    /// Maximum characters for a single bootstrap file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_max_chars: Option<usize>,

    /// Maximum total characters across all bootstrap files
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_total_max_chars: Option<usize>,
}

// =============================================================================
// AgentIdentity
// =============================================================================

/// Agent identity for display purposes
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
}

// =============================================================================
// AgentModelRef
// =============================================================================

/// The model reference selected for an agent.
///
/// Used as the value of `AgentDefinition.model`; a `None` field means
/// **inherit the system default model**
/// (= `defaults.model > profile.model > DEFAULT_MODEL`, i.e. "current default model").
///
/// `#[serde(untagged)]` supports two TOML forms:
/// - bare string `model = "claude-x"` → [`AgentModelRef::Legacy`] (old format, no drop-detection)
/// - inline table `model = { provider = "anthropic", model = "claude-x" }` → [`AgentModelRef::Qualified`]
///   (Panel picks from configured models, carrying the provider so "deleted/disabled/dropped"
///   detection can auto-fallback)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AgentModelRef {
    /// Panel-selected configured model, pinning provider + model.
    Qualified { provider: String, model: String },
    /// Free-form string from old config. Never participates in drop-detection fallback.
    Legacy(String),
}

impl AgentModelRef {
    /// Returns the model id (carried by both variants). Used only for display
    /// and "no-validation" paths; the real drop-detection resolution lives in
    /// `agent_resolver::resolve_model_ref`.
    ///
    /// `pub(crate)` — the only consumer is this module's own test; the real
    /// production code pattern-matches [`AgentModelRef`] directly.
    pub(crate) fn model_str(&self) -> &str {
        match self {
            Self::Qualified { model, .. } => model,
            Self::Legacy(s) => s,
        }
    }
}

// =============================================================================
// AgentDefinition
// =============================================================================

/// A single agent definition
///
/// Each agent has a unique `id` and optional overrides for model, skills,
/// workspace, profile, and sub-agent policies.
///
/// # Example TOML
/// ```toml
/// [[agents.list]]
/// id = "coder"
/// name = "Coding Agent"
/// workspace = "~/projects"
/// profile = "coding"
/// model = "claude-opus-4"
/// skills = ["git_*", "fs_*"]
///
/// [agents.list.subagents]
/// allow = ["reviewer", "tester"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentDefinition {
    /// Unique agent identifier
    pub id: String,

    /// Whether this is the default agent (at most one should be true)
    #[serde(default)]
    pub default: bool,

    /// Human-readable display name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Workspace directory for this agent.
    ///
    /// **Deprecated**: This field is ignored at runtime. The workspace directory
    /// is always `{workspace_root}/{agent_id}` (1:1 binding). Kept for backward
    /// compatibility with existing TOML configs; will be removed in a future version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,

    /// Profile to inherit from (references a `[profiles.<name>]` entry)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,

    /// AI model override for this agent.
    /// `None` = inherit system default model (= defaults.model > profile.model > `DEFAULT_MODEL`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<AgentModelRef>,

    /// Skills available to this agent (overrides defaults)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,

    /// Skills blacklist (blocked tools) for this agent (overrides defaults)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_blacklist: Option<Vec<String>>,

    /// Soul archetype base for this agent's persona.
    /// Consumed once, when SOUL.md is first composed (`expert | companion |
    /// assistant | maker`). `None` falls back to `assistant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archetype: Option<SoulArchetype>,

    /// Agent identity (emoji, description, avatar, theme)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<AgentIdentity>,

    /// Sub-agent spawning policy
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<SubagentPolicy>,

    /// Per-agent tool permission overrides (merged with global policies)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_permissions: Option<crate::config::types::policies::ToolPermissionsConfig>,

    /// Link access whitelist.
    /// None or empty = all links allowed (default).
    /// Some(list) = only listed link IDs can access this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_links: Option<Vec<String>>,

    /// Which authenticated users may start a run **as** this agent
    /// (`users.user_id` values).
    ///
    /// None or empty = every authenticated caller (default). Some(list) = only
    /// those users; anyone else is refused before the run is built.
    ///
    /// This is the axis `tool_permissions` is keyed on. Until this field
    /// existed, an agent's permission set was chosen by the caller — a member
    /// denied `shell` under one agent simply named another agent on the wire
    /// and inherited its permissions, because the run-start guard only asked
    /// "is this SESSION mine", never "may I act as this AGENT". Enforcement is
    /// [`agent_admits_user`], reached through
    /// [`caller_may_act_as_agent`](crate::gateway::caller_identity::caller_may_act_as_agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_users: Option<Vec<String>>,
}

/// The one admission rule for [`AgentDefinition::allowed_users`].
///
/// Three carriers hold this list on its way to the run-start gate
/// (`AgentDefinition` → `ResolvedAgent` → `AgentInstanceConfig`), and a
/// predicate written at each would be three chances to disagree — the shape
/// `restrictive_min` avoids for permissions and `session_visible` avoids for
/// sessions. They all call this.
///
/// # Why an absent user is admitted
///
/// `user: None` is not "an anonymous stranger" — the login wall refuses those
/// before any method runs. It means there is no gateway connection at all:
/// cron, heartbeat, A2A, a team dispatcher, an in-process test. Every sibling
/// predicate in the codebase opens with that same arm
/// (`caller_may_choose_directory`, `role_is_operator`, `caller_is_member`),
/// and deviating here would kill scheduled work on any restricted agent the
/// moment an operator named a single user. Loopback is NOT this case — it
/// resolves to the implicit owner, so a local Panel arrives as `Some(owner)`.
#[must_use]
pub fn agent_admits_user(allowed: Option<&[String]>, user: Option<&str>) -> bool {
    match allowed {
        // Unset or empty: unrestricted. This is what keeps a fresh install and
        // every pre-existing config byte-for-byte unchanged in behaviour.
        None | Some([]) => true,
        Some(list) => match user {
            Some(u) => list.iter().any(|allowed_user| allowed_user == u),
            None => true,
        },
    }
}

/// Storage form of a patched `allowed_users` list.
///
/// An empty list is the wire form of "clear the restriction", and the TOML
/// writer expresses that by **removing the key** — writing `allowed_users = []`
/// would read, to whoever opens `config.toml` next, like "nobody may use this
/// agent", the opposite of what [`agent_admits_user`] does with it. This is the
/// same collapse for the runtime half, so a live agent and the file it was
/// loaded from cannot disagree about which of `None` / `Some([])` is on disk.
///
/// Both are admitted by [`agent_admits_user`], so this changes no verdict — it
/// keeps one representation reaching the registry, which is what makes "reload
/// from disk" and "apply live" produce the same object.
#[must_use]
pub fn normalized_allowed_users(list: &[String]) -> Option<Vec<String>> {
    if list.is_empty() {
        None
    } else {
        Some(list.to_vec())
    }
}

// =============================================================================
// SubagentPolicy
// =============================================================================

/// Sub-agent spawning policy
///
/// Controls which sub-agents an agent is allowed to spawn.
/// Use `["*"]` to allow all agents, or list specific agent IDs.
///
/// # Example TOML
/// ```toml
/// [agents.list.subagents]
/// allow = ["reviewer", "tester"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct SubagentPolicy {
    /// List of allowed sub-agent IDs, or `["*"]` for unrestricted
    #[serde(default)]
    pub allow: Vec<String>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agents_config_deserialize_full() {
        let toml_str = r#"
            [defaults]
            model = "claude-sonnet-4"
            workspace_root = "/home/user/workspaces"
            skills = ["search", "code_review"]
            dm_scope = "workspace"
            bootstrap_max_chars = 8000
            bootstrap_total_max_chars = 32000

            [[list]]
            id = "main"
            default = true
            name = "Main Agent"
            model = "claude-opus-4"
            skills = ["git_*", "fs_*"]

            [[list]]
            id = "reviewer"
            name = "Code Reviewer"
            profile = "coding"
            workspace = "/home/user/reviews"

            [list.subagents]
            allow = ["tester"]
        "#;

        let config: AgentsConfig = toml::from_str(toml_str).unwrap();

        // Verify defaults
        assert_eq!(config.defaults.model, Some("claude-sonnet-4".to_string()));
        assert_eq!(
            config.defaults.workspace_root,
            Some(PathBuf::from("/home/user/workspaces"))
        );
        assert_eq!(
            config.defaults.skills,
            Some(vec!["search".to_string(), "code_review".to_string()])
        );
        assert_eq!(config.defaults.dm_scope, Some("workspace".to_string()));
        assert_eq!(config.defaults.bootstrap_max_chars, Some(8000));
        assert_eq!(config.defaults.bootstrap_total_max_chars, Some(32000));

        // Verify agent list
        assert_eq!(config.list.len(), 2);

        // First agent
        let main = &config.list[0];
        assert_eq!(main.id, "main");
        assert!(main.default);
        assert_eq!(main.name, Some("Main Agent".to_string()));
        assert_eq!(
            main.model,
            Some(AgentModelRef::Legacy("claude-opus-4".to_string()))
        );
        assert_eq!(
            main.skills,
            Some(vec!["git_*".to_string(), "fs_*".to_string()])
        );
        assert!(main.subagents.is_none());

        // Second agent
        let reviewer = &config.list[1];
        assert_eq!(reviewer.id, "reviewer");
        assert!(!reviewer.default);
        assert_eq!(reviewer.name, Some("Code Reviewer".to_string()));
        assert_eq!(reviewer.profile, Some("coding".to_string()));
        assert_eq!(
            reviewer.workspace,
            Some(PathBuf::from("/home/user/reviews"))
        );
        let subagents = reviewer.subagents.as_ref().unwrap();
        assert_eq!(subagents.allow, vec!["tester"]);
    }

    #[test]
    fn test_agents_config_empty_deserialize() {
        let toml_str = "";
        let config: AgentsConfig = toml::from_str(toml_str).unwrap();

        // Defaults should all be None/empty
        assert!(config.defaults.model.is_none());
        assert!(config.defaults.workspace_root.is_none());
        assert!(config.defaults.agents_root.is_none());
        assert!(config.defaults.skills.is_none());
        assert!(config.defaults.dm_scope.is_none());
        assert!(config.defaults.bootstrap_max_chars.is_none());
        assert!(config.defaults.bootstrap_total_max_chars.is_none());

        // List should be empty
        assert!(config.list.is_empty());
    }

    #[test]
    fn test_ensure_default_when_empty() {
        let mut config = AgentsConfig::default();
        assert!(config.list.is_empty());

        config.ensure_default();

        assert_eq!(config.list.len(), 1);
        let agent = &config.list[0];
        assert_eq!(agent.id, "main");
        assert!(agent.default);
        assert_eq!(agent.name, Some("Main Agent".to_string()));
    }

    #[test]
    fn test_ensure_default_noop_when_populated() {
        let mut config = AgentsConfig {
            list: vec![AgentDefinition {
                id: "custom".to_string(),
                name: Some("Custom Agent".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };

        config.ensure_default();

        // Should not add another agent
        assert_eq!(config.list.len(), 1);
        assert_eq!(config.list[0].id, "custom");
    }

    #[test]
    fn test_agent_identity_deserialize() {
        let toml_str = r#"
            emoji = "🧑‍💻"
            description = "Full-stack coding specialist"
            avatar = "https://example.com/avatar.png"
            theme = "Write clean code"
        "#;
        let identity: AgentIdentity = toml::from_str(toml_str).unwrap();
        assert_eq!(identity.emoji, Some("🧑‍💻".to_string()));
        assert_eq!(
            identity.description,
            Some("Full-stack coding specialist".to_string())
        );
    }

    #[test]
    fn test_agent_definition_with_new_fields() {
        let toml_str = r#"
            [[list]]
            id = "coder"
            name = "Code Master"
            default = true

            [list.identity]
            emoji = "🧑‍💻"
            description = "Full-stack coding specialist"
        "#;
        let config: AgentsConfig = toml::from_str(toml_str).unwrap();
        let agent = &config.list[0];
        assert_eq!(agent.id, "coder");
        assert!(agent.identity.is_some());
        assert_eq!(
            agent.identity.as_ref().unwrap().emoji,
            Some("🧑‍💻".to_string())
        );
    }

    #[test]
    fn test_backward_compat_model_field() {
        let toml_str = r#"
            [[list]]
            id = "legacy"
            model = "claude-sonnet-4"
        "#;
        let config: AgentsConfig = toml::from_str(toml_str).unwrap();
        let agent = &config.list[0];
        assert_eq!(
            agent.model,
            Some(AgentModelRef::Legacy("claude-sonnet-4".to_string()))
        );
    }

    #[test]
    fn test_agents_root_deserialize() {
        let toml_str = r#"
            [defaults]
            agents_root = "/home/user/agents"
            workspace_root = "/home/user/workspaces"
        "#;
        let config: AgentsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.defaults.agents_root,
            Some(PathBuf::from("/home/user/agents"))
        );
    }

    #[test]
    fn test_allowed_links_deserialize() {
        let toml_str = r#"
            [[list]]
            id = "private"
            name = "Private Agent"
            allowed_links = ["telegram-bot-1", "discord-bot"]
        "#;
        let config: AgentsConfig = toml::from_str(toml_str).unwrap();
        let agent = &config.list[0];
        assert_eq!(
            agent.allowed_links,
            Some(vec![
                "telegram-bot-1".to_string(),
                "discord-bot".to_string()
            ])
        );
    }

    #[test]
    fn test_allowed_links_none_by_default() {
        let toml_str = r#"
            [[list]]
            id = "open"
            name = "Open Agent"
        "#;
        let config: AgentsConfig = toml::from_str(toml_str).unwrap();
        assert!(config.list[0].allowed_links.is_none());
    }

    #[test]
    fn test_subagent_policy_wildcard() {
        let toml_str = r#"
            allow = ["*"]
        "#;

        let policy: SubagentPolicy = toml::from_str(toml_str).unwrap();
        assert_eq!(policy.allow, vec!["*"]);
    }
}

#[cfg(test)]
mod model_ref_tests {
    use super::AgentModelRef;

    #[test]
    fn legacy_bare_string_roundtrips() {
        let m: AgentModelRef = serde_json::from_value(serde_json::json!("claude-opus-4")).unwrap();
        assert_eq!(m, AgentModelRef::Legacy("claude-opus-4".to_string()));
        assert_eq!(m.model_str(), "claude-opus-4");
    }

    #[test]
    fn qualified_table_parses() {
        let m: AgentModelRef = serde_json::from_value(
            serde_json::json!({ "provider": "anthropic", "model": "claude-sonnet-4" }),
        )
        .unwrap();
        assert_eq!(
            m,
            AgentModelRef::Qualified {
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4".to_string(),
            }
        );
        assert_eq!(m.model_str(), "claude-sonnet-4");
    }

    #[test]
    fn legacy_serializes_back_to_bare_string() {
        let m = AgentModelRef::Legacy("gpt-5".to_string());
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!("gpt-5")
        );
    }

    #[test]
    fn qualified_serializes_to_table() {
        let m = AgentModelRef::Qualified {
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({ "provider": "openai", "model": "gpt-5" })
        );
    }
}

#[cfg(test)]
mod allowed_users_tests {
    use super::*;

    fn list(users: &[&str]) -> Vec<String> {
        users.iter().map(|u| (*u).to_string()).collect()
    }

    /// The zero-change guarantee. Nothing in a fresh config — or in any config
    /// written before this field existed — names anyone, and every one of those
    /// installs must keep behaving exactly as it did.
    #[test]
    fn an_agent_that_names_nobody_admits_everyone() {
        assert!(agent_admits_user(None, Some("u-alice")));
        assert!(agent_admits_user(None, None));
        assert!(agent_admits_user(Some(&[]), Some("u-alice")));
    }

    #[test]
    fn a_named_user_is_admitted_and_an_unnamed_one_is_not() {
        let allowed = list(&["u-alice", "u-carol"]);
        assert!(agent_admits_user(Some(&allowed), Some("u-alice")));
        assert!(agent_admits_user(Some(&allowed), Some("u-carol")));
        assert!(!agent_admits_user(Some(&allowed), Some("u-bob")));
    }

    /// Matching is exact. A prefix/substring rule here would make `u-bob`
    /// admissible on a list naming `u-bobby`, which is the kind of quiet
    /// widening nobody re-reads a config to discover.
    #[test]
    fn matching_is_exact_not_prefix() {
        let allowed = list(&["u-bobby"]);
        assert!(!agent_admits_user(Some(&allowed), Some("u-bob")));
        assert!(!agent_admits_user(Some(&allowed), Some("u-bobby-2")));
        assert!(agent_admits_user(Some(&allowed), Some("u-bobby")));
    }

    /// A run with no gateway connection at all (cron, heartbeat, A2A, a team
    /// dispatcher, an in-process test) is admitted — the same first arm every
    /// sibling predicate opens with. Pinned because the alternative is
    /// attractive-looking and wrong: refusing here would silently kill every
    /// scheduled job on an agent the moment an operator named one user, and
    /// the failure would surface as "the nightly run stopped happening".
    #[test]
    fn a_run_with_no_gateway_caller_is_admitted() {
        let allowed = list(&["u-alice"]);
        assert!(agent_admits_user(Some(&allowed), None));
    }

    /// The field survives a TOML round trip under the name the operator types.
    #[test]
    fn allowed_users_round_trips_through_toml() {
        let toml_str = r#"
            id = "ops"
            allowed_users = ["u-alice", "u-carol"]
        "#;
        let agent: AgentDefinition = toml::from_str(toml_str).expect("parse");
        assert_eq!(
            agent.allowed_users.as_deref(),
            Some(list(&["u-alice", "u-carol"]).as_slice())
        );
        let back = toml::to_string(&agent).expect("serialize");
        assert!(back.contains("allowed_users"));

        // Absent stays absent rather than serializing an empty list that would
        // read, to a human, like "nobody is allowed".
        let bare: AgentDefinition = toml::from_str(r#"id = "main""#).expect("parse");
        assert!(bare.allowed_users.is_none());
        assert!(!toml::to_string(&bare)
            .expect("serialize")
            .contains("allowed_users"));
    }
}
