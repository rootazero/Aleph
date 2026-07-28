//! Agent registry for managing agent definitions.

use crate::sync_primitives::RwLock;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tracing::warn;

use crate::agents::types::{AgentDef, AgentMode, ContextMode};

/// Plugin-shipped sub-agent definitions, published by
/// `ExtensionManager::load_all` after every (re)load and consumed lazily by
/// [`AgentRegistry::resolve`] (delegation) and the harness prompt builder
/// (`<available_agents>` catalog). Mirrors `utils::paths::PLUGIN_SKILL_DIRS`:
/// the extension manager owns the authoritative plugin definitions but runs
/// long after the registry's callers are constructed, so a process-global
/// publish is used instead of threading the list through every boot seam. Empty
/// until the first `load_all` — plugins aren't loaded before then and resolve /
/// catalog only run per-request afterwards. Lowest precedence: read only on a
/// registry miss (resolve) or insert-if-absent (catalog), so a builtin / user /
/// project agent of the same id always wins.
static PLUGIN_SUBAGENTS: OnceLock<RwLock<Arc<[AgentDef]>>> = OnceLock::new();

fn plugin_subagents_lock() -> &'static RwLock<Arc<[AgentDef]>> {
    PLUGIN_SUBAGENTS.get_or_init(|| RwLock::new(Arc::new([])))
}

/// Publish the installed plugins' sub-agent definitions for delegation +
/// catalog surfacing. Replaces the previous set wholesale (called after every
/// extension (re)load so the set stays in sync with what is installed).
pub fn publish_plugin_subagents(agents: Vec<AgentDef>) {
    let mut guard = plugin_subagents_lock()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = Arc::from(agents.into_boxed_slice());
}

