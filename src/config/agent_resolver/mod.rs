//! Agent Definition Resolver
//!
//! Merges agent configuration entries, profiles, and workspace files into
//! fully resolved agent definitions ready for runtime use.
//!
//! The resolver handles:
//! - Workspace path resolution (explicit path > auto-layout)
//! - Profile inheritance
//! - Model/skills cascading (agent > defaults > profile > hardcoded)
//! - Workspace initialization (directory creation + template files)
//! - Loading SOUL.md, AGENTS.md, MEMORY.md from workspaces

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::types::agents_def::{
    AgentDefaults, AgentDefinition, AgentModelRef, AgentsConfig, SubagentPolicy,
};
use crate::config::types::profile::ProfileConfig;
use crate::config::types::provider::ProviderConfig;
use crate::gateway::identity_loader::IdentityFileLoader;
use crate::thinker::soul_archetypes::SoulArchetype;

mod templates;

// =============================================================================
// Constants
// =============================================================================

/// Fallback model when no model is specified at any level.
/// Empty string signals the provider registry to use the provider's own default model.
const DEFAULT_MODEL: &str = "";

// =============================================================================
// Model Reference Validation
// =============================================================================

/// Resolve an [`AgentModelRef`] to a final model string, or `None` if unavailable.
///
/// - `Legacy(s)` — always returns `Some(s)` (free-form string, no validation).
/// - `Qualified{provider, model}` — returns `Some(model)` only when the provider
///   exists, is enabled, and the model is listed in `provider.models`. Otherwise
///   returns `None` and emits a warning so the caller falls back to the system
///   default chain.
pub(crate) fn resolve_model_ref(
    m: &AgentModelRef,
    providers: &HashMap<String, ProviderConfig>,
) -> Option<String> {
    match m {
        AgentModelRef::Legacy(s) => Some(s.clone()),
        AgentModelRef::Qualified { provider, model } => {
            let available = providers
                .get(provider)
                .is_some_and(|p| p.enabled && p.models.iter().any(|x| x == model));
            if available {
                Some(model.clone())
            } else {
                tracing::warn!(
                    provider = %provider,
                    model = %model,
                    "selected agent model unavailable (provider removed/disabled or model dropped), \
                     falling back to system default"
                );
                None
            }
        }
    }
}

// =============================================================================
// ResolvedAgent
// =============================================================================

/// A fully resolved agent definition ready for runtime use.
///
/// All optional fields from `AgentDefinition` have been resolved by cascading
/// through defaults, profile, and hardcoded fallbacks.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    /// Unique agent identifier
    pub id: String,

    /// Human-readable display name
    pub name: String,

    /// Whether this is the default agent
    pub is_default: bool,

    /// Resolved workspace directory path
    pub workspace_path: PathBuf,

    /// Resolved agent state directory path
    pub agent_dir: PathBuf,

    /// Resolved profile configuration
    pub profile: ProfileConfig,

    /// Raw SOUL.md content (if present in workspace)
    pub soul_md: Option<String>,

    /// Raw AGENTS.md content (if present in workspace)
    pub agents_md: Option<String>,

    /// Resolved AI model identifier
    pub model: String,

    /// Resolved list of allowed skills
    pub skills: Vec<String>,

    /// Resolved list of blocked skills/tools
    pub skills_blacklist: Vec<String>,

    /// Sub-agent spawning policy
    pub subagent_policy: Option<SubagentPolicy>,

    /// Link access whitelist (None or empty = all links allowed)
    pub allowed_links: Option<Vec<String>>,

    /// Per-agent tool permission overrides
    pub tool_permissions: Option<crate::config::types::policies::ToolPermissionsConfig>,
}

// =============================================================================
// AgentDefinitionResolver
// =============================================================================

/// Resolves agent definitions from configuration into runtime-ready structs.
///
/// Merges `AgentDefinition` entries with `AgentDefaults`, `ProfileConfig`,
/// and workspace files (SOUL.md, AGENTS.md) to produce fully resolved
/// `ResolvedAgent` instances. MEMORY.md is loaded separately by the
/// curated memory module — the resolver does not touch it.
#[derive(Default)]
pub struct AgentDefinitionResolver {
    identity_loader: IdentityFileLoader,
}

