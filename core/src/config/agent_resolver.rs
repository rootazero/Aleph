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

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::types::agents_def::{AgentDefinition, AgentDefaults, AgentsConfig, SubagentPolicy};
use crate::config::types::profile::ProfileConfig;
use crate::gateway::workspace_loader::WorkspaceFileLoader;
use crate::thinker::soul::SoulManifest;

// =============================================================================
// Constants
// =============================================================================

/// Maximum characters for bootstrap file loading (MEMORY.md truncation).
const DEFAULT_BOOTSTRAP_MAX_CHARS: usize = 20_000;

/// Fallback model when no model is specified at any level.
const DEFAULT_MODEL: &str = "claude-opus-4-6";

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

    /// Resolved profile configuration
    pub profile: ProfileConfig,

    /// Parsed SOUL.md manifest (if present in workspace)
    pub soul: Option<SoulManifest>,

    /// Raw AGENTS.md content (if present in workspace)
    pub agents_md: Option<String>,

    /// Raw MEMORY.md content, truncated to max chars (if present in workspace)
    pub memory_md: Option<String>,

    /// Resolved AI model identifier
    pub model: String,

    /// Resolved list of allowed skills
    pub skills: Vec<String>,

    /// Sub-agent spawning policy
    pub subagent_policy: SubagentPolicy,
}

// =============================================================================
// AgentDefinitionResolver
// =============================================================================

/// Resolves agent definitions from configuration into runtime-ready structs.
///
/// Merges `AgentDefinition` entries with `AgentDefaults`, `ProfileConfig`,
/// and workspace files (SOUL.md, AGENTS.md, MEMORY.md) to produce fully
/// resolved `ResolvedAgent` instances.
pub struct AgentDefinitionResolver {
    workspace_loader: WorkspaceFileLoader,
}

impl AgentDefinitionResolver {
    /// Create a new resolver with a fresh workspace file loader.
    pub fn new() -> Self {
        Self {
            workspace_loader: WorkspaceFileLoader::new(),
        }
    }

    /// Resolve all agent definitions from configuration.
    ///
    /// If `config.list` is empty, a default "main" agent is created via
    /// `AgentsConfig::ensure_default()` on a cloned config.
    pub fn resolve_all(
        &mut self,
        config: &AgentsConfig,
        profiles: &HashMap<String, ProfileConfig>,
    ) -> Vec<ResolvedAgent> {
        let config = if config.list.is_empty() {
            let mut cloned = config.clone();
            cloned.ensure_default();
            cloned
        } else {
            config.clone()
        };

        config
            .list
            .iter()
            .map(|agent_def| self.resolve_one(agent_def, &config.defaults, profiles))
            .collect()
    }

    /// Resolve workspace path for an agent.
    ///
    /// Priority: explicit agent workspace > auto-layout (`{workspace_root}/{agent_id}`).
    pub fn resolve_workspace_path(
        &self,
        agent: &AgentDefinition,
        defaults: &AgentDefaults,
    ) -> PathBuf {
        // Explicit workspace path on the agent takes priority
        if let Some(ref workspace) = agent.workspace {
            return resolve_user_path(workspace);
        }

        // Auto-layout: {workspace_root}/{agent_id}
        let root = defaults
            .workspace_root
            .as_ref()
            .map(|p| resolve_user_path(p))
            .unwrap_or_else(default_workspace_root);

        root.join(&agent.id)
    }

