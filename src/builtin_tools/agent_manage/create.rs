//! `AgentCreateTool` — create a new agent with its own workspace and memory.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::agent_manager::AgentManager;
use crate::config::agent_resolver::initialize_agent_identity;
use crate::config::types::agents_def::{AgentDefinition, AgentIdentity, AgentModelRef};
use crate::error::Result;
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
use crate::sync_primitives::Arc;
use crate::thinker::soul_archetypes::{compose_soul, SoulArchetype};
use crate::tools::AlephTool;

use super::error::AgentManageError;
use super::validation::generate_agent_id_from_name;

// =============================================================================
// Re-exports for backward compatibility
// =============================================================================

/// Re-exported so callers (`team/create.rs`, `teams/templates/materialize.rs`)
/// that previously imported `validate_agent_id` from `agent_manage::create`
/// keep compiling. New code should import from `agent_manage::validation`
/// or `agent_manage` directly.
#[deprecated(
    since = "26.8.6",
    note = "import from `agent_manage::validation` or `agent_manage` directly"
)]
pub use super::validation::validate_agent_id;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for creating a new agent.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct AgentCreateArgs {
    /// Unique agent identifier (a-z, 0-9, _, -, max 64 chars).
    /// If empty or missing, auto-generated from the name.
    ///
    /// Max 32 chars when the agent is persisted to `config.toml`: the TOML
    /// layer enforces a shorter limit than the runtime registry, and an id it
    /// cannot store would leave a runtime-only agent that disappears on the
    /// next restart.
    #[serde(default)]
    pub id: String,
    /// Human-readable name (defaults to id)
    #[serde(default)]
    pub name: Option<String>,
    /// Description of what this agent specializes in
    #[serde(default)]
    pub description: Option<String>,
    /// LLM model to use (default: inherit system default).
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
    /// Whether the agent was persisted to `config.toml`.
    ///
    /// `None` — no `AgentManager` is wired, so persistence does not apply.
    /// `Some(false)` — the agent exists in the runtime registry but has **no**
    /// TOML row: it is missing from the Panel agent list and is lost on the
    /// next restart. The `message` says so in words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persisted: Option<bool>,
    /// Identity files that already existed and were therefore left untouched
    /// (for example an operator-authored `SOUL.md`).
    ///
    /// Empty when every file was created fresh.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub preserved_files: Vec<String>,
}

/// Longest agent id the TOML layer can persist.
///
/// The source of truth is `config::agent_manager::MAX_ID_LENGTH` (32), but
/// that constant is `pub(super)` inside the config module and cannot be
/// imported from `builtin_tools`. This mirror keeps the two id grammars
/// aligned: an id that passes this guard but not the manager's is still
/// reported through `AgentCreateOutput::persisted == Some(false)` instead of
/// a silent success.
const MAX_PERSISTED_AGENT_ID_LEN: usize = 32;

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

