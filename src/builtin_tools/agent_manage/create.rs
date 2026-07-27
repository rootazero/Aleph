//! `AgentCreateTool` — create a new agent with its own workspace and memory.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::agent_manager::AgentManager;
use crate::config::agent_resolver::initialize_agent_identity;
use crate::config::types::agents_def::{AgentDefinition, AgentModelRef};
use crate::error::Result;
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
use crate::sync_primitives::Arc;
use crate::thinker::soul_archetypes::{compose_soul, SoulArchetype};
use crate::tools::AlephTool;

// =============================================================================
// Validation
// =============================================================================

/// Generate a valid ASCII agent ID from a display name.
///
/// For ASCII names: slugify ("Trading Assistant" → "trading-assistant")
/// For non-ASCII names: use a deterministic hash ("交易助手" → "agent-a1b2c3d4")
#[must_use]
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
pub fn validate_agent_id(id: &str) -> std::result::Result<(), String> {
    if id.is_empty() {
        return Err("Agent ID cannot be empty".to_string());
    }
    if id.len() > 64 {
        return Err(format!("Agent ID too long ({} chars, max 64)", id.len()));
    }
    let first = match id.chars().next() {
        Some(c) => c,
        None => unreachable!("id checked non-empty above"),
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(format!(
            "Agent ID must start with a lowercase letter or digit, got '{first}'"
        ));
    }
    for ch in id.chars().skip(1) {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '_' && ch != '-' {
            return Err(format!(
                "Agent ID contains invalid character '{ch}'. Allowed: a-z, 0-9, _, -"
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
    /// Soul archetype base for this agent's persona: expert | companion | assistant | maker.
    /// Defaults to assistant when omitted. Ignored if `system_prompt` is provided.
    #[serde(default)]
    pub archetype: Option<SoulArchetype>,
    /// Personalization markdown synthesized from the creation interview
    /// (domain focus, tone tweaks, hard boundaries, signature behaviors).
    /// Appended under "## This Agent". Ignored if `system_prompt` is provided.
    #[serde(default)]
    pub personalization: Option<String>,
    /// Raw input from slash command fast path (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub input: Option<String>,
}

/// Output from agent creation.
#[derive(Debug, Clone, Serialize)]
pub struct AgentCreateOutput {
    /// The agent ID that was created
    pub agent_id: String,
    /// Path to the agent's workspace directory
    pub workspace_path: String,
    /// Human-readable status message
    pub message: String,
}

/// Decide the SOUL.md content for a new agent.
///
/// `system_prompt` (when non-blank) is a verbatim full override. Otherwise the
/// soul is composed from the chosen archetype (default Assistant) + Base +
/// optional personalization.
fn resolve_soul_content(args: &AgentCreateArgs, display_name: &str) -> String {
    if let Some(prompt) = args.system_prompt.as_deref() {
        if !prompt.trim().is_empty() {
            return prompt.to_string();
        }
    }
    compose_soul(
        args.archetype.unwrap_or_default(),
        display_name,
        args.personalization.as_deref(),
    )
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that creates a new agent with its own workspace and memory.
#[derive(Clone)]
pub struct AgentCreateTool {
    registry: Arc<AgentRegistry>,
    #[allow(dead_code)]
    workspace_mgr: Arc<AgentEnvStore>,
    agent_manager: Option<Arc<AgentManager>>,
    session_store: Arc<dyn crate::gateway::session_store::SessionStore>,
    raw_memory_writer: Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
    /// Bus for the `Registered` lifecycle event (parity with boot-time
    /// registration and the `agents.create` RPC).
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
}

impl AgentCreateTool {
    pub fn new(
        registry: Arc<AgentRegistry>,
        workspace_mgr: Arc<AgentEnvStore>,
        session_store: Arc<dyn crate::gateway::session_store::SessionStore>,
    ) -> Self {
        Self {
            registry,
            workspace_mgr,
            agent_manager: None,
            session_store,
            raw_memory_writer: None,
            event_bus: None,
        }
    }

    #[must_use]
    pub fn with_agent_manager(mut self, manager: Arc<AgentManager>) -> Self {
        self.agent_manager = Some(manager);
        self
    }

    pub fn with_raw_memory_writer(
        mut self,
        writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    ) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }

    #[must_use]
    pub fn with_event_bus(
        mut self,
        bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    ) -> Self {
        self.event_bus = bus;
        self
    }
}

#[async_trait]
impl AlephTool for AgentCreateTool {
    const NAME: &'static str = "agent_create";
    const DESCRIPTION: &'static str =
        "Create a new agent with its own workspace, memory, and soul. Use when the user \
         wants a specialized agent (trading, coding, health, a companion, etc.).\n\n\
         Before creating, if the request is under-specified, run a short creation interview:\n\
         1) Recommend ONE soul archetype from the user's purpose and confirm it — pick from \
         the Soul Archetypes catalog in this tool's usage notes below.\n\
         2) Ask up to 2-5 short questions to gather: domain/focus, name, tone tweaks, hard \
         boundaries, signature behaviors.\n\
         3) Call agent_create with the chosen `archetype` and a `personalization` markdown \
         block synthesizing the answers.\n\
         If the user already gave enough detail or asks you to just create it, skip the \
         questions. After creation, make it active with agent_switch.";

    type Args = AgentCreateArgs;
    type Output = AgentCreateOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "agent_create(id='quant', name='Quant', archetype='expert', personalization='Focus: equities and macro. Always show confidence and sourcing. Hard boundary: no trade execution.')".to_string(),
            "agent_create(id='coder', name='Coder', archetype='maker', personalization='Stack: Rust + tokio. Always run cargo check before claiming done.')".to_string(),
            "agent_create(id='iris', name='Iris', archetype='companion', personalization='Evening check-ins. Reflect first; never push advice unasked.')".to_string(),
        ])
    }

    /// Build the definition, then append the Soul Archetypes catalog to
    /// `llm_context` from the single source ([`soul_archetypes::creation_catalog`]).
    ///
    /// The trait default only injects `examples()`; we extend it so the
    /// interview list the model reads is generated from [`SoulArchetype::summary`]
    /// rather than a hand-copied literal that drifts from the templates.
    fn definition(&self) -> crate::tool_metadata::ToolDefinition {
        use crate::thinker::soul_archetypes::creation_catalog;

        let mut context = format!("## Soul Archetypes (choose one)\n\n{}", creation_catalog());
        if let Some(examples) = self.examples() {
            let examples_text = examples
                .iter()
                .enumerate()
                .map(|(i, ex)| format!("{}. {}", i + 1, ex))
                .collect::<Vec<_>>()
                .join("\n");
            context.push_str(&format!("\n\n## Usage Examples\n\n{examples_text}"));
        }

        let schema = schemars::schema_for!(AgentCreateArgs);
        let parameters = serde_json::to_value(&schema).unwrap_or_default();
        crate::tool_metadata::ToolDefinition::new(
            Self::NAME,
            Self::DESCRIPTION,
            parameters,
            self.category(),
        )
        .with_confirmation(self.requires_confirmation())
        .with_strict(self.strict_schema())
        .with_llm_context(context)
    }

    async fn call(&self, mut args: Self::Args) -> Result<Self::Output> {
        // Auto-resolve name and id from raw slash command input
        // e.g., /agent_create 交易助手 → name="交易助手", id="agent-{hash}"
        if args.id.is_empty() {
            let raw_name = args
                .name
                .clone()
                .or_else(|| args.input.as_ref().map(|s| s.trim().to_string()))
                .unwrap_or_default();

            if raw_name.is_empty() {
                return Err(crate::error::AlephError::other(
                    "Agent name or id is required. Usage: /agent_create <name>",
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

        // 3. Determine paths
        let home = dirs::home_dir()
            .ok_or_else(|| crate::error::AlephError::other("Cannot determine home directory"))?;
        let agents_state_root = home.join(".aleph/agents");
        let agent_state_dir = agents_state_root.join(&args.id);

        let workspaces_dir = home.join(".aleph/workspaces");
        let workspace_path = workspaces_dir.join(&args.id);

        // 4. Compose this agent's soul (archetype + base + personalization, or a
        // verbatim system_prompt override) and write it BEFORE identity-init so
        // initialize_agent_identity's write_if_missing keeps it.
        let display_name = args.name.as_deref().unwrap_or(&args.id);
        let soul_content = resolve_soul_content(&args, display_name);
        tokio::fs::create_dir_all(&agent_state_dir)
            .await
            .map_err(|e| {
                crate::error::AlephError::other(format!(
                    "Failed to create agent state dir for '{}': {}",
                    args.id, e
                ))
            })?;
        tokio::fs::write(agent_state_dir.join("SOUL.md"), &soul_content)
            .await
            .map_err(|e| {
                crate::error::AlephError::other(format!(
                    "Failed to write SOUL.md for '{}': {}",
                    args.id, e
                ))
            })?;

        // Initialize the rest of the identity directory (AGENTS.md, MEMORY.md, …).
        // SOUL.md was already written above, so the archetype here only matters
        // for the unreachable case where that write was skipped.
        initialize_agent_identity(
            &agent_state_dir,
            display_name,
            args.archetype.unwrap_or_default(),
        )
        .map_err(|e| {
            crate::error::AlephError::other(format!(
                "Failed to initialize identity files for '{}': {}",
                args.id, e
            ))
        })?;

        // Initialize agent state directory (sessions/)
        crate::config::agent_resolver::initialize_agent_dir(&agent_state_dir).map_err(|e| {
            crate::error::AlephError::other(format!(
                "Failed to initialize agent state dir for '{}': {}",
                args.id, e
            ))
        })?;

        // Create workspace directory for tool output
        tokio::fs::create_dir_all(&workspace_path)
            .await
            .map_err(|e| {
                crate::error::AlephError::other(format!(
                    "Failed to create workspace for '{}': {}",
                    args.id, e
                ))
            })?;

        // 5. Write custom system_prompt to AGENTS.md if provided
        if let Some(ref prompt) = args.system_prompt {
            let agents_md = agent_state_dir.join("AGENTS.md");
            let content = format!(
                "# {display_name} Workspace\n\n\
                 ## System Prompt\n\n\
                 {prompt}\n\n\
                 ## Instructions\n\n\
                 Add workspace-specific instructions here.\n"
            );
            tokio::fs::write(&agents_md, content).await.map_err(|e| {
                crate::error::AlephError::other(format!("Failed to write AGENTS.md: {e}"))
            })?;
        }

        // IDENTITY.md / TOOLS.md are owned by `initialize_agent_identity` above
        // (via `write_if_missing`, using the rich archetype-seeded templates).
        // They always exist by this point, so the old `if !exists` fallbacks
        // here were dead code that could only ever write a thinner, worse copy —
        // removed. Keep new identity-file templates in `agent_resolver`.

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

        let instance = {
            let mut inst =
                AgentInstance::new(config, Arc::clone(&self.session_store)).map_err(|e| {
                    crate::error::AlephError::other(format!(
                        "Failed to create agent instance '{}': {}",
                        args.id, e
                    ))
                })?;
            if let Some(ref writer) = self.raw_memory_writer {
                inst = inst.with_raw_memory_writer(Arc::clone(writer));
            }
            inst
        };

        // 8. Register in AgentRegistry (runtime) + lifecycle event so the
        //    Panel and other consumers see the new agent immediately (same
        //    contract as boot-time registration and the agents.create RPC).
        self.registry.register(instance).await;
        if let Some(ref bus) = self.event_bus {
            crate::gateway::agent_lifecycle::AgentLifecycleEvent::Registered {
                agent_id: args.id.clone(),
                workspace: workspace_path.clone(),
                model: model.to_string(),
            }
            .publish(bus);
        }

        // 8b. Persist to AgentManager (TOML config) so agents.list RPC returns it
        if let Some(ref manager) = self.agent_manager {
            let def = AgentDefinition {
                id: args.id.clone(),
                name: args.name.clone(),
                model: Some(AgentModelRef::Legacy(model.to_string())),
                archetype: args.archetype,
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

        let workspace_str = workspace_path.to_string_lossy().to_string();
        let msg = format!("Agent '{}' created. Workspace: {}", args.id, workspace_str);

        info!(agent_id = %args.id, "Agent created successfully");

        Ok(AgentCreateOutput {
            agent_id: args.id,
            workspace_path: workspace_str,
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
    use crate::gateway::agent_env::AgentEnvStoreConfig;
    use tempfile::tempdir;

    fn test_workspace_mgr() -> Arc<AgentEnvStore> {
        let temp = tempdir().unwrap();
        let config = AgentEnvStoreConfig {
            db_path: temp.keep().join("test.db"),
            default_profile: "default".to_string(),
            archive_after_days: 0,
        };
        Arc::new(AgentEnvStore::new(config).unwrap())
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
        assert_eq!(
            generate_agent_id_from_name("Trading Assistant"),
            "trading-assistant"
        );
        assert_eq!(
            generate_agent_id_from_name("code-reviewer"),
            "code-reviewer"
        );
        assert_eq!(generate_agent_id_from_name("my_agent"), "my-agent");
    }

    #[test]
    fn test_generate_id_non_ascii_name() {
        // Chinese names should produce a deterministic hash-based id
        let id = generate_agent_id_from_name("交易助手");
        assert!(id.starts_with("agent-"), "Got: {}", id);
        assert!(
            validate_agent_id(&id).is_ok(),
            "Generated id should be valid: {}",
            id
        );

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
        assert!(
            id.starts_with("agent-"),
            "Single char should fallback: {}",
            id
        );
    }

    #[test]
    fn resolve_soul_expert_with_personalization() {
        let args: AgentCreateArgs = serde_json::from_str(
            r#"{"id":"quant","archetype":"expert","personalization":"Focus: equities and macro."}"#,
        )
        .unwrap();
        let soul = resolve_soul_content(&args, "Quant");
        assert!(soul.contains("Accuracy beats approval.")); // expert
        assert!(soul.contains("Never fabricate facts, citations")); // base
        assert!(soul.contains("## This Agent"));
        assert!(soul.contains("Focus: equities and macro."));
    }

    #[test]
    fn resolve_soul_defaults_to_assistant() {
        let args: AgentCreateArgs = serde_json::from_str(r#"{"id":"helper"}"#).unwrap();
        let soul = resolve_soul_content(&args, "Helper");
        assert!(soul.contains("Lead with the answer or the action.")); // assistant
        assert!(!soul.contains("## This Agent"));
    }

    #[test]
    fn resolve_soul_system_prompt_overrides_verbatim() {
        let args: AgentCreateArgs =
            serde_json::from_str(r#"{"id":"x","system_prompt":"RAW SOUL TEXT"}"#).unwrap();
        assert_eq!(resolve_soul_content(&args, "X"), "RAW SOUL TEXT");
    }

    #[test]
    fn test_create_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let workspace_mgr = test_workspace_mgr();
        let temp = tempfile::tempdir().unwrap();
        let sm_config = crate::gateway::session_manager::SessionManagerConfig {
            db_path: temp.path().join("test_sessions.db"),
            ..Default::default()
        };
        let sm = Arc::new(
            crate::gateway::session_manager::SessionManager::new(sm_config)
                .expect("test session manager"),
        );
        let tool = AgentCreateTool::new(registry, workspace_mgr, sm);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "agent_create");
        assert!(!def.requires_confirmation);

        // llm_context carries the SSOT archetype catalog (from summary()) AND
        // the usage examples — both must be wired in.
        let context = def
            .llm_context
            .expect("agent_create must inject llm_context");
        assert!(context.contains("## Soul Archetypes"));
        assert!(context.contains("expert:"));
        assert!(context.contains("companion:"));
        assert!(context.contains("(default when unclear)"));
        assert!(context.contains("## Usage Examples"));
    }
}
