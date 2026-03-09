//! AgentCreateTool — create a new agent with its own workspace and memory.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

use crate::config::agent_manager::AgentManager;
use crate::config::agent_resolver::initialize_workspace;
use crate::config::types::agents_def::AgentDefinition;
use crate::error::Result;
use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
use crate::gateway::workspace::WorkspaceManager;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Validation
// =============================================================================

/// Generate a valid ASCII agent ID from a display name.
///
/// For ASCII names: slugify ("Trading Assistant" → "trading-assistant")
/// For non-ASCII names: use a deterministic hash ("交易助手" → "agent-a1b2c3d4")
pub fn generate_agent_id_from_name(name: &str) -> String {
    // Try to build an ASCII slug from the name
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c == ' ' || c == '-' || c == '_' {
                '-'
            } else {
                '\0' // skip non-ASCII
            }
        })
        .filter(|&c| c != '\0')
        .collect();

    // Clean up consecutive hyphens
    let slug: String = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    // Use slug if it's a valid id
    if slug.len() >= 2
        && slug.len() <= 64
        && slug
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return slug;
    }

    // Fallback: deterministic hash-based id
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    format!("agent-{:08x}", hasher.finish() as u32)
}

/// Validate an agent ID: `[a-z0-9][a-z0-9_-]*`, 1-64 characters.
fn validate_agent_id(id: &str) -> std::result::Result<(), String> {
    if id.is_empty() {
        return Err("Agent ID cannot be empty".to_string());
    }
    if id.len() > 64 {
        return Err(format!(
            "Agent ID too long ({} chars, max 64)",
            id.len()
        ));
    }
    let mut chars = id.chars();
    let first = chars.next().unwrap(); // safe: checked non-empty above
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(format!(
            "Agent ID must start with a lowercase letter or digit, got '{}'",
            first
        ));
    }
    for ch in chars {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '_' && ch != '-' {
            return Err(format!(
                "Agent ID contains invalid character '{}'. Allowed: a-z, 0-9, _, -",
                ch
            ));
        }
    }
    Ok(())
}

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for creating a new agent.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentCreateArgs {
    /// Unique agent identifier (a-z, 0-9, _, -, max 64 chars).
    /// If empty or missing, auto-generated from the name.
    #[serde(default)]
    pub id: String,
    /// Human-readable name (defaults to id)
    #[serde(default)]
    pub name: Option<String>,
    /// Description of what this agent specializes in
    #[serde(default)]
    pub description: Option<String>,
    /// LLM model to use (default: claude-sonnet-4-5)
    #[serde(default)]
    pub model: Option<String>,
    /// Custom system prompt for this agent
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Raw input from slash command fast path (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub input: Option<String>,
    /// Injected by registry — session channel (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __channel: String,
    /// Injected by registry — session peer_id (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __peer_id: String,
}

/// Output from agent creation.
#[derive(Debug, Clone, Serialize)]
pub struct AgentCreateOutput {
    /// The agent ID that was created
    pub agent_id: String,
    /// Path to the agent's workspace directory
    pub workspace_path: String,
    /// Whether the agent was auto-switched to
    pub switched: bool,
    /// Human-readable status message
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that creates a new agent with its own workspace and memory.
#[derive(Clone)]
pub struct AgentCreateTool {
    registry: Arc<AgentRegistry>,
    workspace_mgr: Arc<WorkspaceManager>,
    agent_manager: Option<Arc<AgentManager>>,
}

impl AgentCreateTool {
    pub fn new(
        registry: Arc<AgentRegistry>,
        workspace_mgr: Arc<WorkspaceManager>,
    ) -> Self {
        Self {
            registry,
            workspace_mgr,
            agent_manager: None,
        }
    }

    pub fn with_agent_manager(mut self, manager: Arc<AgentManager>) -> Self {
        self.agent_manager = Some(manager);
        self
    }
}

#[async_trait]
impl AlephTool for AgentCreateTool {
    const NAME: &'static str = "agent_create";
    const DESCRIPTION: &'static str =
        "Create a new agent with its own workspace and memory. The new agent is \
         automatically activated for the current conversation. Use this when the \
         user wants a specialized assistant (e.g., trading, coding, health).";

    type Args = AgentCreateArgs;
    type Output = AgentCreateOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "agent_create(id='trader', name='Trading Assistant', description='Specialized in stock analysis', model='claude-sonnet-4-5')".to_string(),
            "agent_create(id='coder', system_prompt='You are an expert Rust developer.')".to_string(),
        ])
    }

    async fn call(&self, mut args: Self::Args) -> Result<Self::Output> {
        // Auto-resolve name and id from raw slash command input
        // e.g., /agent_create 交易助手 → name="交易助手", id="agent-{hash}"
        if args.id.is_empty() {
            let raw_name = args.name.clone()
                .or_else(|| args.input.as_ref().map(|s| s.trim().to_string()))
                .unwrap_or_default();

            if raw_name.is_empty() {
                return Err(crate::error::AlephError::other(
                    "Agent name or id is required. Usage: /agent_create <name>"
                ));
            }

            // Set display name
            if args.name.is_none() {
                args.name = Some(raw_name.clone());
            }

            // Generate valid ASCII id from name
            args.id = generate_agent_id_from_name(&raw_name);
        }

        info!(agent_id = %args.id, "Agent creation requested");

        // 1. Validate ID
        validate_agent_id(&args.id).map_err(crate::error::AlephError::other)?;

        // 2. Check for duplicates
        if self.registry.get(&args.id).await.is_some() {
            return Err(crate::error::AlephError::other(format!(
                "Agent '{}' already exists",
                args.id
            )));
        }

        // 3. Determine workspace path
        let workspaces_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".aleph/workspaces");
        let workspace_path = workspaces_dir.join(&args.id);

        // 4. Initialize workspace directory
        let display_name = args.name.as_deref().unwrap_or(&args.id);
        initialize_workspace(&workspace_path, display_name)
            .map_err(|e| crate::error::AlephError::other(format!(
                "Failed to initialize workspace for '{}': {}",
                args.id, e
            )))?;

        // Initialize agent state directory
        let agents_state_root = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".aleph/agents");
        let agent_state_dir = agents_state_root.join(&args.id);
        crate::config::agent_resolver::initialize_agent_dir(&agent_state_dir)
            .map_err(|e| crate::error::AlephError::other(format!(
                "Failed to initialize agent state dir for '{}': {}",
                args.id, e
            )))?;

        // 5. Write custom system_prompt to AGENTS.md if provided
        if let Some(ref prompt) = args.system_prompt {
            let agents_md = workspace_path.join("AGENTS.md");
            let content = format!(
                "# {} Workspace\n\n\
                 ## System Prompt\n\n\
                 {}\n\n\
                 ## Instructions\n\n\
                 Add workspace-specific instructions here.\n",
                display_name, prompt
            );
            std::fs::write(&agents_md, content).map_err(|e| {
                crate::error::AlephError::other(format!(
                    "Failed to write AGENTS.md: {}", e
                ))
            })?;
        }

        // 6. Generate template files (non-fatal if write fails)
        let soul_path = workspace_path.join("SOUL.md");
        if !soul_path.exists() {
            let soul_content = if let Some(ref prompt) = args.system_prompt {
                prompt.clone()
            } else {
                let soul_name = args.name.as_deref().unwrap_or(&args.id);
                let specialized = match args.description.as_deref() {
                    Some(desc) => format!(" specialized in {}", desc),
                    None => String::new(),
                };
                format!(
                    "You are {}{}.\n\n\
                     ## Tone\n\
                     - Professional, friendly, concise\n\n\
                     ## Boundaries\n\
                     - Focus on your area of expertise\n\
                     - Suggest switching to another agent for out-of-scope requests\n",
                    soul_name, specialized
                )
            };
            let _ = std::fs::write(&soul_path, soul_content);
        }

        let identity_path = workspace_path.join("IDENTITY.md");
        if !identity_path.exists() {
            let identity_name = args.name.as_deref().unwrap_or(&args.id);
            let identity_content = format!(
                "- Name: {}\n- Emoji: 🤖\n- Theme: professional\n",
                identity_name
            );
            let _ = std::fs::write(&identity_path, identity_content);
        }

        let tools_path = workspace_path.join("TOOLS.md");
        if !tools_path.exists() {
            let tools_content = "# Tool Notes\n\nRecord your tool usage preferences and notes here.\n";
            let _ = std::fs::write(&tools_path, tools_content);
        }

        // 7. Create AgentInstance
        let model = args.model.as_deref().unwrap_or("claude-sonnet-4-5");
        let config = AgentInstanceConfig {
            agent_id: args.id.clone(),
            workspace: workspace_path.clone(),
            model: model.to_string(),
            system_prompt: args.system_prompt.clone(),
            agent_dir: agents_state_root.join(&args.id),
            ..Default::default()
        };

        let instance = AgentInstance::new(config)
            .map_err(|e| crate::error::AlephError::other(format!(
                "Failed to create agent instance '{}': {}",
                args.id, e
            )))?;

        // 8. Register in AgentRegistry (runtime)
        self.registry.register(instance).await;

        // 8b. Persist to AgentManager (TOML config) so agents.list RPC returns it
        if let Some(ref manager) = self.agent_manager {
            let def = AgentDefinition {
                id: args.id.clone(),
                name: args.name.clone(),
                model: Some(model.to_string()),
                ..Default::default()
            };
            if let Err(e) = manager.create(def) {
                // Non-fatal: agent works in runtime, just won't appear in Panel agents list
                warn!(
                    agent_id = %args.id,
                    error = %e,
                    "Failed to persist agent to TOML config (runtime registration succeeded)"
                );
            }
        }

        // 9. Auto-switch via WorkspaceManager (channel/peer_id injected by registry snapshot)
        let channel = args.__channel.clone();
        let peer_id = args.__peer_id.clone();
        let switched = if !channel.is_empty() && !peer_id.is_empty() {
            self.workspace_mgr
                .set_active_agent(&channel, &peer_id, &args.id)
                .map(|_| true)
                .unwrap_or(false)
        } else {
            false
        };

        let workspace_str = workspace_path.to_string_lossy().to_string();
        let msg = if switched {
            format!(
                "Agent '{}' created and activated. Workspace: {}",
                args.id, workspace_str
            )
        } else {
            format!(
                "Agent '{}' created. Workspace: {}",
                args.id, workspace_str
            )
        };

        info!(agent_id = %args.id, switched, "Agent created successfully");

        Ok(AgentCreateOutput {
            agent_id: args.id,
            workspace_path: workspace_str,
            switched,
            message: msg,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::workspace::WorkspaceManagerConfig;
    use tempfile::tempdir;

    fn test_workspace_mgr() -> Arc<WorkspaceManager> {
        let temp = tempdir().unwrap();
        let config = WorkspaceManagerConfig {
            db_path: temp.into_path().join("test.db"),
            default_profile: "default".to_string(),
            archive_after_days: 0,
        };
        Arc::new(WorkspaceManager::new(config).unwrap())
    }

    #[test]
    fn test_validate_agent_id_valid() {
        assert!(validate_agent_id("main").is_ok());
        assert!(validate_agent_id("trader").is_ok());
        assert!(validate_agent_id("my-agent").is_ok());
        assert!(validate_agent_id("agent_01").is_ok());
        assert!(validate_agent_id("0agent").is_ok());
        assert!(validate_agent_id("a").is_ok());
    }

    #[test]
    fn test_validate_agent_id_invalid() {
        assert!(validate_agent_id("").is_err());
        assert!(validate_agent_id("Agent").is_err()); // uppercase
        assert!(validate_agent_id("-start").is_err()); // starts with dash
        assert!(validate_agent_id("_start").is_err()); // starts with underscore
        assert!(validate_agent_id("has space").is_err());
        assert!(validate_agent_id("has.dot").is_err());
        let long = "a".repeat(65);
        assert!(validate_agent_id(&long).is_err()); // too long
    }

    #[test]
    fn test_validate_agent_id_max_length() {
        let exact = "a".repeat(64);
        assert!(validate_agent_id(&exact).is_ok());
    }

    #[test]
    fn test_generate_id_ascii_name() {
        assert_eq!(generate_agent_id_from_name("Trading Assistant"), "trading-assistant");
        assert_eq!(generate_agent_id_from_name("code-reviewer"), "code-reviewer");
        assert_eq!(generate_agent_id_from_name("my_agent"), "my-agent");
    }

    #[test]
    fn test_generate_id_non_ascii_name() {
        // Chinese names should produce a deterministic hash-based id
        let id = generate_agent_id_from_name("交易助手");
        assert!(id.starts_with("agent-"), "Got: {}", id);
        assert!(validate_agent_id(&id).is_ok(), "Generated id should be valid: {}", id);

        // Same name should produce same id (deterministic)
        assert_eq!(id, generate_agent_id_from_name("交易助手"));
    }

    #[test]
    fn test_generate_id_mixed_name() {
        // Mixed ASCII + non-ASCII
        let id = generate_agent_id_from_name("AI助手");
        // "AI" → "ai", Chinese chars filtered → slug is "ai" (len 2, valid)
        assert_eq!(id, "ai");
    }

    #[test]
    fn test_generate_id_single_char() {
        // Too short slug → hash fallback
        let id = generate_agent_id_from_name("A");
        assert!(id.starts_with("agent-"), "Single char should fallback: {}", id);
    }

    #[test]
    fn test_create_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let workspace_mgr = test_workspace_mgr();
        let tool = AgentCreateTool::new(registry, workspace_mgr);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "agent_create");
        assert!(!def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }
}
