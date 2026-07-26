//! Context Aggregator for Channel Capability Awareness
//!
//! This module RECONCILES the interaction layer (what's technically possible)
//! with the security layer (what's allowed by policy) through two-phase filtering.
//!
//! # Architecture
//!
//! ```text
//! ┌────────────────────────────────────────────────────────────────────────────┐
//! │                         ContextAggregator                                  │
//! │                                                                            │
//! │  ┌──────────────────────┐        ┌──────────────────────┐                  │
//! │  │ InteractionManifest  │        │   SecurityContext    │                  │
//! │  │ (technical caps)     │        │   (policy rules)     │                  │
//! │  └──────────┬───────────┘        └──────────┬───────────┘                  │
//! │             │                               │                              │
//! │             ▼                               ▼                              │
//! │  ┌─────────────────────────────────────────────────────────────────────┐   │
//! │  │                    Two-Phase Filtering                              │   │
//! │  │                                                                     │   │
//! │  │  Phase 1: InteractionManifest.supports_tool()                       │   │
//! │  │           → UnsupportedByChannel (silent filter)                    │   │
//! │  │                                                                     │   │
//! │  │  Phase 2: SecurityContext.check_tool()                              │   │
//! │  │           → BlockedByPolicy / RequiresApproval (transparent)        │   │
//! │  └─────────────────────────────────────────────────────────────────────┘   │
//! │                                    │                                       │
//! │                                    ▼                                       │
//! │  ┌─────────────────────────────────────────────────────────────────────┐   │
//! │  │                       ResolvedContext                               │   │
//! │  │  • available_tools: Vec<ToolInfo>                                   │   │
//! │  │  • disabled_tools: Vec<DisabledTool>                                │   │
//! │  │  • environment_contract: EnvironmentContract                        │   │
//! │  └─────────────────────────────────────────────────────────────────────┘   │
//! └────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Examples
//!
//! ```
//! use alephcore::thinker::{ContextAggregator, InteractionManifest, InteractionParadigm};
//! use alephcore::thinker::security_context::SecurityContext;
//! use alephcore::tools::info::ToolInfo;
//!
//! // Create manifests for WebRich channel with permissive security
//! let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
//! let security = SecurityContext::permissive();
//!
//! // Sample tools
//! let tools = vec![
//!     ToolInfo {
//!         name: "web_search".to_string(),
//!         description: "Search the web".to_string(),
//!         parameters_schema: None,
//!     },
//! ];
//!
//! // Resolve context
//! let resolved = ContextAggregator::resolve(&interaction, &security, &tools);
//! assert_eq!(resolved.available_tools.len(), 1);
//! assert!(resolved.disabled_tools.is_empty());
//! ```

use serde::{Deserialize, Serialize};

use super::interaction::{
    Capability, InteractionConstraints, InteractionManifest, InteractionParadigm,
};
use super::security_context::{SecurityContext, ToolPermission};
use crate::tools::info::ToolInfo;

/// Reason why a tool is disabled
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum DisableReason {
    /// Tool is not supported by the current channel (silent filter)
    ///
    /// This is used for interaction-layer filtering. The AI should not
    /// mention these tools as they are technically unavailable.
    UnsupportedByChannel,

    /// Tool is blocked by security policy (transparent to AI)
    ///
    /// The AI is informed about this restriction so it can explain
    /// to the user why the tool is unavailable.
    BlockedByPolicy {
        /// Reason for the policy block
        reason: String,
    },

    /// Tool requires user approval before execution
    ///
    /// The tool is available but execution requires explicit approval.
    /// The AI should inform the user before attempting to use it.
    RequiresApproval {
        /// Prompt to display for approval
        prompt: String,
    },

    /// Tool's runtime health probe reports unhealthy (transparent to AI)
    ///
    /// The tool catalog's `ToolHealthCache` ran a probe (Docker daemon
    /// liveness, auth token presence, depth budget, etc.) and recorded
    /// an Unhealthy result. The AI is shown the short label so it can
    /// explain to the user why the capability is dormant.
    Unhealthy {
        /// Short reason label from the health probe.
        reason: String,
    },
}

/// A tool that is disabled with a specific reason
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisabledTool {
    /// Tool name
    pub name: String,
    /// Reason for disabling
    pub reason: DisableReason,
}

