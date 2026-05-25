//! Agent registry for managing agent definitions.

use crate::sync_primitives::RwLock;
use std::collections::HashMap;

use crate::agents::types::{AgentDef, AgentMode, ContextMode};

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
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    /// Create a registry with built-in agents
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
        let mut agents = self.agents.write().unwrap_or_else(|e| e.into_inner());
        agents.insert(agent.id.clone(), agent);
    }

    /// Get an agent by ID
    pub fn get(&self, id: &str) -> Option<AgentDef> {
        // Recover from poisoned lock — the data is still valid after a panic
        let agents = self.agents.read().unwrap_or_else(|e| e.into_inner());
        agents.get(id).cloned()
    }

    /// List all registered agent IDs (sorted for deterministic output)
    pub fn list_ids(&self) -> Vec<String> {
        let agents = self.agents.read().unwrap_or_else(|e| e.into_inner());
        let mut ids: Vec<String> = agents.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// List all sub-agents (excluding primary, sorted by id)
    pub fn list_subagents(&self) -> Vec<AgentDef> {
        let agents = self.agents.read().unwrap_or_else(|e| e.into_inner());
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

    /// Remove an agent by ID
    pub fn unregister(&self, id: &str) -> Option<AgentDef> {
        let mut agents = self.agents.write().unwrap_or_else(|e| e.into_inner());
        agents.remove(id)
    }
}

/// Returns the built-in agent definitions
pub fn builtin_agents() -> Vec<AgentDef> {
    vec![
        // Main agent - full access. Explicit "flow_run" alongside "*" marks the
        // sub-flow dispatch tool as an intentional capability in the catalog
        // (registration wiring lands in Phase 6 — see orchestrator::flow_run_tool).
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
        // Coder agent - file operations
        AgentDef::new("coder", AgentMode::SubAgent)
            .with_description("Code writing specialist with file operations")
            .with_when_to_use("When you need to write, edit, or create code files")
            .with_prompt_sections(vec!["coder_guidelines".into()])
            .with_allowed_tools(vec![
                "read_file".into(),
                "write_file".into(),
                "edit_file".into(),
                "glob".into(),
                "grep".into(),
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
                "read_file".into(),
            ])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into(), "bash".into()])
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
            .with_allowed_tools(vec![
                "glob".into(),
                "grep".into(),
                "read_file".into(),
                "bash".into(),
            ])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into()])
            .with_max_iterations(20)
            .with_context_mode(ContextMode::Summary),
        // Verify agent - adversarial verifier (read-only)
        AgentDef::new("verify", AgentMode::SubAgent)
            .with_description("Adversarial verification specialist")
            .with_when_to_use("When you need to independently verify that work was done correctly")
            .with_prompt_sections(vec!["verify_protocol".into()])
            .with_allowed_tools(vec!["*".into()])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into()])
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
        assert_eq!(agents.len(), 7);
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

        assert!(coder.is_tool_allowed("write_file"));
        assert!(coder.is_tool_allowed("edit_file"));
        assert!(coder.is_tool_allowed("bash"));
        assert_eq!(coder.max_iterations, Some(30));
    }

    #[test]
    fn test_researcher_agent_config() {
        let registry = AgentRegistry::with_builtins();
        let researcher = registry.get("researcher").unwrap();

        assert!(researcher.is_tool_allowed("search"));
        assert!(researcher.is_tool_allowed("web_fetch"));
        assert!(!researcher.is_tool_allowed("write_file"));
        assert_eq!(researcher.max_iterations, Some(15));
    }

    #[test]
    fn test_verify_agent_config() {
        let registry = AgentRegistry::with_builtins();
        let verify = registry.get("verify").unwrap();
        assert_eq!(verify.mode, AgentMode::SubAgent);
        assert!(verify.is_tool_allowed("glob"));
        assert!(verify.is_tool_allowed("bash"));
        assert!(!verify.is_tool_allowed("write_file")); // read-only: write denied
        assert!(!verify.is_tool_allowed("edit_file")); // read-only: edit denied
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
}
