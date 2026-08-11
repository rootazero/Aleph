//! Agent type definitions.

use serde::{Deserialize, Serialize};

/// Origin of an `AgentDef`. Set by `crate::agents::loader` based on load source;
/// hardcoded `builtin_agents()` entries default to `Builtin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AgentSource {
    #[default]
    Builtin,
    User,
    Project,
    /// Shipped by an installed plugin (`<plugin>/agents/*`), bridged into the
    /// registry via the process-global published by
    /// `ExtensionManager::load_all`. Lowest precedence: a Builtin / User /
    /// Project agent of the same id shadows a plugin's (resolve consults the
    /// plugin set only on a registry miss; the catalog folds it in
    /// insert-if-absent).
    Plugin,
}

/// Mode of an agent
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentMode {
    /// Main agent that responds directly to user
    Primary,
    /// Sub-agent called by other agents
    SubAgent,
}

impl std::fmt::Display for AgentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Primary => write!(f, "Primary"),
            Self::SubAgent => write!(f, "SubAgent"),
        }
    }
}

/// Subagent execution isolation mode (P3 Stage H).
///
/// `Worktree` runs the subagent in a fresh git worktree under `$TMPDIR`
/// with a separate `target/` dir; cleanup is guaranteed on every exit path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IsolationMode {
    Worktree,
}

/// Inline MCP server config carried in `McpServerSpec::Inline` (P3 Stage I).
///
/// Spawned fresh for the subagent's lifetime; not shared across agents.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McpInlineConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

/// Per-agent MCP server scope (P3 Stage I).
///
/// `Inline` spawns a fresh process owned by this subagent. `Reference`
/// reuses a server already registered in the global `McpRegistry`.
/// Name-conflict detection (Inline name vs global) happens at spawn
/// time (`McpScope::provision`), not at loader time — see design § 3
/// Q2 for the rationale.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServerSpec {
    Inline {
        name: String,
        config: McpInlineConfig,
    },
    Reference {
        name: String,
    },
}

/// Context mode for sub-agents
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ContextMode {
    /// Start with a fresh context (no parent history)
    #[default]
    Fresh,
    /// Receive a summary of parent context
    Summary,
}

impl std::fmt::Display for ContextMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fresh => write!(f, "Fresh"),
            Self::Summary => write!(f, "Summary"),
        }
    }
}

/// Where **this** sub-agent call's starting context comes from.
///
/// [`ContextMode`] answers the same question one level up — it is the *agent
/// definition's* default, chosen by whoever wrote the agent file. That was the
/// only answer available for a long time, and it puts the choice in the wrong
/// hands: whether a given delegation wants an untainted reviewer or a child
/// that already knows what happened is a property of the delegation, not of the
/// role. `explore` is the right agent for both "go find out, ignore what I
/// think" and "carry on from where we are".
///
/// So this is the per-call override; `None` at the call site falls back to
/// [`ContextMode`], which keeps every existing caller byte-identical.
///
/// There is deliberately **no** `Fork` variant on `ContextMode`. An agent file
/// cannot sensibly declare "always fork" — a fork is meaningful only relative to
/// a live parent transcript, and the same role is spawned from cron, from a
/// channel, and from a nested child, where there is nothing worth forking. A
/// variant with no producer is the abstraction R10's YAGNI clause says to leave
/// unbuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnContext {
    /// Clean room. The child sees its task and nothing else — not the parent's
    /// transcript, not the parent's `context_summary`.
    ///
    /// This is the setting an independent review needs: the single input most
    /// likely to bias a verifier is the author's own account of what it did and
    /// why, and that account is exactly what `context_summary` contains.
    Isolated,
    /// The caller's hand-written `context_summary`, prefixed to the task.
    /// Lossy and parent-authored, but cheap and under the caller's control.
    Summary,
    /// A verbatim copy of the parent's own recent transcript.
    ///
    /// Highest fidelity — the child reads what actually happened rather than
    /// the parent's précis of it — and the only mode with prefix warmth across
    /// a fan-out (see [`crate::agents::subagent_spawner::fork`] for what that
    /// does and does not mean). It also inherits the parent's framing wholesale,
    /// which is the opposite of what [`Self::Isolated`] is for.
    ///
    /// `turns` bounds how many complete parent turns are carried, newest-first;
    /// `None` means "as many as fit the child's context budget".
    Fork { turns: Option<usize> },
}