/// Contract describing the current environment for the AI
///
/// This struct provides a unified view of what the AI can do in the
/// current environment, combining interaction capabilities with
/// security constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentContract {
    /// The interaction paradigm (CLI, `WebRich`, Messaging, etc.)
    pub paradigm: InteractionParadigm,
    /// Active capabilities in this environment
    pub active_capabilities: Vec<Capability>,
    /// Interaction constraints (output limits, streaming, etc.)
    pub constraints: InteractionConstraints,
    /// Security notes to include in system prompt
    pub security_notes: Vec<String>,
}

/// What kind of voice context this turn is in.
///
/// Replaces the bare `voice_mode_active: bool` so `VoiceModeLayer` can adapt the
/// spoken-reply contract to *why* voice is on, not just *whether*. Two distinct
/// facts the gateway already computes flow in here:
/// - the reply is read aloud (TTS), and
/// - the user's message arrived as ASR-transcribed speech.
///
/// The second fact was previously discarded (the inbound router OR-collapsed it
/// into one boolean). Modelling it as an enum makes the illegal "transcribed but
/// not spoken" state unrepresentable — the layer only ever sees the variants
/// below. R7/R10: this is a mechanical record of gateway facts, no judgment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VoiceContext {
    /// Not a voice turn — `VoiceModeLayer` emits nothing (prompt byte-identical).
    #[default]
    Off,
    /// The reply is read aloud; the user typed their message.
    Spoken,
    /// The reply is read aloud **and** the user's message arrived as
    /// ASR-transcribed speech — the layer additionally invites the model to
    /// repair transcription artifacts.
    SpokenTranscribed,
}

impl VoiceContext {
    /// True for any spoken turn (everything except [`Self::Off`]).
    #[must_use]
    pub const fn is_active(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// True when this turn's user input was ASR-transcribed speech.
    #[must_use]
    pub const fn is_transcribed(self) -> bool {
        matches!(self, Self::SpokenTranscribed)
    }
}

/// Resolved context after two-phase filtering
///
/// This is the final output of the `ContextAggregator`, containing:
/// - Tools that are fully available for use
/// - Tools that are disabled (with reasons)
/// - The environment contract describing the AI's working context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedContext {
    /// Tools available for the AI to use
    pub available_tools: Vec<ToolInfo>,
    /// Tools that are disabled with reasons
    pub disabled_tools: Vec<DisabledTool>,
    /// Environment contract describing the working context
    pub environment_contract: EnvironmentContract,
    /// Optional runtime context for micro-environmental awareness
    #[serde(skip)]
    pub runtime_context: Option<super::runtime_context::RuntimeContext>,
    /// Aggregated per-tool runtime state fragments. Populated by the
    /// orchestrator before prompt assembly; rendered by
    /// `ToolRuntimeStateLayer` (priority 502) as `<tool_runtime_state>`
    /// XML. Empty when no opt-in tools have anything to say.
    #[serde(skip, default)]
    pub runtime_state_blocks: Vec<crate::tools::runtime_state::RuntimeStateFragment>,
    /// Active sandbox posture for system-prompt injection (codex-inspired).
    /// Populated by the orchestrator from `Sandbox::summary()` before
    /// prompt assembly; rendered by `SecurityLayer` (priority 600). `None`
    /// keeps the sandbox section absent (e.g. mock sandboxes in tests).
    #[serde(skip, default)]
    pub sandbox_summary: Option<crate::sandbox::SandboxSummary>,
    /// Compact progress snapshot of the session's active scratchpad
    /// execution list (objective + checklist + current step), rendered by
    /// `ExecutionPlanLayer` (priority 1756) as `<execution_plan>`. Populated
    /// by the orchestrator from `scratchpad_registry::active` before prompt
    /// assembly so the live plan stays in context across long tool-only
    /// stretches where the model never re-calls the `scratchpad` tool —
    /// codex `update_plan` / opencode `todowrite` / Claude Code `TodoWrite`
    /// persistent-visibility parity. `None` when no active plan with pending
    /// work, so the layer emits nothing and the prompt is byte-identical.
    #[serde(skip, default)]
    pub execution_plan: Option<String>,
    /// Active standing-goal summary, rendered by `StandingGoalLayer`
    /// (priority 1755) as `<standing_goal>`. Populated from `GoalStore` in
    /// the harness bridge; `None` (no active goal) emits nothing.
    #[serde(skip, default)]
    pub standing_goal: Option<String>,
    /// Governance-topology context for a session that is a registered node
    /// in the loop graph. Rendered by `GraphTopologyLayer` (priority 1754)
    /// as `<loop_graph_context>`. Populated from `LoopGraphStore` in
    /// `harness_bridge::prompt_build`; `None` (the common case) leaves the
    /// prompt byte-identical.
    pub graph_topology: Option<String>,
    /// Active timer-loop summary (watch prompt + status), rendered by
    /// `TimerLoopLayer` (priority 1753) as `<timer_loop>`. Populated from
    /// the loop registry in the harness bridge; `None` (no active loop)
    /// emits nothing.
    #[serde(skip, default)]
    pub timer_loop: Option<String>,
    /// Full `<strategy>` body for `StrategyLayer` (priority 70, Stable),
    /// rendered once from the session's active `Strategy` via
    /// `render_strategy_summary`. Populated in the harness bridge from
    /// `active_strategy`; `None` (no planned Strategy) emits nothing, leaving
    /// the cacheable stable prefix byte-identical.
    #[serde(skip, default)]
    pub strategy: Option<String>,
    /// Guardrail lines for `StrategyPointerLayer` (priority 1757, Dynamic),
    /// rendered from the same `Strategy` via `render_guardrails_only` and
    /// echoed near the read head every turn to fight goal-drift. Populated in
    /// the harness bridge; `None` emits nothing (byte-identical tail).
    #[serde(skip, default)]
    pub strategy_guardrails: Option<String>,
    /// Voice context for this session, rendered by `VoiceModeLayer`
    /// (priority 1710) as the spoken-reply guidelines. Populated in the harness
    /// bridge from `voice::voice_mode` (written by the gateway inbound
    /// router). [`VoiceContext::Off`] keeps the section absent — the prompt is
    /// byte-identical for non-voice turns.
    #[serde(skip, default)]
    pub voice: VoiceContext,
    /// Active execution-permission tier (Ask / Auto / Full), rendered by
    /// `SecurityLayer` (priority 600) as the `Approval mode:` line. This is the
    /// approval half of the operating envelope — the complement of
    /// [`Self::sandbox_summary`]'s filesystem/network half — and the Aleph
    /// equivalent of codex's `<approval_policy>`. Populated in the harness
    /// bridge from the turn's resolved [`ExecTier`](crate::config::types::policies::ExecTier)
    /// (request pill > session > global, after the channel clamp — the exact
    /// tier the tool gate will enforce). `None` on internal / subagent dispatch
    /// that carries no resolved tier, keeping their prompt byte-identical.
    #[serde(skip, default)]
    pub approval_tier: Option<crate::config::types::policies::ExecTier>,