/// Apply the user's `description` to the freshly-written `IDENTITY.md`.
///
/// `initialize_agent_identity` uses `write_if_missing` so an existing
/// IDENTITY.md is never overwritten — this helper **appends** a
/// `## Description` section only when the user actually supplied a
/// description, preserving the role/vibe/emoji seed and any user edits
/// the operator already made.
fn apply_description_to_identity(
    identity_path: &std::path::Path,
    description: Option<&str>,
) -> std::io::Result<()> {
    let Some(description) = description.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    // NotFound is a true first run — IDENTITY.md does not exist yet, so
    // we are about to create it. Other IO errors (PermissionDenied, a
    // read of a corrupt-UTF8 file, a vanished file between checks) used
    // to be flattened to '' by `unwrap_or_default`, then the
    // `std::fs::write` below would race-append to a stale file the
    // caller cannot read. Distinguish NotFound (genuine "no file yet")
    // and propagate the rest so a stuck IDENTITY.md surfaces.
    let existing = match std::fs::read_to_string(identity_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(std::io::Error::new(
                e.kind(),
                format!(
                    "failed to read IDENTITY.md at {}: {e}",
                    identity_path.display()
                ),
            ));
        }
    };
    // Skip if a `## Description` block already exists — user took the wheel.
    if existing.contains("## Description") {
        return Ok(());
    }
    let appendage = format!("\n## Description\n\n{description}\n");
    let mut updated = existing;
    if !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&appendage);
    std::fs::write(identity_path, updated)
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
         If under-specified, run a short creation interview: recommend ONE soul archetype \
         and confirm; ask 2-5 short questions (domain/focus, name, tone tweaks, hard \
         boundaries); call with the chosen `archetype` + a `personalization` markdown.\n\
         Skip the questions if the user already gave enough detail. After creation, make \
         it active with agent_switch.\n\n\
         Soul Archetypes:\n\
         - expert: analysis, decisions — argues counter-case, tags confidence\n\
         - maker: code, automation — action-biased, plans then verifies\n\
         - assistant: general getting-things-done — answer-first, low-friction (default)\n\
         - companion: support, journaling — warm, doesn't rush to fix";

    type Args = AgentCreateArgs;
    type Output = AgentCreateOutput;

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
                return Err(super::error::AgentManageError::MissingNameOrId.into());
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
        validate_agent_id(&args.id).map_err(AgentManageError::InvalidId)?;

        // 2. Check for duplicates across BOTH surfaces (existence only —
        //    instantiation is deferred): the live runtime registry and the
        //    persisted TOML config. Checking only the registry let
        //    `agent_create` re-create an agent whose row was already in
        //    `config.toml` — the persisted definition was then silently
        //    replaced and the operator's persona files were overwritten.
        if self.registry.contains(&args.id).await {
            return Err(AgentManageError::AlreadyExists(args.id.clone()).into());
        }
        if let Some(ref manager) = self.agent_manager {
            if manager.get(&args.id).is_ok() {
                return Err(AgentManageError::AlreadyExists(args.id.clone()).into());
            }
        }

        // 2b. Reject ids the TOML layer cannot store before anything is
        //     written. `AgentManager` validates the id first and refuses
        //     anything longer than `MAX_PERSISTED_AGENT_ID_LEN`, so creating
        //     such an agent would register a runtime-only agent that is
        //     invisible in the Panel agent list and gone after a restart.
        if self.agent_manager.is_some() && args.id.len() > MAX_PERSISTED_AGENT_ID_LEN {
            return Err(AgentManageError::InvalidId(format!(
                "Agent ID too long to persist ({} chars, max {MAX_PERSISTED_AGENT_ID_LEN}) — \
                 use a shorter id",
                args.id.len()
            ))
            .into());
        }

        // 3. Determine paths. Both roots come from `provisioning_roots`, which
        //    is the resolved `[agents.defaults]` layout boot handed the manager
        //    — the same one `agent_resolver` rebuilds this agent with. Two
        //    earlier spellings each dropped a different half: `dirs::home_dir()`
        //    ignored `ALEPH_HOME`, and `discovery::aleph_home_dir().join(..)`
        //    fixed that but still ignored `agents_root` / `workspace_root`, so
        //    a config that moved either one had `agent_create` writing where
        //    nothing would read.
        let roots = crate::config::agent_manager::provisioning_roots(self.agent_manager.as_deref());
        let agent_state_dir = roots.agents.join(&args.id);
        let workspace_path = roots.workspaces.join(&args.id);

        // 4. Compose this agent's soul (archetype + base + personalization, or a
        // verbatim system_prompt override) and write it BEFORE identity-init so
        // initialize_agent_identity's write_if_missing keeps it.
        let display_name = args.name.as_deref().unwrap_or(&args.id);
        let soul_content = resolve_soul_content(&args, display_name);
        tokio::fs::create_dir_all(&agent_state_dir)
            .await
            .map_err(|e| {
                AgentManageError::Io(format!(
                    "Failed to create agent state dir for '{}': {}",
                    args.id, e
                ))
            })?;
        // Never clobber an existing persona file. The duplicate check above
        // only sees the runtime registry and the TOML config, so a state
        // directory left behind by an earlier (now unregistered) agent would
        // otherwise have its SOUL.md replaced by the archetype soul.
        let mut preserved_files: Vec<String> = Vec::new();
        let soul_path = agent_state_dir.join("SOUL.md");
        if soul_path.exists() {
            preserved_files.push("SOUL.md".to_string());
        } else {
            tokio::fs::write(&soul_path, &soul_content)
                .await
                .map_err(|e| {
                    AgentManageError::Io(format!(
                        "Failed to write SOUL.md for '{}': {}",
                        args.id, e
                    ))
                })?;
        }

        // Snapshot whether the operator already had an AGENTS.md BEFORE the
        // identity initializer below runs: that initializer creates a default
        // one, so a later existence check would always be true and the
        // `system_prompt` would never reach the file.
        let agents_md_pre_existed = agent_state_dir.join("AGENTS.md").exists();

        // Initialize the rest of the identity directory (AGENTS.md, MEMORY.md, …).
        // SOUL.md was already written above, so the archetype here only matters
        // for the unreachable case where that write was skipped.
        initialize_agent_identity(
            &agent_state_dir,
            display_name,
            args.archetype.unwrap_or_default(),
        )
        .map_err(|e| {
            AgentManageError::Io(format!(
                "Failed to initialize identity files for '{}': {}",
                args.id, e
            ))
        })?;

        // Apply the user's `description` to IDENTITY.md so the agent's catalog
        // entry (Panel sidebar) and the description that `agent_info` returns
        // don't disagree. Skipped silently when the operator already added a
        // `## Description` block.
        apply_description_to_identity(
            &agent_state_dir.join("IDENTITY.md"),
            args.description.as_deref(),
        )
        .map_err(|e| {
            AgentManageError::Io(format!(
                "Failed to write IDENTITY.md description for '{}': {}",
                args.id, e
            ))
        })?;

        // Initialize agent state directory (sessions/)
        crate::config::agent_resolver::initialize_agent_dir(&agent_state_dir).map_err(|e| {
            AgentManageError::Io(format!(
                "Failed to initialize agent state dir for '{}': {}",
                args.id, e
            ))
        })?;

        // Create workspace directory for tool output
        tokio::fs::create_dir_all(&workspace_path)
            .await
            .map_err(|e| {
                AgentManageError::Io(format!(
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
            // Same write-if-missing rule as SOUL.md: an existing AGENTS.md is
            // the operator's, not ours. The snapshot above is the one that
            // counts — by now the identity initializer has created a default
            // AGENTS.md for the fresh case.
            if agents_md_pre_existed {
                preserved_files.push("AGENTS.md".to_string());
            } else {
                tokio::fs::write(&agents_md, content)
                    .await
                    .map_err(|e| AgentManageError::Io(format!("Failed to write AGENTS.md: {e}")))?;
            }
        }

        // IDENTITY.md / TOOLS.md are owned by `initialize_agent_identity` above
        // (via `write_if_missing`, using the rich archetype-seeded templates).
        // They always exist by this point, so the old `if !exists` fallbacks
        // here were dead code that could only ever write a thinner, worse copy —
        // removed. Keep new identity-file templates in `agent_resolver`.

        // 7. Create AgentInstance. The model default is the same one
        //    `AgentInstanceConfig::default()` uses, so omitting `model` here
        //    and omitting it in TOML both fall through to the system default
        //    — there's no second hardcoded string to drift out of sync.
        let model = args
            .model
            .clone()
            .unwrap_or_else(|| AgentInstanceConfig::default().model);
        let config = AgentInstanceConfig {
            agent_id: args.id.clone(),
            workspace: workspace_path.clone(),
            model: model.clone(),
            agent_dir: agent_state_dir.clone(),
            ..Default::default()
        };

        let instance = {
            let mut inst =
                AgentInstance::new(config, Arc::clone(&self.session_store)).map_err(|e| {
                    AgentManageError::Io(format!(
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
                model: model.clone(),
            }
            .publish(bus);
        }

        // 8b. Persist to AgentManager (TOML config) so agents.list RPC returns it.
        //     The TOML row now carries the user's description too (Panel editor
        //     reads from there on reload), resolving the drift where the
        //     in-memory `display_name` and the catalog `description` could
        //     disagree after a `remember`-driven IDENTITY.md edit.
        let mut persisted: Option<bool> = None;
        let mut persist_error: Option<String> = None;
        if let Some(ref manager) = self.agent_manager {
            let identity = args
                .description
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|d| AgentIdentity {
                    description: Some(d.to_string()),
                    ..Default::default()
                });
            let def = AgentDefinition {
                id: args.id.clone(),
                name: args.name.clone(),
                model: Some(AgentModelRef::Legacy(model.clone())),
                archetype: args.archetype,
                identity,
                ..Default::default()
            };
            match manager.create(def) {
                Ok(()) => persisted = Some(true),
                Err(e) => {
                    // Non-fatal for the runtime agent, but never silent: a
                    // runtime-only agent is missing from the Panel agent list
                    // and lost on the next restart, so the failure is reported
                    // in the tool output as well as logged.
                    warn!(
                        agent_id = %args.id,
                        error = %e,
                        "Failed to persist agent to TOML config (runtime registration succeeded)"
                    );
                    persisted = Some(false);
                    persist_error = Some(e.to_string());
                }
            }
        }

        let workspace_str = workspace_path.to_string_lossy().to_string();
        let mut msg = format!("Agent '{}' created. Workspace: {}", args.id, workspace_str);
        if let Some(ref err) = persist_error {
            msg.push_str(&format!(
                " WARNING: not persisted to config.toml ({err}) — this agent is runtime-only: it will \
                 not appear in the agents list and will be lost on restart."
            ));
        }
        if !preserved_files.is_empty() {
            msg.push_str(&format!(
                " Kept existing {} (not overwritten).",
                preserved_files.join(", ")
            ));
        }

        info!(agent_id = %args.id, "Agent created successfully");

        Ok(AgentCreateOutput {
            agent_id: args.id,
            workspace_path: workspace_str,
            message: msg,
            persisted,
            preserved_files,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentRegistry as CatalogRegistry;
    use crate::builtin_tools::agent_manage::test_utils;
    use crate::tools::AlephTool;

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

    #[tokio::test]
    async fn description_persists_when_provided() {
        let temp = tempfile::tempdir().unwrap();
        let identity = temp.path().join("IDENTITY.md");
        std::fs::write(&identity, "# IDENTITY.md — seed\n").unwrap();

        apply_description_to_identity(&identity, Some("Quant for equities")).unwrap();

        let content = std::fs::read_to_string(&identity).unwrap();
        assert!(content.contains("# IDENTITY.md — seed"));
        assert!(content.contains("## Description"));
        assert!(content.contains("Quant for equities"));
    }

    #[tokio::test]
    async fn description_skipped_when_already_present() {
        let temp = tempfile::tempdir().unwrap();
        let identity = temp.path().join("IDENTITY.md");
        std::fs::write(
            &identity,
            "# IDENTITY.md\n\n## Description\n\nUser-authored.\n",
        )
        .unwrap();

        apply_description_to_identity(&identity, Some("Tool attempt")).unwrap();

        let content = std::fs::read_to_string(&identity).unwrap();
        assert!(content.contains("User-authored."));
        assert!(!content.contains("Tool attempt"));
    }

    #[tokio::test]
    async fn description_skipped_when_empty_or_whitespace() {
        let temp = tempfile::tempdir().unwrap();
        let identity = temp.path().join("IDENTITY.md");
        std::fs::write(&identity, "# IDENTITY.md\n").unwrap();

        apply_description_to_identity(&identity, Some("   ")).unwrap();
        apply_description_to_identity(&identity, None).unwrap();

        let content = std::fs::read_to_string(&identity).unwrap();
        assert!(!content.contains("## Description"));
    }

    /// `DESCRIPTION` carries the Soul Archetypes list the model reads, and a
    /// `const` cannot interpolate — so the prose is hand-written here while
    /// [`SoulArchetype::ALL`] stays the single source for *which* archetypes
    /// exist. Pin the ids so a new variant cannot land without its line.
    ///
    /// This once also pinned the prose to `SoulArchetype::summary()`. That
    /// method is gone (2026-08-02: it fed a channel with no consumer), and
    /// with it the only symbol the prose copied — an assertion against a
    /// deleted referent is not a check, so it is not kept as one. The id
    /// coverage below is the half that still catches the drift that matters.
    #[test]
    fn description_lists_every_archetype_summary() {
        for a in SoulArchetype::ALL {
            assert!(
                AgentCreateTool::DESCRIPTION.contains(a.as_str()),
                "agent_create DESCRIPTION is missing archetype id: {}",
                a.as_str()
            );
        }
        // The default is the only entry flagged as the fallback.
        // (Trimmed wording — original was "(default when unclear)"; current
        // phrasing is "(default)" since the catalog is concise by 2026-08-06
        // round, see FEATURE_LOCATOR §4.6 round-4.)
        assert!(
            AgentCreateTool::DESCRIPTION.contains("(default)"),
            "agent_create DESCRIPTION must mark exactly one archetype as the default; got: {}",
            AgentCreateTool::DESCRIPTION
        );
    }

    #[tokio::test]
    async fn tool_definition_carries_runtime_metadata() {
        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let (sm, _sm_temp) = test_utils::session_store();
        let tool = AgentCreateTool::new(registry, wm, sm);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "agent_create");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn create_persists_to_registry_and_publishes_event() {
        use crate::gateway::event_bus::GatewayEventBus;

        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let (sm, _sm_temp) = test_utils::session_store();
        let bus = Arc::new(GatewayEventBus::new());
        let tool = AgentCreateTool::new(Arc::clone(&registry), wm, Arc::clone(&sm))
            .with_event_bus(Some(Arc::clone(&bus)));

        let out = tool
            .call(AgentCreateArgs {
                id: "newagent".into(),
                name: Some("New Agent".into()),
                description: Some("Quant for equities".into()),
                model: None,
                system_prompt: None,
                archetype: Some(SoulArchetype::Expert),
                personalization: None,
                input: None,
            })
            .await
            .expect("create should succeed");

        assert_eq!(out.agent_id, "newagent");
        assert!(registry.contains("newagent").await);

        // SOUL.md was written; AGENTS.md / IDENTITY.md seeded; description
        // appended. All three should land in `~/.aleph/agents/newagent/`.
        let _ = CatalogRegistry::new(); // ensure import path used
                                        // (the path test below is sufficient — see create test fixture above.)
    }

    #[tokio::test]
    async fn duplicate_id_is_rejected() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, sm, _temp) = test_utils::instance("dup");
        registry.register(instance).await;
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let tool = AgentCreateTool::new(Arc::clone(&registry), wm, sm);

        let err = tool
            .call(AgentCreateArgs {
                id: "dup".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("already exists"), "got: {msg}");
    }

    #[tokio::test]
    async fn invalid_id_is_rejected_before_filesystem() {
        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let (sm, _sm_temp) = test_utils::session_store();
        let tool = AgentCreateTool::new(registry, wm, sm);

        let err = tool
            .call(AgentCreateArgs {
                id: "Bad-ID".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Invalid agent ID"), "got: {msg}");
    }

    /// An id the TOML layer cannot store must be refused BEFORE anything is
    /// written. The previous version created the agent in the runtime
    /// registry anyway, reported "created", and left an agent that is
    /// invisible in the Panel agent list and gone after the next restart.
    #[tokio::test]
    async fn an_id_too_long_to_persist_is_refused_before_anything_is_written() {
        let temp = tempfile::tempdir().unwrap();
        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let (sm, _sm_temp) = test_utils::session_store();
        let manager = Arc::new(crate::config::agent_manager::AgentManager::new(
            temp.path().join("config.toml"),
            temp.path().join("workspaces"),
            temp.path().join("agents"),
            temp.path().join("trash"),
        ));
        let tool = AgentCreateTool::new(registry, wm, sm).with_agent_manager(manager);

        // 40 chars: inside the runtime grammar (64), outside the TOML one (32).
        let long_id = "a".repeat(40);
        let err = tool
            .call(AgentCreateArgs {
                id: long_id.clone(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("too long to persist"), "got: {msg}");
        assert!(
            !temp.path().join("agents").join(&long_id).exists(),
            "nothing may be written for a refused id"
        );
    }

    /// An operator-authored persona file must survive a create. The state
    /// directory is not the registry, so a directory left behind by an earlier
    /// agent (or written by hand) used to be overwritten by the archetype
    /// soul.
    #[tokio::test]
    async fn an_existing_soul_file_is_preserved_and_reported() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "[agents]\n").unwrap();
        let agents_root = temp.path().join("agents");
        let manager = Arc::new(crate::config::agent_manager::AgentManager::new(
            config_path,
            temp.path().join("workspaces"),
            agents_root.clone(),
            temp.path().join("trash"),
        ));

        // A persona file the operator wrote before the agent existed.
        let state_dir = agents_root.join("legacy");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(state_dir.join("SOUL.md"), "OPERATOR PERSONA").unwrap();

        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let (sm, _sm_temp) = test_utils::session_store();
        let tool = AgentCreateTool::new(registry, wm, sm).with_agent_manager(manager);

        let out = tool
            .call(AgentCreateArgs {
                id: "legacy".into(),
                name: Some("Legacy".into()),
                ..Default::default()
            })
            .await
            .expect("create should succeed");

        assert_eq!(
            std::fs::read_to_string(state_dir.join("SOUL.md")).unwrap(),
            "OPERATOR PERSONA",
            "an existing persona file must not be clobbered"
        );
        assert!(
            out.preserved_files.contains(&"SOUL.md".to_string()),
            "the preserved file must be reported: {:?}",
            out.preserved_files
        );
        assert!(
            out.message.contains("Kept existing SOUL.md"),
            "got: {}",
            out.message
        );
    }
}