impl SpawnContext {
    /// The per-call mode an agent's declared default corresponds to.
    #[must_use]
    pub const fn from_context_mode(mode: &ContextMode) -> Self {
        match mode {
            ContextMode::Fresh => Self::Isolated,
            ContextMode::Summary => Self::Summary,
        }
    }

    /// Parse the model-facing `context` argument.
    ///
    /// Returns the accepted spelling on success and `None` on anything else, so
    /// the caller can reject with a message naming the valid values rather than
    /// silently falling back — a mistyped `context` that degraded to the agent
    /// default would be a review the caller believes is isolated and is not.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "isolated" => Some(Self::Isolated),
            "summary" => Some(Self::Summary),
            "fork" => Some(Self::Fork { turns: None }),
            _ => None,
        }
    }

    /// Every value [`Self::parse`] accepts, for schema and error text.
    ///
    /// One source: the tool schema's `enum`, the parse error message and the
    /// parser itself all read this, so a fourth mode cannot be added to one and
    /// missed by the others.
    pub const ACCEPTED: &'static [&'static str] = &["isolated", "summary", "fork"];

    /// Is this a fork? (Pattern-matching helper for call sites that only care
    /// about the branch, not the bound.)
    #[must_use]
    pub const fn is_fork(&self) -> bool {
        matches!(self, Self::Fork { .. })
    }

    /// The wire spelling — the exact string a caller would pass as `context`.
    ///
    /// Exists so the *other* face of this axis can speak the same vocabulary.
    /// `agent_info` reports an agent's declared default, and it used to render
    /// `ContextMode`'s own `Display` (`Fresh` / `Summary`) — which meant the
    /// model was shown one word and had to type a different one for the same
    /// thing, with nothing anywhere saying they were the same thing. One axis,
    /// one set of names.
    #[must_use]
    pub const fn as_arg(&self) -> &'static str {
        match self {
            Self::Isolated => "isolated",
            Self::Summary => "summary",
            Self::Fork { .. } => "fork",
        }
    }
}

/// Definition of an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    /// Unique identifier (e.g., "explore", "coder", "researcher")
    pub id: String,
    /// One-line description for catalog index
    pub description: String,
    /// Usage trigger hint for the model
    pub when_to_use: Option<String>,
    /// Agent mode
    pub mode: AgentMode,
    /// Prompt sections this agent needs (assembled by Section Registry)
    pub prompt_sections: Vec<String>,
    /// Tools this agent is allowed to use ("*" for all)
    pub allowed_tools: Vec<String>,
    /// Provenance of `allowed_tools`: was the flat list set explicitly by
    /// the loader or another builder, or is it still the constructor default?
    /// Distinguishes "author wrote `allowed_tools: ['*']`" from "no one has
    /// touched this since `AgentDef::new`". `with_allowed_tool_sets` only
    /// clears `['*']` in the second case (so a builtin like `explore` keeps
    /// its cleared allowlist) and leaves an explicit `['*']` alone (so an
    /// author who wrote both `allowed_tools: ['*']` and `allowed_tool_sets`
    /// actually gets the union they asked for). Private to the module on
    /// purpose — only the builders and the loader should mutate it.
    #[serde(skip)]
    pub allowed_tools_explicit: bool,
    /// Named tool sets for declarative agent allowlists. Resolved via
    /// `crate::agents::tool_sets::resolve`; unknown names contribute nothing
    /// (silent skip at runtime; the loader emits a warning at startup).
    /// `allowed_tools` is unioned on top — a tool is allowed if it appears
    /// in any resolved set OR in the flat list.
    #[serde(default)]
    pub allowed_tool_sets: Vec<String>,
    /// Tools this agent is denied from using
    pub denied_tools: Vec<String>,
    /// Maximum iterations (overrides default loop limit)
    pub max_iterations: Option<u32>,
    /// Token budget override for this agent's loop
    pub token_budget: Option<u32>,
    /// Suggested model to use (e.g., "fast", "deep")
    pub model_hint: Option<String>,
    /// Provider this agent should run on — a `[providers]` toml name. When set
    /// and the name resolves at boot, the agent's subagent runs on that
    /// provider pinned as primary, then falls through the global failover
    /// chain. `#[serde(default)]` for schema back-compat: legacy agent files
    /// have no `provider_hint` key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_hint: Option<String>,
    /// Context mode: whether sub-agent gets parent context
    pub context_mode: ContextMode,
    /// Where this definition was loaded from
    #[serde(default)]
    pub source: AgentSource,
    /// Per-agent MCP server scope (P3 Stage I). `#[serde(default)]` for
    /// schema back-compat; legacy agent files have no `mcp_servers` key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServerSpec>,
    /// B1 — subagent worktree isolation. `#[serde(default)]` for schema
    /// back-compat; `None` (default) keeps the shared-cwd behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<IsolationMode>,
}