    /// Active session usage mode (chat / work / code), rendered by
    /// `SecurityLayer` beside the approval line as the `Usage mode:` line —
    /// the presentation half of the operating envelope: which register the
    /// session runs in and how its tool surface was partitioned
    /// (schema-resident vs deferred). Populated in the harness bridge from
    /// the turn's resolved
    /// [`SessionMode`](crate::config::types::policies::SessionMode)
    /// (request pill > session > global). `None` on internal / subagent
    /// dispatch, keeping their prompt byte-identical.
    #[serde(skip, default)]
    pub session_mode: Option<crate::config::types::policies::SessionMode>,
}

/// Context Aggregator for reconciling interaction and security layers
///
/// The `ContextAggregator` performs two-phase filtering:
///
/// 1. **Interaction Phase**: Filter based on channel capabilities
///    - Tools unsupported by the channel are silently removed
///
/// 2. **Security Phase**: Filter based on security policy
///    - Tools blocked by policy are disabled with transparent reasons
///    - Tools requiring approval are marked but still available
pub struct ContextAggregator;

impl ContextAggregator {
    /// Resolve the final context from interaction manifest, security context, and tools
    ///
    /// This performs two-phase filtering:
    /// 1. Check if tool is supported by interaction layer (silent filter)
    /// 2. Check security policy (transparent filter with reasons)
    ///
    /// Tools requiring approval are added to BOTH `available_tools` AND `disabled_tools`
    /// so the AI knows they exist but also knows approval is needed.
    #[must_use]
    pub fn resolve(
        interaction: &InteractionManifest,
        security: &SecurityContext,
        all_tools: &[ToolInfo],
    ) -> ResolvedContext {
        let mut available_tools = Vec::new();
        let mut disabled_tools = Vec::new();

        for tool in all_tools {
            // Phase 1: Interaction layer filter (silent)
            if !interaction.supports_tool(&tool.name) {
                disabled_tools.push(DisabledTool {
                    name: tool.name.clone(),
                    reason: DisableReason::UnsupportedByChannel,
                });
                continue;
            }

            // Phase 2: Security layer filter (transparent)
            match security.check_tool(&tool.name) {
                ToolPermission::Allowed => {
                    available_tools.push(tool.clone());
                }
                ToolPermission::Denied { reason } => {
                    disabled_tools.push(DisabledTool {
                        name: tool.name.clone(),
                        reason: DisableReason::BlockedByPolicy { reason },
                    });
                }
                ToolPermission::RequiresApproval { prompt } => {
                    // Tool is available but marked as requiring approval
                    // Add to BOTH lists so AI knows it exists and knows approval is needed
                    available_tools.push(tool.clone());
                    disabled_tools.push(DisabledTool {
                        name: tool.name.clone(),
                        reason: DisableReason::RequiresApproval { prompt },
                    });
                }
            }
        }

        let environment_contract = Self::build_contract(interaction, security);

        ResolvedContext {
            available_tools,
            disabled_tools,
            environment_contract,
            runtime_context: None,
            runtime_state_blocks: Vec::new(),
            sandbox_summary: None,
            execution_plan: None,
            standing_goal: None,
            graph_topology: None,
            timer_loop: None,
            strategy: None,
            strategy_guardrails: None,
            voice: VoiceContext::Off,
            approval_tier: None,
            session_mode: None,
        }
    }