impl AgentDefinitionResolver {
    /// Create a new resolver with a fresh workspace file loader.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve all agent definitions from configuration.
    ///
    /// If `config.list` is empty, a default "main" agent is created via
    /// `AgentsConfig::ensure_default()` on a cloned config.
    pub fn resolve_all(
        &mut self,
        config: &AgentsConfig,
        profiles: &HashMap<String, ProfileConfig>,
        providers: &HashMap<String, ProviderConfig>,
    ) -> Vec<ResolvedAgent> {
        // Only clone if we need to inject a default agent
        let owned;
        let effective = if config.list.is_empty() {
            owned = {
                let mut c = config.clone();
                c.ensure_default();
                c
            };
            &owned
        } else {
            config
        };

        // Validate: warn on duplicate IDs and multiple defaults
        let mut seen_ids = HashSet::new();
        let default_count = effective.list.iter().filter(|a| a.default).count();
        if default_count > 1 {
            tracing::warn!("Multiple agents marked as default, using the first one");
        }

        effective
            .list
            .iter()
            .filter(|agent_def| {
                if seen_ids.insert(&agent_def.id) {
                    true
                } else {
                    tracing::warn!(agent_id = %agent_def.id, "Duplicate agent ID, skipping");
                    false
                }
            })
            .map(|agent_def| self.resolve_one(agent_def, &effective.defaults, profiles, providers))
            .collect()
    }

    /// Resolve workspace path for an agent.
    ///
    /// Workspace directory name is always equal to `agent_id` (1:1 binding).
    /// The `AgentDefinition.workspace` field is deprecated and ignored.
    pub fn resolve_workspace_path(
        &self,
        agent: &AgentDefinition,
        defaults: &AgentDefaults,
    ) -> PathBuf {
        // Warn if explicit workspace is set (deprecated, ignored)
        if agent.workspace.is_some() {
            let root = defaults
                .workspace_root
                .as_ref()
                .map_or_else(default_workspace_root, |p| resolve_user_path(p));
            tracing::warn!(
                "Agent '{}': explicit workspace path is deprecated, using {}/{}",
                agent.id,
                root.display(),
                agent.id
            );
        }

        // Enforce 1:1 binding: workspace dir = agent_id
        let root = defaults
            .workspace_root
            .as_ref()
            .map_or_else(default_workspace_root, |p| resolve_user_path(p));

        root.join(&agent.id)
    }

    /// Resolve the agent state directory path.
    pub fn resolve_agent_dir(&self, agent: &AgentDefinition, defaults: &AgentDefaults) -> PathBuf {
        let root = defaults
            .agents_root
            .as_ref()
            .map_or_else(default_agents_root, |p| resolve_user_path(p));
        root.join(&agent.id)
    }