impl AgentDef {
    /// Create a new agent definition
    pub fn new(id: impl Into<String>, mode: AgentMode) -> Self {
        Self {
            id: id.into(),
            description: String::new(),
            when_to_use: None,
            mode,
            prompt_sections: vec![],
            allowed_tools: vec!["*".into()],
            allowed_tool_sets: vec![],
            denied_tools: vec![],
            max_iterations: None,
            token_budget: None,
            model_hint: None,
            provider_hint: None,
            context_mode: ContextMode::default(),
            source: AgentSource::default(),
            mcp_servers: vec![],
            isolation: None,
            allowed_tools_explicit: false,
        }
    }

    /// Set one-line description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Set usage trigger hint
    pub fn with_when_to_use(mut self, hint: impl Into<String>) -> Self {
        self.when_to_use = Some(hint.into());
        self
    }

    /// Set allowed tools
    ///
    /// Marks the flat list as *explicit* so [`Self::with_allowed_tool_sets`]
    /// can distinguish an author-written `allowed_tools: ['*']` from the
    /// constructor default — the previous value-inspection heuristic (B1-06)
    /// conflated the two and silently dropped an explicit wildcard whenever a
    /// named set was also declared. The provenance flag survives on the
    /// struct via the (private) `allowed_tools_explicit` bit the loader is
    /// expected to set when an `allowed_tools` key was actually present in
    /// the parsed frontmatter.
    #[must_use]
    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = tools;
        self.allowed_tools_explicit = true;
        self
    }

    /// Set named tool sets.
    ///
    /// The flat `allowed_tools` is left alone **iff** the caller (or the
    /// loader, via `with_allowed_tools`) marked it explicit. When the flat
    /// list is still the constructor default `['*']`, it is cleared so the
    /// named sets actually govern access; an explicit `['*']` (from
    /// frontmatter or from another builder) survives intact.
    ///
    /// Callers wanting both an explicit flat list and named sets get both —
    /// this matches the documented behaviour on the previous heuristic and
    /// removes the silent-drop footgun the value-inspection heuristic had.
    #[must_use]
    pub fn with_allowed_tool_sets(mut self, sets: Vec<String>) -> Self {
        self.allowed_tool_sets = sets;
        if !self.allowed_tools_explicit
            && self.allowed_tools.len() == 1
            && self.allowed_tools.first().is_some_and(|s| s == "*")
        {
            self.allowed_tools = vec![];
        }
        self
    }

    /// Set denied tools
    #[must_use]
    pub fn with_denied_tools(mut self, tools: Vec<String>) -> Self {
        self.denied_tools = tools;
        self
    }

    /// Set max iterations
    #[must_use]
    pub const fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = Some(max);
        self
    }

    /// Set context mode
    #[must_use]
    pub const fn with_context_mode(mut self, mode: ContextMode) -> Self {
        self.context_mode = mode;
        self
    }

    /// Set token budget
    #[must_use]
    pub const fn with_token_budget(mut self, budget: u32) -> Self {
        self.token_budget = Some(budget);
        self
    }

    /// Set model hint
    pub fn with_model_hint(mut self, hint: impl Into<String>) -> Self {
        self.model_hint = Some(hint.into());
        self
    }

    /// Set provider hint — the `[providers]` toml name this agent runs on.
    pub fn with_provider_hint(mut self, hint: impl Into<String>) -> Self {
        self.provider_hint = Some(hint.into());
        self
    }

    /// Set prompt sections this agent needs
    #[must_use]
    pub fn with_prompt_sections(mut self, sections: Vec<String>) -> Self {
        self.prompt_sections = sections;
        self
    }

    /// Set per-agent MCP server scope (P3 Stage I).
    #[must_use]
    pub fn with_mcp_servers(mut self, specs: Vec<McpServerSpec>) -> Self {
        self.mcp_servers = specs;
        self
    }

    /// B1 — set subagent worktree isolation.
    #[must_use]
    pub const fn with_isolation(mut self, mode: IsolationMode) -> Self {
        self.isolation = Some(mode);
        self
    }

    /// Check if a tool is allowed for this agent.
    ///
    /// **Recursion guard**: agents in `AgentMode::SubAgent` are denied the
    /// `subagent` tool unconditionally — this rule overrides the allowlist
    /// (including wildcard `"*"`) and any explicit `"subagent"` entry. This
    /// prevents a subagent from spawning further subagents and triggering
    /// unbounded recursion. Primary-mode agents retain full subagent
    /// spawning capability subject to the normal allow/deny lists.
    #[must_use]
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        // Stage B (P1): recursion guard — system invariant, overrides everything
        if matches!(self.mode, AgentMode::SubAgent) && tool_name == "subagent" {
            return false;
        }

        // Explicit deny short-circuits (after recursion guard, before allows)
        if self.denied_tools.iter().any(|t| t == tool_name) {
            return false;
        }

        // Stage G (P2): named allowed_tool_sets
        for set_name in &self.allowed_tool_sets {
            if let Some(tools) = crate::agents::tool_sets::resolve(set_name) {
                if tools.contains(&tool_name) {
                    return true;
                }
            }
        }

        // Existing flat allowlist with "*" wildcard support
        self.allowed_tools
            .iter()
            .any(|t| t == "*" || t == tool_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_def_new() {
        let agent = AgentDef::new("test", AgentMode::SubAgent);
        assert_eq!(agent.id, "test");
        assert_eq!(agent.mode, AgentMode::SubAgent);
        assert!(agent.prompt_sections.is_empty());
        assert_eq!(agent.allowed_tools, vec!["*"]);
        assert!(agent.denied_tools.is_empty());
        assert!(agent.max_iterations.is_none());
    }

    #[test]
    fn test_prompt_sections_default_empty() {
        let agent = AgentDef::new("test", AgentMode::Primary);
        assert!(agent.prompt_sections.is_empty());
    }

    #[test]
    fn test_with_prompt_sections() {
        let agent = AgentDef::new("test", AgentMode::SubAgent).with_prompt_sections(vec![
            "core_identity".into(),
            "tool_usage".into(),
            "safety".into(),
        ]);
        assert_eq!(agent.prompt_sections.len(), 3);
        assert_eq!(agent.prompt_sections[0], "core_identity");
        assert_eq!(agent.prompt_sections[1], "tool_usage");
        assert_eq!(agent.prompt_sections[2], "safety");
    }

    #[test]
    fn test_is_tool_allowed_wildcard() {
        let agent = AgentDef::new("test", AgentMode::Primary);
        assert!(agent.is_tool_allowed("any_tool"));
        assert!(agent.is_tool_allowed("another_tool"));
    }

    #[test]
    fn test_is_tool_allowed_specific() {
        let agent = AgentDef::new("test", AgentMode::SubAgent)
            .with_allowed_tools(vec!["read_file".into(), "glob".into()]);

        assert!(agent.is_tool_allowed("read_file"));
        assert!(agent.is_tool_allowed("glob"));
        assert!(!agent.is_tool_allowed("write_file"));
    }

    #[test]
    fn test_is_tool_denied() {
        let agent = AgentDef::new("test", AgentMode::SubAgent)
            .with_denied_tools(vec!["bash".into(), "write_file".into()]);

        assert!(!agent.is_tool_allowed("bash"));
        assert!(!agent.is_tool_allowed("write_file"));
        assert!(agent.is_tool_allowed("read_file"));
    }

    #[test]
    fn test_denied_overrides_allowed() {
        let agent = AgentDef::new("test", AgentMode::SubAgent)
            .with_allowed_tools(vec!["bash".into()])
            .with_denied_tools(vec!["bash".into()]);

        // Denied takes precedence
        assert!(!agent.is_tool_allowed("bash"));
    }

    #[test]
    fn test_with_allowed_tool_sets_clears_constructor_wildcard() {
        // No prior with_allowed_tools: the heuristic still fires and the
        // constructor default `["*"]` is cleared so the named sets govern
        // (this is the path builtin `explore` and `loop-auditor` rely on).
        let agent = AgentDef::new("test", AgentMode::SubAgent)
            .with_allowed_tool_sets(vec!["INVESTIGATION".into()]);
        assert!(agent.allowed_tools.is_empty());
        assert!(agent.allowed_tools_explicit);
    }

    #[test]
    fn test_with_allowed_tool_sets_preserves_explicit_wildcard() {
        // Regression for B1-06: an explicit `allowed_tools: ['*']` followed by
        // `allowed_tool_sets` must NOT have the wildcard silently dropped. The
        // heuristic must check provenance, not value, and the new field makes
        // provenance observable.
        let agent = AgentDef::new("test", AgentMode::SubAgent)
            .with_allowed_tools(vec!["*".into()])
            .with_allowed_tool_sets(vec!["INVESTIGATION".into()]);
        assert_eq!(agent.allowed_tools, vec!["*"]);
        assert!(agent.allowed_tools_explicit);
    }

    #[test]
    fn test_with_max_iterations() {
        let agent = AgentDef::new("test", AgentMode::SubAgent).with_max_iterations(20);

        assert_eq!(agent.max_iterations, Some(20));
    }

    #[test]
    fn test_context_mode_default() {
        let agent = AgentDef::new("test", AgentMode::SubAgent);
        assert_eq!(agent.context_mode, ContextMode::Fresh);
        assert!(agent.token_budget.is_none());
        assert!(agent.model_hint.is_none());
    }

    #[test]
    fn test_with_context_mode() {
        let agent =
            AgentDef::new("test", AgentMode::SubAgent).with_context_mode(ContextMode::Summary);
        assert_eq!(agent.context_mode, ContextMode::Summary);
    }

    #[test]
    fn test_with_token_budget() {
        let agent = AgentDef::new("test", AgentMode::SubAgent).with_token_budget(50_000);
        assert_eq!(agent.token_budget, Some(50_000));
    }

    #[test]
    fn test_with_model_hint() {
        let agent = AgentDef::new("test", AgentMode::SubAgent).with_model_hint("fast");
        assert_eq!(agent.model_hint.as_deref(), Some("fast"));
    }

    #[test]
    fn test_provider_hint_default_none_and_setter() {
        let agent = AgentDef::new("test", AgentMode::SubAgent);
        assert!(agent.provider_hint.is_none());
        let agent = agent.with_provider_hint("openai");
        assert_eq!(agent.provider_hint.as_deref(), Some("openai"));
    }

    #[test]
    fn test_provider_hint_serde_back_compat() {
        // Legacy agent JSON with no `provider_hint` key still deserializes.
        let json = serde_json::to_string(&AgentDef::new("a", AgentMode::SubAgent)).unwrap();
        assert!(
            !json.contains("provider_hint"),
            "None must skip serialization"
        );
        let back: AgentDef = serde_json::from_str(&json).unwrap();
        assert!(back.provider_hint.is_none());
    }

    #[test]
    fn test_agent_def_description_default() {
        let agent = AgentDef::new("test", AgentMode::SubAgent);
        assert!(agent.description.is_empty());
        assert!(agent.when_to_use.is_none());
    }

    #[test]
    fn test_with_description() {
        let agent = AgentDef::new("test", AgentMode::SubAgent).with_description("A test agent");
        assert_eq!(agent.description, "A test agent");
    }

    #[test]
    fn test_with_when_to_use() {
        let agent =
            AgentDef::new("test", AgentMode::SubAgent).with_when_to_use("When you need testing");
        assert_eq!(agent.when_to_use.as_deref(), Some("When you need testing"));
    }

    // -- AgentSource (Stage E, P2 subagent uplift) ---------------------------

    #[test]
    fn agent_source_defaults_to_builtin() {
        assert_eq!(AgentSource::default(), AgentSource::Builtin);
    }

    #[test]
    fn agent_def_default_source_is_builtin() {
        let def = AgentDef::new("foo", AgentMode::SubAgent);
        assert_eq!(def.source, AgentSource::Builtin);
    }

    #[test]
    fn agent_source_serde_roundtrip() {
        for variant in [
            AgentSource::Builtin,
            AgentSource::User,
            AgentSource::Project,
            AgentSource::Plugin,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let back: AgentSource = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back);
        }
    }

    // -- Recursion guard (Stage B, P1 subagent uplift) -----------------------

    #[test]
    fn subagent_mode_denies_subagent_tool_even_with_wildcard() {
        let agent =
            AgentDef::new("child", AgentMode::SubAgent).with_allowed_tools(vec!["*".into()]);

        // Wildcard would normally allow everything, but the recursion guard
        // must override it for the `subagent` tool name.
        assert!(!agent.is_tool_allowed("subagent"));
        // Other tools are unaffected by the guard.
        assert!(agent.is_tool_allowed("read"));
    }

    #[test]
    fn subagent_mode_denies_subagent_tool_with_explicit_entry() {
        let agent = AgentDef::new("child", AgentMode::SubAgent)
            .with_allowed_tools(vec!["subagent".into(), "read".into()]);

        // Explicit allowlist entry must not bypass the recursion guard.
        assert!(!agent.is_tool_allowed("subagent"));
        // Unrelated explicit entries still pass.
        assert!(agent.is_tool_allowed("read"));
    }

    #[test]
    fn primary_mode_allows_subagent_tool() {
        let agent = AgentDef::new("primary", AgentMode::Primary)
            .with_allowed_tools(vec!["subagent".into()]);

        // Primary-mode agents retain full subagent spawning capability.
        assert!(agent.is_tool_allowed("subagent"));
    }

    // -- Named tool sets (Stage G, P2 subagent uplift) -----------------------

    #[test]
    fn is_tool_allowed_via_set_only() {
        let def = AgentDef::new("test", AgentMode::SubAgent)
            .with_allowed_tool_sets(vec!["READ_ONLY".into()]);
        assert!(def.is_tool_allowed("file_read"));
        assert!(def.is_tool_allowed("file_ops"));
        assert!(!def.is_tool_allowed("bash"));
        assert!(!def.is_tool_allowed("file_write"));
    }

    #[test]
    fn is_tool_allowed_set_and_flat_union() {
        let def = AgentDef::new("test", AgentMode::SubAgent)
            .with_allowed_tool_sets(vec!["READ_ONLY".into()])
            .with_allowed_tools(vec!["custom_tool".into()]);
        // Flat list contributes:
        assert!(def.is_tool_allowed("custom_tool"));
        // Set contributes:
        assert!(def.is_tool_allowed("file_read"));
        // Neither contributes:
        assert!(!def.is_tool_allowed("bash"));
    }

    #[test]
    fn denied_tools_overrides_set() {
        let def = AgentDef::new("test", AgentMode::SubAgent)
            .with_allowed_tool_sets(vec!["INVESTIGATION".into()])
            .with_denied_tools(vec!["web_fetch".into()]);
        // INVESTIGATION includes web_fetch but denied_tools wins:
        assert!(!def.is_tool_allowed("web_fetch"));
        // Other INVESTIGATION tools still allowed:
        assert!(def.is_tool_allowed("search"));
        assert!(def.is_tool_allowed("file_read"));
    }

    #[test]
    fn subagent_mode_denies_subagent_even_in_investigation_set() {
        let def = AgentDef::new("nested", AgentMode::SubAgent)
            .with_allowed_tool_sets(vec!["INVESTIGATION".into()]);
        // INVESTIGATION's "subagent" entry is overridden by Stage B mode-aware deny:
        assert!(!def.is_tool_allowed("subagent"));
        // Other INVESTIGATION members still allowed:
        assert!(def.is_tool_allowed("search"));
    }

    #[test]
    fn primary_mode_with_investigation_set_can_subagent() {
        let def = AgentDef::new("main", AgentMode::Primary)
            .with_allowed_tool_sets(vec!["INVESTIGATION".into()]);
        // Primary mode + INVESTIGATION → subagent allowed:
        assert!(def.is_tool_allowed("subagent"));
    }

    #[test]
    fn unknown_set_name_silently_empty() {
        let def = AgentDef::new("test", AgentMode::SubAgent)
            .with_allowed_tool_sets(vec!["NONEXISTENT_SET".into()])
            .with_allowed_tools(vec!["read_file".into()]);
        // Unknown set contributes nothing; flat list still works:
        assert!(def.is_tool_allowed("read_file"));
        assert!(!def.is_tool_allowed("grep"));
    }

    #[test]
    fn isolation_mode_serde_round_trip_worktree() {
        let mode = IsolationMode::Worktree;
        let json = serde_json::to_string(&mode).expect("serialize");
        assert_eq!(json, r#"{"kind":"worktree"}"#);
        let parsed: IsolationMode = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, IsolationMode::Worktree);
    }

    #[test]
    fn mcp_server_spec_inline_serde_round_trip() {
        let spec = McpServerSpec::Inline {
            name: "my-server".into(),
            config: McpInlineConfig {
                command: "node".into(),
                args: vec!["server.js".into()],
                env: std::collections::HashMap::new(),
            },
        };
        let json = serde_json::to_string(&spec).expect("serialize");
        assert!(json.contains(r#""type":"inline""#));
        assert!(json.contains(r#""name":"my-server""#));
        let parsed: McpServerSpec = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, spec);
    }

    #[test]
    fn mcp_server_spec_reference_serde_round_trip() {
        let spec = McpServerSpec::Reference {
            name: "github".into(),
        };
        let json = serde_json::to_string(&spec).expect("serialize");
        assert_eq!(json, r#"{"type":"reference","name":"github"}"#);
        let parsed: McpServerSpec = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, spec);
    }

    #[test]
    fn agent_def_default_mcp_servers_is_empty() {
        let def = AgentDef::new("test", AgentMode::SubAgent);
        assert!(
            def.mcp_servers.is_empty(),
            "default mcp_servers should be empty"
        );
    }

    #[test]
    fn agent_def_with_mcp_servers_roundtrip() {
        let specs = vec![
            McpServerSpec::Reference {
                name: "global-mcp".into(),
            },
            McpServerSpec::Inline {
                name: "fresh".into(),
                config: McpInlineConfig {
                    command: "echo".into(),
                    args: vec!["hi".into()],
                    env: Default::default(),
                },
            },
        ];
        let def = AgentDef::new("test", AgentMode::SubAgent).with_mcp_servers(specs.clone());
        assert_eq!(def.mcp_servers, specs);
    }
}