    /// Build the environment contract from interaction and security contexts
    fn build_contract(
        interaction: &InteractionManifest,
        security: &SecurityContext,
    ) -> EnvironmentContract {
        let mut active_capabilities: Vec<Capability> =
            interaction.capabilities.iter().cloned().collect();
        active_capabilities.sort_by_key(|c| c.prompt_hint().0);
        EnvironmentContract {
            paradigm: interaction.paradigm,
            active_capabilities,
            constraints: interaction.constraints.clone(),
            security_notes: security.security_notes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_tool(name: &str) -> ToolInfo {
        ToolInfo {
            name: name.to_string(),
            description: format!("{} tool description", name),
            parameters_schema: None,
        }
    }

    #[test]
    fn test_all_tools_available_in_permissive() {
        // WebRich + permissive = all tools should be available
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();

        let tools = vec![
            make_tool("web_search"),
            make_tool("file_ops"),
            make_tool("bash"),
            make_tool("canvas"),
        ];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        // All tools should be available
        assert_eq!(resolved.available_tools.len(), 4);

        // No tools should be disabled
        assert!(resolved.disabled_tools.is_empty());
    }

    #[test]
    fn test_security_blocks_tool() {
        // CLI + strict_readonly should block file_ops and exec tools
        let interaction = InteractionManifest::new(InteractionParadigm::CLI);
        let security = SecurityContext::strict_readonly(PathBuf::from("/workspace"));

        let tools = vec![
            make_tool("web_search"),
            make_tool("file_ops"),
            make_tool("exec"),
            make_tool("read_skill"),
        ];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        // Only read_skill should be available (web_search blocked by network policy)
        let available_names: Vec<&str> = resolved
            .available_tools
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(available_names.contains(&"read_skill"));
        assert!(!available_names.contains(&"file_ops"));
        assert!(!available_names.contains(&"exec"));

        // file_ops, exec should be in disabled list with BlockedByPolicy
        let file_ops_disabled = resolved
            .disabled_tools
            .iter()
            .find(|d| d.name == "file_ops")
            .expect("file_ops should be in disabled list");
        assert!(matches!(
            file_ops_disabled.reason,
            DisableReason::BlockedByPolicy { .. }
        ));

        let exec_disabled = resolved
            .disabled_tools
            .iter()
            .find(|d| d.name == "exec")
            .expect("exec should be in disabled list");
        assert!(matches!(
            exec_disabled.reason,
            DisableReason::BlockedByPolicy { .. }
        ));
    }

    #[test]
    fn test_requires_approval_shows_both() {
        // Standard sandbox requires approval for exec tools
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::standard_sandbox(PathBuf::from("/workspace"));

        let tools = vec![make_tool("web_search"), make_tool("bash")];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        // bash should be in available_tools
        let available_names: Vec<&str> = resolved
            .available_tools
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(available_names.contains(&"bash"));
        assert!(available_names.contains(&"web_search"));

        // bash should ALSO be in disabled_tools with RequiresApproval
        let bash_disabled = resolved
            .disabled_tools
            .iter()
            .find(|d| d.name == "bash")
            .expect("bash should be in disabled list");
        assert!(matches!(
            bash_disabled.reason,
            DisableReason::RequiresApproval { .. }
        ));

        // web_search should NOT be in disabled_tools
        let web_disabled = resolved
            .disabled_tools
            .iter()
            .find(|d| d.name == "web_search");
        assert!(web_disabled.is_none());
    }

    #[test]
    fn test_environment_contract() {
        let interaction = InteractionManifest::new(InteractionParadigm::CLI);
        let security = SecurityContext::strict_readonly(PathBuf::from("/workspace"));

        let resolved = ContextAggregator::resolve(&interaction, &security, &[]);

        let contract = &resolved.environment_contract;

        // Check paradigm
        assert_eq!(contract.paradigm, InteractionParadigm::CLI);

        // Check capabilities (CLI has RichText, CodeHighlight, Streaming)
        assert!(contract.active_capabilities.contains(&Capability::RichText));
        assert!(contract
            .active_capabilities
            .contains(&Capability::Streaming));
        assert!(!contract.active_capabilities.contains(&Capability::Canvas));

        // Check constraints
        assert!(!contract.constraints.prefer_compact);

        // Check security notes (strict mode should have several notes)
        assert!(!contract.security_notes.is_empty());
        assert!(contract.security_notes.iter().any(|n| n.contains("Strict")));
        assert!(contract
            .security_notes
            .iter()
            .any(|n| n.contains("Network Access: Disabled")));
    }

    #[test]
    fn test_canvas_filtered_by_interaction() {
        // CLI doesn't support canvas capability
        let interaction = InteractionManifest::new(InteractionParadigm::CLI);
        let security = SecurityContext::permissive();

        let tools = vec![make_tool("web_search"), make_tool("canvas")];

        let resolved = ContextAggregator::resolve(&interaction, &security, &tools);

        // web_search should be available
        let available_names: Vec<&str> = resolved
            .available_tools
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert!(available_names.contains(&"web_search"));

        // canvas should NOT be in available_tools (filtered by interaction layer)
        assert!(!available_names.contains(&"canvas"));

        // canvas should be in disabled_tools with UnsupportedByChannel
        let canvas_disabled = resolved
            .disabled_tools
            .iter()
            .find(|d| d.name == "canvas")
            .expect("canvas should be in disabled list");
        assert!(matches!(
            canvas_disabled.reason,
            DisableReason::UnsupportedByChannel
        ));
    }
}

#[cfg(test)]
mod active_capabilities_order_tests {
    use super::*;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;
    use std::collections::HashSet;

    fn render_capability_names(ctx: &ResolvedContext) -> Vec<String> {
        ctx.environment_contract
            .active_capabilities
            .iter()
            .map(|c| c.prompt_hint().0.to_string())
            .collect()
    }

    #[test]
    fn active_capabilities_are_sorted_by_name() {
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let ctx = ContextAggregator::resolve(&interaction, &SecurityContext::permissive(), &[]);
        let names = render_capability_names(&ctx);
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "active_capabilities must be sorted");
        assert!(
            names.contains(&"canvas".to_string()),
            "WebRich must include canvas, got {names:?}"
        );
    }

    #[test]
    fn active_capabilities_order_is_independent_of_set_insertion_order() {
        let mut a = InteractionManifest::new(InteractionParadigm::WebRich);
        let mut b = InteractionManifest::new(InteractionParadigm::WebRich);
        let cap_set: HashSet<Capability> = a.capabilities.iter().cloned().collect();
        b.capabilities = cap_set.clone();
        a.capabilities = cap_set;
        let ctx_a = ContextAggregator::resolve(&a, &SecurityContext::permissive(), &[]);
        let ctx_b = ContextAggregator::resolve(&b, &SecurityContext::permissive(), &[]);
        assert_eq!(
            render_capability_names(&ctx_a),
            render_capability_names(&ctx_b),
            "render order must not depend on HashSet iteration"
        );
    }
}

#[cfg(test)]
mod strategy_field_tests {
    use super::*;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    #[test]
    fn resolve_defaults_strategy_fields_to_none() {
        let ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        // Both strategy surfaces default to absent so the prompt is
        // byte-identical for sessions with no planned Strategy.
        assert!(ctx.strategy.is_none());
        assert!(ctx.strategy_guardrails.is_none());
    }
}