/// Snapshot the currently-published plugin sub-agent definitions.
#[must_use]
pub fn plugin_subagents() -> Arc<[AgentDef]> {
    Arc::clone(
        &plugin_subagents_lock()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Registry for managing agent definitions
pub struct AgentRegistry {
    agents: RwLock<HashMap<String, AgentDef>>,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRegistry {
    /// Create a new empty registry
    #[must_use]
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    /// Create a registry with built-in agents
    #[must_use]
    pub fn with_builtins() -> Self {
        let registry = Self::new();
        for agent in builtin_agents() {
            registry.register(agent);
        }
        registry
    }

    /// Register an agent definition
    pub fn register(&self, agent: AgentDef) {
        // Recover from poisoned lock — the data is still valid after a panic
        let mut agents = self.agents.write().unwrap_or_else(|e| {
            warn!("AgentRegistry lock poisoned during register, recovering");
            e.into_inner()
        });
        agents.insert(agent.id.clone(), agent);
    }

    /// Get an agent by ID
    pub fn get(&self, id: &str) -> Option<AgentDef> {
        // Recover from poisoned lock — the data is still valid after a panic
        let agents = self.agents.read().unwrap_or_else(|e| {
            warn!("AgentRegistry lock poisoned during get, recovering");
            e.into_inner()
        });
        agents.get(id).cloned()
    }

    /// List all registered agent IDs (sorted for deterministic output)
    pub fn list_ids(&self) -> Vec<String> {
        let agents = self.agents.read().unwrap_or_else(|e| {
            warn!("AgentRegistry lock poisoned during list_ids, recovering");
            e.into_inner()
        });
        let mut ids: Vec<String> = agents.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Every `agent_type` / `agent_id` selector a caller may legitimately pass —
    /// registered ids ∪ plugin-shipped sub-agent ids, deduped and sorted.
    ///
    /// This is the set [`Self::resolve`] actually accepts, so it is what the
    /// "Unknown agent_type. Available agents: …" errors must print. Printing
    /// `list_ids()` alone told a model that a plugin agent the prompt's
    /// `<available_agents>` catalog advertises (plugin defs are folded in there
    /// too) does not exist — while a delegation to that very id would have
    /// succeeded.
    pub fn available_agent_ids(&self) -> Vec<String> {
        let mut ids = self.list_ids();
        ids.extend(plugin_subagents().iter().map(|a| a.id.clone()));
        ids.sort();
        ids.dedup();
        ids
    }

    /// List all sub-agents (excluding primary, sorted by id)
    pub fn list_subagents(&self) -> Vec<AgentDef> {
        let agents = self.agents.read().unwrap_or_else(|e| {
            warn!("AgentRegistry lock poisoned during list_subagents, recovering");
            e.into_inner()
        });
        let mut result: Vec<AgentDef> = agents
            .values()
            .filter(|a| a.mode == AgentMode::SubAgent)
            .cloned()
            .collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    /// Load agents from filesystem tiers and register non-builtin ones.
    ///
    /// Returns shadow events (id collisions where a higher-tier definition
    /// replaced a lower-tier one). Builtin-source agents in the merged set
    /// are skipped — they are already registered via `with_builtins()`.
    ///
    /// Boot wiring passes `project_dir = None` (the desktop daemon has no
    /// active project at boot). Per-run project overlay is exposed via
    /// [`Self::lookup_with_overlay`] — callers resolving an `agent_id` at
    /// run time consult that method instead of `get` so a project's
    /// `<project>/.aleph/agents/*.md` definitions can shadow the user-tier
    /// agents for runs scoped to that project, without reloading the
    /// global registry.
    pub fn register_from_dirs(
        &self,
        home_dir: &std::path::Path,
        project_dir: Option<&std::path::Path>,
    ) -> Result<Vec<crate::agents::loader::ShadowEvent>, crate::agents::loader::LoaderError> {
        let (agents, shadows) = crate::agents::loader::load_agents(home_dir, project_dir)?;
        for agent in agents
            .into_iter()
            .filter(|a| a.source != crate::agents::types::AgentSource::Builtin)
        {
            self.register(agent);
        }
        Ok(shadows)
    }

    /// Look up an agent by ID, consulting a per-project overlay first.
    ///
    /// When `project_root` is `Some`, the corresponding
    /// `<project_root>/.aleph/agents/{id}.md` shadows the globally
    /// registered agent for `id`. When `None`, when the project file
    /// does not exist, or when reading the overlay errors, falls
    /// through to [`Self::get`].
    ///
    /// The overlay is read lazily on each call — no caching. Project
    /// overlays are small (typically <10 agent files) and `scan_dir`
    /// only enumerates `.md` files, so the per-call IO cost stays well
    /// below a single LLM round-trip. If profiling later flags this
    /// path, the right answer is a small `Mutex<HashMap<PathBuf, …>>`
    /// keyed on the project root.
    pub fn lookup_with_overlay(
        &self,
        id: &str,
        project_root: Option<&std::path::Path>,
    ) -> Option<AgentDef> {
        if let Some(root) = project_root {
            if let Ok(overlay) = crate::agents::loader::load_project_overlay(root) {
                if let Some(def) = overlay.into_iter().find(|d| d.id == id) {
                    return Some(def);
                }
            }
        }
        self.get(id)
    }

    /// Resolve an `agent_type` selector to a definition, tolerating the
    /// vocabulary variations a model naturally emits.
    ///
    /// Resolution is two-pass and strictly non-destructive:
    ///   1. Exact match via [`Self::lookup_with_overlay`] — project overlays
    ///      and custom file-defined ids always win and behave identically to
    ///      before; case-sensitive, no aliasing.
    ///   2. On miss, canonicalize the selector ([`normalize_agent_alias`]:
    ///      trim + lowercase + alias table) and retry the lookup once.
    ///
    /// On a full registry + alias miss, a third pass consults the
    /// process-global plugin sub-agents ([`plugin_subagents`], published by
    /// `ExtensionManager::load_all`) by exact id — so a plugin-shipped agent is
    /// delegatable, but only when no builtin / user / project agent claims the
    /// id (plugin is lowest precedence).
    ///
    /// A genuinely unknown selector still returns `None`, so callers keep
    /// surfacing their "Unknown `agent_type`. Available: …" error. This closes
    /// the brittleness where a model picking `general-purpose`, `Explore`, or
    /// `planner` (Claude Code conventions) hard-errored instead of resolving
    /// to `default` / `explore` / `plan`. Mirrors the canonicalization the
    /// project already applies in [`crate::agents::thinking::normalize_think_level`]
    /// and `ProtocolAdapter::normalize_model_id` — boundary input normalization,
    /// not LLM reasoning (R7-safe).
    pub fn resolve(&self, id: &str, project_root: Option<&std::path::Path>) -> Option<AgentDef> {
        if let Some(def) = self.lookup_with_overlay(id, project_root) {
            return Some(def);
        }
        // Builtin-vocabulary alias retry (e.g. `planner` → `plan`). The alias
        // table only covers builtin ids, so this never re-routes a plugin/user
        // id — and it is tried before the plugin fallback so a plugin cannot
        // hijack a builtin synonym.
        if let Some(canonical) = normalize_agent_alias(id) {
            if canonical != id {
                if let Some(def) = self.lookup_with_overlay(canonical, project_root) {
                    return Some(def);
                }
            }
        }
        // Plugin-shipped sub-agents (lowest precedence): reached only after the
        // registry and alias table both miss, so a plugin agent adds a new
        // delegatable id but never shadows a builtin/user/project one.
        plugin_subagents().iter().find(|a| a.id == id).cloned()
    }

    /// Remove an agent by ID
    pub fn unregister(&self, id: &str) -> Option<AgentDef> {
        let mut agents = self.agents.write().unwrap_or_else(|e| {
            warn!("AgentRegistry lock poisoned during unregister, recovering");
            e.into_inner()
        });
        agents.remove(id)
    }
}

/// Canonicalize a built-in `agent_type` selector, tolerating casing and the
/// common synonyms a model trained on Claude Code conventions emits
/// (`general-purpose`, `Explore`, `planner`, …). Returns the canonical
/// built-in id, or `None` when the selector matches no known alias — in which
/// case [`AgentRegistry::resolve`] reports it as genuinely unknown rather than
/// masking a typo.
///
/// Only built-in ids are aliased; project/user-defined agents are matched
/// exactly in the first resolution pass and never routed through this table.
fn normalize_agent_alias(raw: &str) -> Option<&'static str> {
    // Lowercase once so both passes are case-insensitive — a model emitting the
    // Claude Code convention (`Explore`, `PLAN`, `Verify`) must still resolve to
    // the canonical built-in id. Arms return the `'static` spelling, never the
    // borrowed input (which is tied to the caller's `raw`).
    let lowered = raw.trim().to_ascii_lowercase();
    match lowered.as_str() {
        "default" => return Some("default"),
        "explore" => return Some("explore"),
        "plan" => return Some("plan"),
        "verify" => return Some("verify"),
        "researcher" => return Some("researcher"),
        "coder" => return Some("coder"),
        "main" => return Some("main"),
        _ => {}
    }
    match lowered.as_str() {
        "general" | "general-purpose" | "general_purpose" | "gp" => Some("default"),
        "explorer" | "exploration" => Some("explore"),
        "planner" | "planning" => Some("plan"),
        "verifier" | "verification" => Some("verify"),
        "research" => Some("researcher"),
        "code" | "coding" => Some("coder"),
        _ => None,
    }
}

#[must_use]
pub fn builtin_agents() -> Vec<AgentDef> {
    vec![
        // Main agent - full access. Explicit "flow_run" alongside "*" marks the
        // sub-flow dispatch tool as an intentional capability in the catalog
        // (registration wiring lands in Phase 6 — see orchestrator::flow_run_tool).
        // Hub tools (hub_*) are reachable via the wildcard; install safety is
        // enforced by the operator-tier gate (method_authz), not agent scoping.
        AgentDef::new("main", AgentMode::Primary)
            .with_description("Primary agent that responds directly to user")
            .with_allowed_tools(vec!["*".into(), "flow_run".into()]),
        // Explore agent — INVESTIGATION named set (P2 Stage G demo migration).
        // Effective behavior unchanged: Stage B recursion guard blocks subagent
        // for SubAgent mode; denied_tools preserved. Default wildcard cleared so
        // only INVESTIGATION tools are allowed.
        AgentDef::new("explore", AgentMode::SubAgent)
            .with_description("Read-only codebase exploration specialist")
            .with_when_to_use(
                "When you need to search, read, or understand code without modifying anything",
            )
            .with_prompt_sections(vec!["explore_constraints".into()])
            .with_allowed_tool_sets(vec!["INVESTIGATION".into()])
            .with_denied_tools(vec!["file_write".into(), "file_edit".into(), "bash".into()])
            .with_max_iterations(20),
        // Loop-auditor — the governance layer's independent evidence collector
        // (spec 2026-07-19-graph-multiagent-fusion). Fresh context by design:
        // an auditor that shares the auditee's memory/context can only confirm
        // the auditee's own story ("agents reading the same data prove each
        // other right"). Can measure (bash → real exit codes / counts,
        // governance_metrics → the standing corrections/dreaming probes the
        // audit template orders — bash cannot reach the sandbox-walled
        // ~/.aleph/data, which is why the tool exists) and read; cannot
        // rewrite; network search denied per audit doctrine.
        AgentDef::new("loop-auditor", AgentMode::SubAgent)
            .with_description("Independent-context evidence collector for governance loops")
            .with_when_to_use(
                "When an audit/watcher governance turn needs independently gathered \
                 evidence: run anchor probes, re-measure claimed numbers, verify a \
                 reviewed deliverable against reality. Read-and-measure only.",
            )
            .with_context_mode(ContextMode::Fresh)
            .with_allowed_tool_sets(vec!["READ_ONLY".into()])
            .with_allowed_tools(vec!["bash".into(), "governance_metrics".into()])
            .with_denied_tools(vec![
                "file_write".into(),
                "file_edit".into(),
                "search".into(),
                "web_fetch".into(),
            ])
            .with_max_iterations(15),
        // Coder agent - file operations
        AgentDef::new("coder", AgentMode::SubAgent)
            .with_description("Code writing specialist with file operations")
            .with_when_to_use("When you need to write, edit, or create code files")
            .with_prompt_sections(vec!["coder_guidelines".into()])
            .with_allowed_tools(vec![
                "file_read".into(),
                "file_write".into(),
                "file_edit".into(),
                "file_ops".into(),
                "bash".into(),
            ])
            .with_max_iterations(30),
        // Researcher agent - search and web
        AgentDef::new("researcher", AgentMode::SubAgent)
            .with_description("Web and document research specialist")
            .with_when_to_use(
                "When you need to search the web, fetch URLs, or gather external information",
            )
            .with_prompt_sections(vec!["researcher_protocol".into()])
            .with_allowed_tools(vec![
                "search".into(),
                "web_fetch".into(),
                "file_read".into(),
            ])
            .with_denied_tools(vec!["file_write".into(), "file_edit".into(), "bash".into()])
            .with_max_iterations(15),
        // Default agent - general-purpose sub-agent
        AgentDef::new("default", AgentMode::SubAgent)
            .with_description("General-purpose sub-agent")
            .with_when_to_use("When no specialized agent fits the task")
            .with_context_mode(ContextMode::Summary),
        // Plan agent - read-only planner
        AgentDef::new("plan", AgentMode::SubAgent)
            .with_description("Read-only planning and analysis specialist")
            .with_when_to_use(
                "When you need to analyze requirements, design architecture, or create plans",
            )
            .with_prompt_sections(vec!["plan_protocol".into()])
            .with_allowed_tools(vec!["file_ops".into(), "file_read".into(), "bash".into()])
            .with_denied_tools(vec!["file_write".into(), "file_edit".into()])
            .with_max_iterations(20)
            .with_context_mode(ContextMode::Summary),
        // Verify agent - adversarial verifier (read-only).
        // Hub tools are denied explicitly so the wildcard cannot reach them —
        // verify must not install/mutate; denied_tools short-circuits the "*" wildcard.
        AgentDef::new("verify", AgentMode::SubAgent)
            .with_description("Adversarial verification specialist")
            .with_when_to_use("When you need to independently verify that work was done correctly")
            .with_prompt_sections(vec!["verify_protocol".into()])
            .with_allowed_tools(vec!["*".into()])
            .with_denied_tools(vec![
                "file_write".into(),
                "file_edit".into(),
                "hub_catalog_sync".into(),
                "hub_fetch_docs".into(),
                "hub_resolve_spec".into(),
                "hub_install_run".into(),
                "hub_install_verify".into(),
            ])
            .with_max_iterations(25)
            .with_context_mode(ContextMode::Summary),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_new() {
        let registry = AgentRegistry::new();
        assert!(registry.list_ids().is_empty());
    }

    #[test]
    fn test_registry_register_and_get() {
        let registry = AgentRegistry::new();
        let agent = AgentDef::new("test", AgentMode::SubAgent);

        registry.register(agent);

        let retrieved = registry.get("test").unwrap();
        assert_eq!(retrieved.id, "test");
    }

    #[test]
    fn test_registry_get_nonexistent() {
        let registry = AgentRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_list_ids() {
        let registry = AgentRegistry::new();
        registry.register(AgentDef::new("a", AgentMode::SubAgent));
        registry.register(AgentDef::new("b", AgentMode::SubAgent));

        let ids = registry.list_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"a".to_string()));
        assert!(ids.contains(&"b".to_string()));
    }

    #[test]
    fn test_registry_list_subagents() {
        let registry = AgentRegistry::new();
        registry.register(AgentDef::new("main", AgentMode::Primary));
        registry.register(AgentDef::new("explore", AgentMode::SubAgent));
        registry.register(AgentDef::new("coder", AgentMode::SubAgent));

        let subagents = registry.list_subagents();
        assert_eq!(subagents.len(), 2);
        assert!(subagents.iter().all(|a| a.mode == AgentMode::SubAgent));
    }

    #[test]
    fn test_registry_unregister() {
        let registry = AgentRegistry::new();
        registry.register(AgentDef::new("test", AgentMode::SubAgent));

        let removed = registry.unregister("test");
        assert!(removed.is_some());
        assert!(registry.get("test").is_none());
    }

    #[test]
    fn test_with_builtins() {
        let registry = AgentRegistry::with_builtins();

        assert!(registry.get("main").is_some());
        assert!(registry.get("explore").is_some());
        assert!(registry.get("coder").is_some());
        assert!(registry.get("researcher").is_some());
    }

    #[test]
    fn test_builtin_agents_count() {
        let agents = builtin_agents();
        assert_eq!(agents.len(), 8);
        assert!(
            agents.iter().all(|a| a.id != "store"),
            "store agent must be retired"
        );
    }

    #[test]
    fn test_explore_agent_config() {
        let registry = AgentRegistry::with_builtins();
        let explore = registry.get("explore").unwrap();

        assert_eq!(explore.mode, AgentMode::SubAgent);
        // INVESTIGATION set members (canonical builtin names):
        assert!(explore.is_tool_allowed("file_read"));
        assert!(explore.is_tool_allowed("file_ops"));
        assert!(explore.is_tool_allowed("search"));
        // denied_tools entries — write-side builtins must be blocked:
        assert!(!explore.is_tool_allowed("file_write"));
        assert!(!explore.is_tool_allowed("file_edit"));
        assert!(!explore.is_tool_allowed("bash"));
        assert_eq!(explore.max_iterations, Some(20));
    }

    #[test]
    fn test_coder_agent_config() {
        let registry = AgentRegistry::with_builtins();
        let coder = registry.get("coder").unwrap();

        assert!(coder.is_tool_allowed("file_write"));
        assert!(coder.is_tool_allowed("file_edit"));
        assert!(coder.is_tool_allowed("bash"));
        assert_eq!(coder.max_iterations, Some(30));
    }

    #[test]
    fn test_researcher_agent_config() {
        let registry = AgentRegistry::with_builtins();
        let researcher = registry.get("researcher").unwrap();

        assert!(researcher.is_tool_allowed("search"));
        assert!(researcher.is_tool_allowed("web_fetch"));
        assert!(!researcher.is_tool_allowed("file_write"));
        assert_eq!(researcher.max_iterations, Some(15));
    }

    #[test]
    fn test_verify_agent_config() {
        let registry = AgentRegistry::with_builtins();
        let verify = registry.get("verify").unwrap();
        assert_eq!(verify.mode, AgentMode::SubAgent);
        assert!(verify.is_tool_allowed("file_ops"));
        assert!(verify.is_tool_allowed("bash"));
        assert!(!verify.is_tool_allowed("file_write")); // read-only: write denied
        assert!(!verify.is_tool_allowed("file_edit")); // read-only: edit denied
        assert_eq!(verify.max_iterations, Some(25));
        assert_eq!(verify.prompt_sections, vec!["verify_protocol"]);
    }

    #[test]
    fn test_builtin_subagents_have_descriptions() {
        let registry = AgentRegistry::with_builtins();
        let subagents = registry.list_subagents();
        for agent in &subagents {
            assert!(
                !agent.description.is_empty(),
                "Agent '{}' should have a description",
                agent.id
            );
        }
    }

    #[test]
    fn test_builtin_subagents_have_when_to_use() {
        let registry = AgentRegistry::with_builtins();
        let subagents = registry.list_subagents();
        for agent in &subagents {
            assert!(
                agent.when_to_use.is_some(),
                "Agent '{}' should have when_to_use",
                agent.id
            );
        }
    }

    #[test]
    fn test_builtin_agents_have_prompt_sections() {
        let agents = builtin_agents();
        let by_id: std::collections::HashMap<&str, &AgentDef> =
            agents.iter().map(|a| (a.id.as_str(), a)).collect();

        // Main has no sections (uses default prompt builder flow)
        assert!(by_id["main"].prompt_sections.is_empty());

        // Sub-agents each have their specific section
        assert_eq!(
            by_id["explore"].prompt_sections,
            vec!["explore_constraints"]
        );
        assert_eq!(by_id["coder"].prompt_sections, vec!["coder_guidelines"]);
        assert_eq!(
            by_id["researcher"].prompt_sections,
            vec!["researcher_protocol"]
        );
        assert_eq!(by_id["verify"].prompt_sections, vec!["verify_protocol"]);
    }

    // --- lookup_with_overlay (R3 per-project overlay) ----------------------

    /// Helper: write `<dir>/.aleph/agents/<id>.md` with frontmatter matching
    /// `loader::UserFrontmatter` so `load_project_overlay` can parse it.
    fn write_project_agent(dir: &std::path::Path, id: &str, description: &str) {
        let agents_dir = dir.join(".aleph/agents");
        std::fs::create_dir_all(&agents_dir).expect("create .aleph/agents");
        let path = agents_dir.join(format!("{id}.md"));
        let content = format!(
            "---\nid: {id}\ndescription: \"{description}\"\nwhen_to_use: \"test\"\n---\n\nbody\n"
        );
        std::fs::write(&path, content).expect("write agent md");
    }

    #[test]
    fn lookup_with_overlay_no_project_root_falls_through_to_get() {
        let registry = AgentRegistry::with_builtins();
        let resolved = registry.lookup_with_overlay("explore", None).unwrap();
        // Builtin description, not a project override.
        assert_eq!(
            resolved.description,
            "Read-only codebase exploration specialist"
        );
    }

    #[test]
    fn lookup_with_overlay_project_agent_shadows_builtin() {
        let registry = AgentRegistry::with_builtins();
        let project = tempfile::tempdir().unwrap();
        write_project_agent(project.path(), "explore", "project-tuned explore");

        let resolved = registry
            .lookup_with_overlay("explore", Some(project.path()))
            .unwrap();
        assert_eq!(resolved.description, "project-tuned explore");
        assert_eq!(resolved.source, crate::agents::types::AgentSource::Project);
    }

    #[test]
    fn lookup_with_overlay_missing_project_file_falls_through() {
        let registry = AgentRegistry::with_builtins();
        let project = tempfile::tempdir().unwrap();
        // Project dir exists but has no .aleph/agents/explore.md — fall through.
        let resolved = registry
            .lookup_with_overlay("explore", Some(project.path()))
            .unwrap();
        assert_eq!(
            resolved.description,
            "Read-only codebase exploration specialist"
        );
    }

    #[test]
    fn lookup_with_overlay_unknown_id_returns_none() {
        let registry = AgentRegistry::with_builtins();
        let project = tempfile::tempdir().unwrap();
        assert!(registry
            .lookup_with_overlay("nonexistent_agent_id", Some(project.path()))
            .is_none());
    }

    #[test]
    fn lookup_with_overlay_project_agent_does_not_pollute_global_registry() {
        // Read-only overlay — must not insert into the long-lived registry.
        let registry = AgentRegistry::with_builtins();
        let project = tempfile::tempdir().unwrap();
        write_project_agent(project.path(), "explore", "project-tuned explore");

        let _ = registry.lookup_with_overlay("explore", Some(project.path()));

        // Direct get() must still return the builtin — registry untouched.
        let global = registry.get("explore").unwrap();
        assert_eq!(
            global.description,
            "Read-only codebase exploration specialist"
        );
    }

    // --- resolve (forgiving agent_type canonicalization) -------------------

    #[test]
    fn resolve_exact_match_behaves_like_lookup() {
        let registry = AgentRegistry::with_builtins();
        // Exact, canonical ids resolve unchanged.
        for id in [
            "explore",
            "plan",
            "verify",
            "researcher",
            "coder",
            "default",
        ] {
            assert_eq!(registry.resolve(id, None).unwrap().id, id, "exact {id}");
        }
    }

    #[test]
    fn resolve_is_case_insensitive_for_builtins() {
        let registry = AgentRegistry::with_builtins();
        assert_eq!(registry.resolve("Explore", None).unwrap().id, "explore");
        assert_eq!(registry.resolve("PLAN", None).unwrap().id, "plan");
        assert_eq!(registry.resolve("  Verify ", None).unwrap().id, "verify");
    }

    #[test]
    fn resolve_maps_claude_code_aliases() {
        let registry = AgentRegistry::with_builtins();
        // The vocabulary a model trained on Claude Code naturally emits.
        assert_eq!(
            registry.resolve("general-purpose", None).unwrap().id,
            "default"
        );
        assert_eq!(
            registry.resolve("general_purpose", None).unwrap().id,
            "default"
        );
        assert_eq!(registry.resolve("planner", None).unwrap().id, "plan");
        assert_eq!(registry.resolve("verification", None).unwrap().id, "verify");
        assert_eq!(registry.resolve("explorer", None).unwrap().id, "explore");
        assert_eq!(registry.resolve("research", None).unwrap().id, "researcher");
        assert_eq!(registry.resolve("coding", None).unwrap().id, "coder");
    }

    #[test]
    fn resolve_genuinely_unknown_still_returns_none() {
        // A real typo / unknown selector must NOT be silently coerced — the
        // caller keeps surfacing the "Unknown agent_type. Available: …" error.
        let registry = AgentRegistry::with_builtins();
        assert!(registry.resolve("nonexistent_agent", None).is_none());
        assert!(registry.resolve("explorez", None).is_none());
    }

    #[test]
    fn resolve_exact_match_wins_over_alias_via_project_overlay() {
        // A project-defined `explore` is matched exactly in pass 1 and must
        // never be bypassed by the alias table.
        let registry = AgentRegistry::with_builtins();
        let project = tempfile::tempdir().unwrap();
        write_project_agent(project.path(), "explore", "project-tuned explore");
        assert_eq!(
            registry
                .resolve("explore", Some(project.path()))
                .unwrap()
                .description,
            "project-tuned explore"
        );
    }

    #[test]
    fn resolve_falls_back_to_plugin_subagents_but_registry_wins() {
        use crate::agents::types::AgentSource;
        // Publish a unique plugin sub-agent plus one that collides with the
        // builtin `explore`. Process-global, but unique ids keep this from
        // perturbing parallel tests, and `explore` always resolves to the
        // builtin (registry wins) regardless of what is published.
        let mut plugin_explore = AgentDef::new("explore", AgentMode::SubAgent);
        plugin_explore.description = "PLUGIN explore (must be shadowed)".into();
        plugin_explore.source = AgentSource::Plugin;
        publish_plugin_subagents(vec![
            AgentDef::new("plugin-delegate-xyz", AgentMode::SubAgent),
            plugin_explore,
        ]);

        let registry = AgentRegistry::with_builtins();

        // A new plugin-only id is delegatable via the third-pass fallback.
        assert_eq!(
            registry.resolve("plugin-delegate-xyz", None).unwrap().id,
            "plugin-delegate-xyz"
        );

        // A plugin agent NEVER shadows a builtin of the same id.
        assert_eq!(
            registry.resolve("explore", None).unwrap().description,
            "Read-only codebase exploration specialist",
            "builtin explore must win over a plugin `explore`"
        );

        // Genuinely-unknown still None even with plugins published.
        assert!(registry.resolve("no-such-agent-at-all", None).is_none());

        // The "Available agents: …" faces must offer the same set `resolve`
        // accepts — a delegatable plugin id must never be reported as
        // nonexistent — and a shadowed collision appears exactly once.
        let available = registry.available_agent_ids();
        assert!(
            available.contains(&"plugin-delegate-xyz".to_string()),
            "available ids must include delegatable plugin agents: {available:?}"
        );
        assert_eq!(
            available.iter().filter(|id| *id == "explore").count(),
            1,
            "a plugin id colliding with a builtin must not be listed twice"
        );
        assert!(
            available.windows(2).all(|w| w[0] <= w[1]),
            "available ids must be sorted for deterministic error text"
        );

        // Hygiene: clear the process-global for other tests.
        publish_plugin_subagents(Vec::new());
    }

    #[test]
    fn main_allows_hub_tools_verify_denies() {
        let agents = builtin_agents();
        let main = agents.iter().find(|a| a.id == "main").unwrap();
        let verify = agents.iter().find(|a| a.id == "verify").unwrap();

        // Hub tools are demoted to the main loop; the read-only verifier still denies them.
        let hub_tools = [
            "hub_catalog_sync",
            "hub_fetch_docs",
            "hub_resolve_spec",
            "hub_install_run",
            "hub_install_verify",
        ];
        for name in &hub_tools {
            assert!(
                main.is_tool_allowed(name),
                "main should allow hub tool: {name}"
            );
            assert!(
                !verify.is_tool_allowed(name),
                "verify (read-only) should deny hub tool: {name}"
            );
        }

        // Unrelated tools must still be allowed on main.
        assert!(main.is_tool_allowed("flow_run"));
    }

    #[test]
    fn loop_auditor_is_independent_and_measure_only() {
        let agents = builtin_agents();
        let auditor = agents
            .iter()
            .find(|a| a.id == "loop-auditor")
            .expect("loop-auditor builtin must exist");
        assert!(matches!(auditor.mode, AgentMode::SubAgent));
        // Independent context is the whole point of this agent.
        assert!(matches!(auditor.context_mode, ContextMode::Fresh));
        // Can measure (bash for real exit codes) and read, cannot rewrite.
        assert!(auditor.is_tool_allowed("bash"));
        assert!(auditor.is_tool_allowed("file_read"));
        // The audit/watch templates order governance_metrics probes; bash
        // cannot reach ~/.aleph/data (sandbox), so the read-only tool must be
        // reachable or the auditor fails its own standing probes.
        assert!(auditor.is_tool_allowed("governance_metrics"));
        assert!(!auditor.is_tool_allowed("file_write"));
        assert!(!auditor.is_tool_allowed("file_edit"));
        // Audit doctrine forbids network search — deny at the definition level.
        assert!(!auditor.is_tool_allowed("search"));
        assert!(!auditor.is_tool_allowed("web_fetch"));
        // SubAgent mode structurally cannot recurse.
        assert!(!auditor.is_tool_allowed("subagent"));
    }
}