    /// Resolve a single agent definition into a `ResolvedAgent`.
    ///
    /// Public for hot registration: the `agents.create` RPC resolves the new
    /// definition through the exact same path boot's [`Self::resolve_all`]
    /// uses, so a created agent is immediately routable without a restart.
    pub fn resolve_one(
        &mut self,
        agent: &AgentDefinition,
        defaults: &AgentDefaults,
        profiles: &HashMap<String, ProfileConfig>,
        providers: &HashMap<String, ProviderConfig>,
    ) -> ResolvedAgent {
        // 1. Resolve workspace path and agent state directory
        let workspace_path = self.resolve_workspace_path(agent, defaults);
        let agent_dir = self.resolve_agent_dir(agent, defaults);

        // 2. Initialize agent identity directory (SOUL.md, AGENTS.md, etc.)
        let agent_name = agent.name.as_deref().unwrap_or(&agent.id);
        if let Err(e) = initialize_agent_dir(&agent_dir) {
            tracing::warn!(
                agent_id = %agent.id,
                path = %agent_dir.display(),
                error = %e,
                "Failed to initialize agent state directory"
            );
        }
        // 2b. Ensure workspace directory exists (for project files)
        if let Err(e) = fs::create_dir_all(&workspace_path) {
            tracing::warn!(
                agent_id = %agent.id,
                path = %workspace_path.display(),
                error = %e,
                "Failed to create workspace directory"
            );
        }
        // Identity files (SOUL.md, AGENTS.md, etc.) go in agent_dir
        if let Err(e) =
            initialize_agent_identity(&agent_dir, agent_name, agent.archetype.unwrap_or_default())
        {
            tracing::warn!(
                agent_id = %agent.id,
                path = %agent_dir.display(),
                error = %e,
                "Failed to initialize agent identity files"
            );
        }

        // 2c. Lazy migration: move sessions/ from workspace to agent_dir if needed
        let old_sessions = workspace_path.join("sessions");
        if old_sessions.is_symlink() {
            tracing::warn!(
                agent_id = %agent.id,
                path = %old_sessions.display(),
                "Skipping session migration: sessions/ is a symlink"
            );
        } else if old_sessions.is_dir() && !agent_dir.join("sessions").join(".migrated").exists() {
            // Check if there are actual session files to migrate
            let has_files =
                fs::read_dir(&old_sessions).is_ok_and(|mut entries| entries.next().is_some());
            if has_files {
                let new_sessions = agent_dir.join("sessions");
                tracing::info!(
                    agent_id = %agent.id,
                    "Migrating sessions/ from workspace to agent state directory"
                );
                // Copy files from old to new (rename may fail across filesystems).
                // The originals are deleted ONLY if every entry copied successfully —
                // a missing destination dir or an un-copyable entry (e.g. a nested
                // subdirectory, which fs::copy cannot handle) must never lead to
                // deleting the source before its contents are safely relocated.
                let mut all_copied = fs::create_dir_all(&new_sessions).is_ok();
                if all_copied {
                    match fs::read_dir(&old_sessions) {
                        Ok(entries) => {
                            for entry in entries.flatten() {
                                let dest = new_sessions.join(entry.file_name());
                                if dest.exists() {
                                    continue;
                                }
                                let is_file = entry.file_type().is_ok_and(|t| t.is_file());
                                if !is_file || fs::copy(entry.path(), &dest).is_err() {
                                    all_copied = false;
                                }
                            }
                        }
                        Err(_) => all_copied = false,
                    }
                }
                if all_copied {
                    // Remove old sessions dir only after a fully successful migration
                    let _ = fs::remove_dir_all(&old_sessions);
                    // Mark migration as complete to prevent re-processing
                    let _ = fs::write(new_sessions.join(".migrated"), "");
                } else {
                    tracing::warn!(
                        agent_id = %agent.id,
                        "Session migration incomplete; leaving original sessions/ in place to avoid data loss"
                    );
                }
            }
        }

        // 3. Load ProfileConfig from profiles HashMap
        let profile = agent
            .profile
            .as_ref()
            .and_then(|name| profiles.get(name))
            .cloned()
            .unwrap_or_default();

        // 4. Resolve model: an invalidated Qualified selection → treat as None, falling to the system default chain.
        let model = agent
            .model
            .as_ref()
            .and_then(|m| resolve_model_ref(m, providers))
            .or_else(|| defaults.model.clone())
            .or_else(|| profile.model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        // 5. Resolve skills: agent.skills > defaults.skills > vec!["*"]
        let skills = agent
            .skills
            .clone()
            .or_else(|| defaults.skills.clone())
            .unwrap_or_else(|| vec!["*".to_string()]);

        // 5b. Resolve skills_blacklist: agent > defaults > vec![]
        let skills_blacklist = agent
            .skills_blacklist
            .clone()
            .or_else(|| defaults.skills_blacklist.clone())
            .unwrap_or_default();

        // 6. Load SOUL.md, AGENTS.md from agent identity directory.
        // MEMORY.md is owned by the curated memory module and read at prompt
        // build time — see `MemoryContextProvider::build_curated_message`.
        let soul_md = self.identity_loader.load(&agent_dir, "SOUL.md");
        let agents_md = self.identity_loader.load_agents_md(&agent_dir);

        // 7. Build ResolvedAgent
        let name = agent.name.clone().unwrap_or_else(|| agent.id.clone());

        let subagent_policy = agent.subagents.clone();
        let allowed_links = agent.allowed_links.clone();
        let tool_permissions = agent.tool_permissions.clone();

        ResolvedAgent {
            id: agent.id.clone(),
            name,
            is_default: agent.default,
            workspace_path,
            agent_dir,
            profile,
            soul_md,
            agents_md,
            model,
            skills,
            skills_blacklist,
            subagent_policy,
            allowed_links,
            tool_permissions,
        }
    }
}

// =============================================================================
// Public Helper Functions
// =============================================================================

/// Initialize agent state directory structure.
pub fn initialize_agent_dir(path: &Path) -> Result<(), io::Error> {
    fs::create_dir_all(path.join("sessions"))?;
    Ok(())
}

/// Standard workspace directory structure:
///
/// ```text
/// ~/.aleph/agents/{agent_id}/
/// ├── SOUL.md           # Agent soul — core persona and behavior
/// ├── AGENTS.md         # Workspace-specific instructions
/// └── MEMORY.md         # Persistent memory notes
/// ```
///
pub fn initialize_agent_identity(
    path: &Path,
    agent_name: &str,
    archetype: SoulArchetype,
) -> Result<(), io::Error> {
    // Ensure directory exists
    fs::create_dir_all(path)?;

    // Write each identity file (skip if already exists — never overwrite user content)
    write_if_missing(
        path,
        "SOUL.md",
        &templates::default_soul(agent_name, archetype),
    )?;
    write_if_missing(path, "AGENTS.md", &templates::default_agents(agent_name))?;
    write_if_missing(
        path,
        "IDENTITY.md",
        &templates::default_identity(agent_name, archetype),
    )?;
    write_if_missing(path, "MEMORY.md", templates::DEFAULT_MEMORY)?;
    write_if_missing(path, "TOOLS.md", templates::DEFAULT_TOOLS)?;
    write_if_missing(path, "HEARTBEAT.md", templates::DEFAULT_HEARTBEAT)?;

    Ok(())
}

/// Write a file only if it doesn't already exist.
fn write_if_missing(dir: &Path, filename: &str, content: &str) -> Result<(), io::Error> {
    let path = dir.join(filename);
    if !path.exists() {
        fs::write(&path, content)?;
    }
    Ok(())
}

// =============================================================================
// Private Helper Functions
// =============================================================================

/// Expand `~` prefix to the user's home directory.
///
/// Uses `dirs::home_dir()` for resolution. If `~` cannot be expanded
/// (e.g., no home directory available), the path is returned as-is.
fn resolve_user_path(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    if path_str.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            let rest = path_str
                .strip_prefix("~/")
                .or_else(|| path_str.strip_prefix('~'));
            if let Some(rest) = rest {
                return home.join(rest);
            }
        }
    }
    path.to_path_buf()
}