    /// Resolve a single agent definition into a `ResolvedAgent`.
    fn resolve_one(
        &mut self,
        agent: &AgentDefinition,
        defaults: &AgentDefaults,
        profiles: &HashMap<String, ProfileConfig>,
    ) -> ResolvedAgent {
        // 1. Resolve workspace path
        let workspace_path = self.resolve_workspace_path(agent, defaults);

        // 2. Initialize workspace directory (create dirs + default files)
        let agent_name = agent
            .name
            .as_deref()
            .unwrap_or(&agent.id);
        if let Err(e) = initialize_workspace(&workspace_path, agent_name) {
            tracing::warn!(
                agent_id = %agent.id,
                path = %workspace_path.display(),
                error = %e,
                "Failed to initialize workspace directory"
            );
        }

        // 3. Load ProfileConfig from profiles HashMap
        let profile = agent
            .profile
            .as_ref()
            .and_then(|name| profiles.get(name))
            .cloned()
            .unwrap_or_default();

        // 4. Resolve model: agent.model > defaults.model > profile.model > DEFAULT_MODEL
        let model = agent
            .model
            .clone()
            .or_else(|| defaults.model.clone())
            .or_else(|| profile.model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        // 5. Resolve skills: agent.skills > defaults.skills > vec!["*"]
        let skills = agent
            .skills
            .clone()
            .or_else(|| defaults.skills.clone())
            .unwrap_or_else(|| vec!["*".to_string()]);

        // 6. Load SOUL.md, AGENTS.md, MEMORY.md from workspace
        let soul = self.workspace_loader.load_soul(&workspace_path);
        let agents_md = self.workspace_loader.load_agents_md(&workspace_path);
        let memory_md = self
            .workspace_loader
            .load_memory_md(&workspace_path, DEFAULT_BOOTSTRAP_MAX_CHARS);

        // 7. Build ResolvedAgent
        let name = agent
            .name
            .clone()
            .unwrap_or_else(|| agent.id.clone());

        let subagent_policy = agent
            .subagents
            .clone()
            .unwrap_or_default();

        ResolvedAgent {
            id: agent.id.clone(),
            name,
            is_default: agent.default,
            workspace_path,
            profile,
            soul,
            agents_md,
            memory_md,
            model,
            skills,
            subagent_policy,
        }
    }
}

// =============================================================================
// Public Helper Functions
// =============================================================================

/// Initialize an agent workspace directory.
///
/// Creates the workspace directory structure and default files:
/// - `{path}/memory/` directory
/// - `{path}/AGENTS.md` with a template (only if it doesn't already exist)
pub fn initialize_workspace(path: &Path, agent_name: &str) -> Result<(), io::Error> {
    // Create memory directory
    fs::create_dir_all(path.join("memory"))?;

    // Create AGENTS.md with template if it doesn't exist
    let agents_md_path = path.join("AGENTS.md");
    if !agents_md_path.exists() {
        let template = format!(
            "# {} Workspace\n\n\
             ## Instructions\n\n\
             Add workspace-specific instructions here.\n",
            agent_name
        );
        fs::write(&agents_md_path, template)?;
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
            let rest = path_str.strip_prefix("~/").or_else(|| path_str.strip_prefix('~'));
            if let Some(rest) = rest {
                return home.join(rest);
            }
        }
    }
    path.to_path_buf()
}

/// Default workspace root directory: `~/.aleph/workspaces`.
fn default_workspace_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".aleph")
        .join("workspaces")
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_workspace_path_explicit() {
        let resolver = AgentDefinitionResolver::new();
        let agent = AgentDefinition {
            id: "coder".to_string(),
            workspace: Some(PathBuf::from("/custom/workspace")),
            ..Default::default()
        };
        let defaults = AgentDefaults::default();

        let result = resolver.resolve_workspace_path(&agent, &defaults);
        assert_eq!(result, PathBuf::from("/custom/workspace"));
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

        let config = AgentsConfig {
            defaults: AgentDefaults {
                model: Some("claude-sonnet-4".to_string()),
                workspace_root: Some(workspace_root.clone()),
                skills: Some(vec!["search".to_string(), "code_review".to_string()]),
                ..Default::default()
            },
            list: vec![
                AgentDefinition {
                    id: "main".to_string(),
                    default: true,
                    name: Some("Main Agent".to_string()),
                    model: Some("claude-opus-4".to_string()),
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
        let resolved = resolver.resolve_all(&config, &profiles);

        assert_eq!(resolved.len(), 2);

        // Main agent: model overridden at agent level
        let main = &resolved[0];
        assert_eq!(main.id, "main");
        assert_eq!(main.name, "Main Agent");
        assert!(main.is_default);
        assert_eq!(main.model, "claude-opus-4");
        assert_eq!(main.skills, vec!["search", "code_review"]);
        assert_eq!(main.workspace_path, workspace_root.join("main"));
        assert!(main.workspace_path.join("memory").exists());

        // Reviewer agent: model falls through to defaults
        let reviewer = &resolved[1];
        assert_eq!(reviewer.id, "reviewer");
        assert_eq!(reviewer.name, "Code Reviewer");
        assert!(!reviewer.is_default);
        assert_eq!(reviewer.model, "claude-sonnet-4");
        assert_eq!(reviewer.skills, vec!["search", "code_review"]);
        assert_eq!(reviewer.workspace_path, workspace_root.join("reviewer"));
        assert!(reviewer.workspace_path.join("memory").exists());
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
        let resolved = resolver.resolve_all(&config, &profiles);

        assert_eq!(resolved.len(), 1);

        let main = &resolved[0];
        assert_eq!(main.id, "main");
        assert_eq!(main.name, "Main Agent");
        assert!(main.is_default);
        assert_eq!(main.model, DEFAULT_MODEL);
        assert_eq!(main.skills, vec!["*"]);
    }

    #[test]
    fn test_workspace_initialization() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("test-agent");

        initialize_workspace(&workspace, "Test Agent").unwrap();

        // memory/ directory should exist
        assert!(workspace.join("memory").is_dir());

        // AGENTS.md should exist with template content
        let agents_md = fs::read_to_string(workspace.join("AGENTS.md")).unwrap();
        assert!(agents_md.contains("Test Agent"));
        assert!(agents_md.contains("Instructions"));

        // Running again should not overwrite AGENTS.md
        fs::write(workspace.join("AGENTS.md"), "Custom content").unwrap();
        initialize_workspace(&workspace, "Test Agent").unwrap();
        let agents_md_after = fs::read_to_string(workspace.join("AGENTS.md")).unwrap();
        assert_eq!(agents_md_after, "Custom content");
    }
}