/// Default workspace root directory: `<config_dir>/workspaces`.
///
/// Workspaces hold runtime data (tool output, project files),
/// NOT identity files (SOUL.md, AGENTS.md, MEMORY.md) — those live under `agents/`.
pub(crate) fn default_workspace_root() -> PathBuf {
    aleph_home().join("workspaces")
}

/// Default agent state root directory: `<config_dir>/agents`.
///
/// ⚠️ Same directory `discovery::aleph_agents_dir()` resolves and that
/// `SelfConfigTool` writes into. It must therefore be the same resolution: a
/// second answer for one directory is how identity files end up written where
/// nothing reads them (2026-08-05 fixed exactly that on the tool side).
fn default_agents_root() -> PathBuf {
    aleph_home().join("agents")
}

/// `ALEPH_HOME`-aware Aleph home, with the historical `/tmp` fallback kept for
/// the (practically unreachable) no-home case so behaviour is unchanged there.
fn aleph_home() -> PathBuf {
    crate::utils::paths::get_config_dir().unwrap_or_else(|_| PathBuf::from("/tmp").join(".aleph"))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::templates::*;
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_workspace_path_explicit_ignored() {
        // Explicit workspace path is deprecated and ignored;
        // workspace dir always equals agent_id.
        let resolver = AgentDefinitionResolver::new();
        let agent = AgentDefinition {
            id: "coder".to_string(),
            workspace: Some(PathBuf::from("/custom/workspace")),
            ..Default::default()
        };
        let defaults = AgentDefaults {
            workspace_root: Some(PathBuf::from("/home/user/workspaces")),
            ..Default::default()
        };

        let result = resolver.resolve_workspace_path(&agent, &defaults);
        // Should use {root}/{agent_id}, NOT the explicit path
        assert_eq!(result, PathBuf::from("/home/user/workspaces/coder"));
    }

    #[test]
    fn test_resolve_workspace_path_auto_layout() {
        let resolver = AgentDefinitionResolver::new();
        let agent = AgentDefinition {
            id: "coder".to_string(),
            ..Default::default()
        };
        let defaults = AgentDefaults {
            workspace_root: Some(PathBuf::from("/home/user/workspaces")),
            ..Default::default()
        };

        let result = resolver.resolve_workspace_path(&agent, &defaults);
        assert_eq!(result, PathBuf::from("/home/user/workspaces/coder"));
    }

    #[test]
    fn test_resolve_all_basic() {
        let tmp = TempDir::new().unwrap();
        let workspace_root = tmp.path().to_path_buf();
        let agents_root = tmp.path().join("agents");

        let config = AgentsConfig {
            defaults: AgentDefaults {
                model: Some("claude-sonnet-4".to_string()),
                workspace_root: Some(workspace_root.clone()),
                agents_root: Some(agents_root),
                skills: Some(vec!["search".to_string(), "code_review".to_string()]),
                ..Default::default()
            },
            list: vec![
                AgentDefinition {
                    id: "main".to_string(),
                    default: true,
                    name: Some("Main Agent".to_string()),
                    model: Some(crate::config::types::agents_def::AgentModelRef::Legacy(
                        "claude-opus-4".to_string(),
                    )),
                    ..Default::default()
                },
                AgentDefinition {
                    id: "reviewer".to_string(),
                    name: Some("Code Reviewer".to_string()),
                    ..Default::default()
                },
            ],
        };

        let profiles: HashMap<String, ProfileConfig> = HashMap::new();
        let mut resolver = AgentDefinitionResolver::new();
        let resolved = resolver.resolve_all(&config, &profiles, &std::collections::HashMap::new());

        assert_eq!(resolved.len(), 2);

        // Main agent: model overridden at agent level
        let main = &resolved[0];
        assert_eq!(main.id, "main");
        assert_eq!(main.name, "Main Agent");
        assert!(main.is_default);
        assert_eq!(main.model, "claude-opus-4");
        assert_eq!(main.skills, vec!["search", "code_review"]);
        assert_eq!(main.workspace_path, workspace_root.join("main"));
        // Reviewer agent: model falls through to defaults
        let reviewer = &resolved[1];
        assert_eq!(reviewer.id, "reviewer");
        assert_eq!(reviewer.name, "Code Reviewer");
        assert!(!reviewer.is_default);
        assert_eq!(reviewer.model, "claude-sonnet-4");
        assert_eq!(reviewer.skills, vec!["search", "code_review"]);
        assert_eq!(reviewer.workspace_path, workspace_root.join("reviewer"));
    }

    #[test]
    fn test_resolve_all_empty_creates_default() {
        let tmp = TempDir::new().unwrap();
        let workspace_root = tmp.path().to_path_buf();

        let config = AgentsConfig {
            defaults: AgentDefaults {
                workspace_root: Some(workspace_root.clone()),
                ..Default::default()
            },
            list: vec![],
        };

        let profiles: HashMap<String, ProfileConfig> = HashMap::new();
        let mut resolver = AgentDefinitionResolver::new();
        let resolved = resolver.resolve_all(&config, &profiles, &std::collections::HashMap::new());

        assert_eq!(resolved.len(), 1);

        let main = &resolved[0];
        assert_eq!(main.id, "main");
        assert_eq!(main.name, "Main Agent");
        assert!(main.is_default);
        assert_eq!(main.model, DEFAULT_MODEL);
        assert_eq!(main.skills, vec!["*"]);
    }

    #[test]
    fn default_soul_uses_assistant_archetype() {
        let soul = default_soul("Nova", SoulArchetype::default());
        assert!(soul.contains("_You are Nova._"));
        assert!(soul.contains("Never fabricate facts, citations")); // base honesty floor
        assert!(soul.contains("Lead with the answer or the action.")); // assistant archetype
                                                                       // The old "thinking companion" template must be gone.
        assert!(!soul.contains("thinking companion"));
    }

    #[test]
    fn default_soul_honors_chosen_archetype() {
        let soul = default_soul("Quant", SoulArchetype::Expert);
        assert!(soul.contains("_You are Quant._"));
        assert!(soul.contains("Accuracy beats approval.")); // expert archetype marker
    }

    #[test]
    fn default_identity_seeds_role_and_vibe_from_archetype() {
        let id = default_identity("Quant", SoulArchetype::Expert);
        // Name stays machine-parseable (**Name:**), and Role/Vibe are seeded
        // from the Expert archetype rather than left as empty placeholders.
        assert!(id.contains("**Name:** Quant"));
        assert!(id.contains(SoulArchetype::Expert.role_hint()));
        assert!(id.contains(SoulArchetype::Expert.vibe_hint()));
        assert!(id.contains("edit to taste")); // still invites customization
        assert!(!id.contains("(assistant? advisor?")); // old placeholder gone

        // A different archetype seeds different hints.
        let maker = default_identity("Builder", SoulArchetype::Maker);
        assert!(maker.contains(SoulArchetype::Maker.vibe_hint()));
        assert!(!maker.contains(SoulArchetype::Expert.vibe_hint()));
    }

    #[test]
    fn test_workspace_initialization() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("test-agent");

        initialize_agent_identity(&workspace, "Test Agent", SoulArchetype::default()).unwrap();

        // AGENTS.md should exist with template content
        let agents_md = fs::read_to_string(workspace.join("AGENTS.md")).unwrap();
        assert!(agents_md.contains("Test Agent"));
        assert!(agents_md.contains("Operating Manual"));

        // New bootstrap files should also exist
        assert!(workspace.join("SOUL.md").exists());
        assert!(workspace.join("IDENTITY.md").exists());
        assert!(workspace.join("MEMORY.md").exists());
        assert!(workspace.join("HEARTBEAT.md").exists());

        // SOUL.md should contain the agent name
        let soul_md = fs::read_to_string(workspace.join("SOUL.md")).unwrap();
        assert!(soul_md.contains("Test Agent"));

        // Running again should not overwrite AGENTS.md
        fs::write(workspace.join("AGENTS.md"), "Custom content").unwrap();
        initialize_agent_identity(&workspace, "Test Agent", SoulArchetype::default()).unwrap();
        let agents_md_after = fs::read_to_string(workspace.join("AGENTS.md")).unwrap();
        assert_eq!(agents_md_after, "Custom content");
    }

    #[test]
    fn test_resolve_creates_dual_directories() {
        let tmp = TempDir::new().unwrap();
        let workspace_root = tmp.path().join("workspaces");
        let agents_root = tmp.path().join("agents");

        let config = AgentsConfig {
            defaults: AgentDefaults {
                workspace_root: Some(workspace_root.clone()),
                agents_root: Some(agents_root.clone()),
                ..Default::default()
            },
            list: vec![AgentDefinition {
                id: "coder".to_string(),
                name: Some("Coder".to_string()),
                ..Default::default()
            }],
        };

        let profiles = HashMap::new();
        let mut resolver = AgentDefinitionResolver::new();
        let resolved = resolver.resolve_all(&config, &profiles, &std::collections::HashMap::new());

        assert_eq!(resolved.len(), 1);
        let agent = &resolved[0];

        // Workspace dir (tool output only)
        assert_eq!(agent.workspace_path, workspace_root.join("coder"));

        // Agent identity dir has identity files + sessions
        assert_eq!(agent.agent_dir, agents_root.join("coder"));
        assert!(agent.agent_dir.join("SOUL.md").exists());
        assert!(agent.agent_dir.join("sessions").is_dir());

        // Identity files should NOT be in workspace
        assert!(!agent.workspace_path.join("SOUL.md").exists());
    }

    #[test]
    fn test_lazy_migration_moves_sessions() {
        let tmp = TempDir::new().unwrap();
        let workspace_root = tmp.path().join("workspaces");
        let agents_root = tmp.path().join("agents");

        // Pre-create old unified layout: sessions/ inside workspace
        let ws = workspace_root.join("migrator");
        let old_sessions = ws.join("sessions");
        fs::create_dir_all(&old_sessions).unwrap();
        fs::write(old_sessions.join("test-session.json"), "{}").unwrap();
        // Pre-create content files so initialize_agent_identity doesn't overwrite
        fs::create_dir_all(ws.join("memory")).unwrap();
        fs::write(ws.join("SOUL.md"), "# Migrator").unwrap();
        fs::write(ws.join("AGENTS.md"), "# WS").unwrap();
        fs::write(ws.join("MEMORY.md"), "# Mem").unwrap();

        let config = AgentsConfig {
            defaults: AgentDefaults {
                workspace_root: Some(workspace_root.clone()),
                agents_root: Some(agents_root.clone()),
                ..Default::default()
            },
            list: vec![AgentDefinition {
                id: "migrator".to_string(),
                ..Default::default()
            }],
        };

        let profiles = HashMap::new();
        let mut resolver = AgentDefinitionResolver::new();
        let resolved = resolver.resolve_all(&config, &profiles, &std::collections::HashMap::new());
        let agent = &resolved[0];

        // sessions/ should have been copied to agent_dir
        assert!(agent
            .agent_dir
            .join("sessions")
            .join("test-session.json")
            .exists());
        // old sessions/ should be removed
        assert!(!agent.workspace_path.join("sessions").exists());
    }

    #[test]
    fn test_no_migration_when_no_session_files() {
        let tmp = TempDir::new().unwrap();
        let workspace_root = tmp.path().join("workspaces");
        let agents_root = tmp.path().join("agents");

        // Pre-create empty sessions/ in workspace
        let ws = workspace_root.join("empty");
        let old_sessions = ws.join("sessions");
        fs::create_dir_all(&old_sessions).unwrap();
        fs::create_dir_all(ws.join("memory")).unwrap();
        fs::write(ws.join("SOUL.md"), "# Empty").unwrap();
        fs::write(ws.join("AGENTS.md"), "# WS").unwrap();
        fs::write(ws.join("MEMORY.md"), "# Mem").unwrap();

        let config = AgentsConfig {
            defaults: AgentDefaults {
                workspace_root: Some(workspace_root.clone()),
                agents_root: Some(agents_root.clone()),
                ..Default::default()
            },
            list: vec![AgentDefinition {
                id: "empty".to_string(),
                ..Default::default()
            }],
        };

        let profiles = HashMap::new();
        let mut resolver = AgentDefinitionResolver::new();
        resolver.resolve_all(&config, &profiles, &std::collections::HashMap::new());

        // No migration should happen for empty sessions dir
        // agent_dir/sessions/ should exist (from initialize_agent_dir)
        assert!(agents_root.join("empty").join("sessions").is_dir());
    }

    // -------------------------------------------------------------------------
    // resolve_model_ref tests
    // -------------------------------------------------------------------------

    fn test_provider() -> crate::config::types::provider::ProviderConfig {
        crate::config::types::provider::ProviderConfig {
            protocol: None,
            api_key: None,
            models: vec![],
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 60,
            stream_idle_timeout_secs: None,
            cache_retention: None,
            enabled: true,
            max_tokens: None,
            context_window: None,
            temperature: None,
            top_p: None,
            top_k: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
            model_behavior: None,
            verified: false,
            service_tier: None,
            response_format: None,
            parallel_tool_calls: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
            metadata_user_id: None,
            effort: None,
        }
    }

    #[test]
    fn resolve_model_ref_legacy_passes_through() {
        use crate::config::types::agents_def::AgentModelRef;
        let providers = std::collections::HashMap::new();
        let m = AgentModelRef::Legacy("anything".to_string());
        assert_eq!(
            super::resolve_model_ref(&m, &providers),
            Some("anything".to_string())
        );
    }

    #[test]
    fn resolve_model_ref_qualified_hits_when_valid() {
        use crate::config::types::agents_def::AgentModelRef;
        use crate::config::types::provider::ProviderConfig;
        let mut providers = std::collections::HashMap::new();
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig {
                enabled: true,
                models: vec!["claude-sonnet-4".to_string()],
                ..test_provider()
            },
        );
        let m = AgentModelRef::Qualified {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
        };
        assert_eq!(
            super::resolve_model_ref(&m, &providers),
            Some("claude-sonnet-4".to_string())
        );
    }

    #[test]
    fn resolve_model_ref_falls_back_when_provider_missing_disabled_or_model_dropped() {
        use crate::config::types::agents_def::AgentModelRef;
        use crate::config::types::provider::ProviderConfig;
        let mut providers = std::collections::HashMap::new();
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig {
                enabled: false,
                models: vec!["claude-sonnet-4".to_string()],
                ..test_provider()
            },
        );
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                enabled: true,
                models: vec!["gpt-5".to_string()],
                ..test_provider()
            },
        );
        let disabled = AgentModelRef::Qualified {
            provider: "anthropic".into(),
            model: "claude-sonnet-4".into(),
        };
        let missing_provider = AgentModelRef::Qualified {
            provider: "ghost".into(),
            model: "x".into(),
        };
        let model_dropped = AgentModelRef::Qualified {
            provider: "openai".into(),
            model: "gpt-4".into(),
        };
        assert_eq!(super::resolve_model_ref(&disabled, &providers), None);
        assert_eq!(
            super::resolve_model_ref(&missing_provider, &providers),
            None
        );
        assert_eq!(super::resolve_model_ref(&model_dropped, &providers), None);
    }
}
