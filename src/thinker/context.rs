//! Context Aggregator for Channel Capability Awareness
//!
//! Reconciles the interaction layer (what the channel can render) with the
//! security layer (what policy allows) into the [`EnvironmentContract`] the
//! prompt states, and carries the per-turn dynamic fragments the orchestrator
//! fills in afterwards (runtime context, sandbox posture, execution plan,
//! standing goal, strategy, voice, operating envelope).
//!
//! # Not a tool gate
//!
//! This module used to run a "two-phase filter" over a tool list —
//! `InteractionManifest::supports_tool` then `SecurityContext::check_tool` —
//! and hand back `available_tools` / `disabled_tools` for the prompt to list.
//! That was removed 2026-07-27, because it could only ever be wrong or
//! redundant:
//!
//! - **Nothing fed it.** The one production caller
//!   (`harness_bridge::prompt_build::resolve_prompt_context`) always passed an
//!   empty slice: tool schemas reach the model through native tool_use, not as
//!   prompt text — the same reason `ToolsLayer` was deleted a day earlier.
//! - **It would have been a second, weaker voice.** `check_tool` matched
//!   hardcoded *tool-name* substrings (`"bash"`, `"exec"`, `"web_search"`),
//!   while the enforced permission model is declared tool metadata × exec tier
//!   × sandbox floor, gated in `src/tools/scoped/`. Connecting it would have
//!   printed a verdict the gate does not apply — the same "two approval voices"
//!   defect [`SecurityContext::elevated_policy_note`] exists to prevent.
//!
//! The verdict the model *does* need — the enforced tier — is stated by
//! `OperatingEnvelopeLayer`; dormant capabilities are reported by
//! `ToolRuntimeStateLayer` from live health probes.

use serde::{Deserialize, Serialize};

use super::interaction::{
    Capability, InteractionConstraints, InteractionManifest, InteractionParadigm,
};
use super::security_context::SecurityContext;

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
    /// The paradigm-derived approval posture, kept OUT of
    /// [`Self::security_notes`] because it answers the same question as the
    /// resolved [`ExecTier`](crate::config::types::policies::ExecTier). Rendered
    /// by `SecurityLayer` only when [`ResolvedContext::approval_tier`] is absent,
    /// so exactly one approval regime — the enforced one — reaches the model.
    /// See [`SecurityContext::elevated_policy_note`].
    pub elevated_policy_note: Option<String>,
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

/// The per-turn operating envelope the gateway resolves and the prompt renders.
///
/// §2.3 "Context mode" had no named type: the three facts below were three loose
/// `Option<…>` positional parameters threaded through `FlowRequest` →
/// `HarnessRunner::run` → `build_system_prompt` → `resolve_prompt_context`. Two of
/// them (`cwd` and the adjacent `workspace_override`) are the *same* Rust type, so
/// the positional form let a caller swap them with no compiler complaint. Grouping
/// them makes the concept locatable, removes two positional parameters from an
/// already 15-argument trait method, and gives future envelope facts a home that
/// does not grow the signature.
///
/// Every field is `Option`al with the same contract: `None` means "this dispatch
/// path resolved no such fact", and the corresponding prompt line stays **absent**
/// so internal / sub-agent / token-estimate prompts remain byte-identical (and
/// therefore prompt-cache-stable) rather than rendering a guessed default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnEnvelope {
    /// Active execution-permission tier (Ask / Auto / Full) — the approval half
    /// of the envelope, rendered by `OperatingEnvelopeLayer` as `Approval mode:`
    /// and the Aleph equivalent of codex's `<approval_policy>`. Carries the tier
    /// *already resolved* by the gateway (request pill > session > global, after
    /// the channel clamp) — the exact tier the tool gate will enforce — so the
    /// prompt can never promise a regime the gate does not apply.
    pub exec_tier: Option<crate::config::types::policies::ExecTier>,
    /// Active session usage mode (chat / work / code) — the presentation half,
    /// rendered by `OperatingEnvelopeLayer` as `Usage mode:`. Names the partition the
    /// tool surface was built with so the model knows which families are
    /// deferred behind `tool_search` instead of learning it from failed calls.
    pub session_mode: Option<crate::config::types::policies::SessionMode>,
    /// The run's **effective** working directory: the project override when the
    /// user picked one, else the agent's `~/.aleph/workspaces/{id}`.
    ///
    /// This is the same value the gateway feeds the tool adapters as
    /// `default_working_dir`, i.e. the directory a shell tool call actually
    /// executes in. It anchors `RuntimeContext`'s `cwd=` / `repo=` / `git=`
    /// segments. Before it was threaded, those three were derived from
    /// `std::env::current_dir()` — the *daemon's* directory — so the prompt
    /// advertised a path where no tool ran and, in project mode, reported the
    /// daemon's git branch as the project's.
    pub cwd: Option<std::path::PathBuf>,
    /// The model actually serving this turn, as `RuntimeContext`'s `model=`
    /// segment.
    ///
    /// Must be the resolved serving model, never `provider.name()`. Every
    /// production `llm` is a `FailoverProvider` (often wrapped again in
    /// Metering / ModelOverride, which delegate `name()`), so `name()` is the
    /// literal string `"failover"` — which is what the envelope shipped to the
    /// model on every turn. The envelope exists to be the single source of
    /// truth for facts the model cannot otherwise know, and this is the one
    /// fact it was stating wrongly: the model could not tell which model it
    /// was, so it could not pace itself against its own context window or
    /// answer honestly when asked. Silent in both directions — the string is
    /// well-formed and constant, so nothing looks broken.
    ///
    /// `runner_impl` resolves this as `gauge_model` (serving-model hint →
    /// routing model id → provider name) before the loop, and the gauge and
    /// cost estimate key off the same value, so all three agree by
    /// construction. `None` leaves the previous behaviour for dispatch paths
    /// that resolve no model.
    pub serving_model: Option<String>,
}

impl TurnEnvelope {
    /// Envelope for dispatch paths that resolve no per-turn facts (internal
    /// tooling, sub-flows, token estimation). Named so a call site states the
    /// intent instead of spelling three `None`s.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }
}

/// Everything the prompt layers read about *this* turn.
///
/// [`Self::environment_contract`] is resolved up front by
/// [`ContextAggregator::resolve`]; every other field is filled in afterwards by
/// `harness_bridge::prompt_build::resolve_prompt_context` from a session-keyed
/// lookup, and each has exactly one rendering layer. A `None` field means its
/// layer emits nothing — which keeps the prompt byte-identical, and therefore
/// prompt-cache-stable, for sessions that carry no such state.
#[derive(Debug, Clone)]
pub struct ResolvedContext {
    /// Environment contract describing the working context
    pub environment_contract: EnvironmentContract,
    /// Optional runtime context for micro-environmental awareness
    pub runtime_context: Option<super::runtime_context::RuntimeContext>,
    /// Aggregated per-tool runtime state fragments. Populated by the
    /// orchestrator before prompt assembly; rendered by
    /// `ToolRuntimeStateLayer` (priority 1703, Dynamic) as `<tool_runtime_state>`
    /// XML. Empty when no opt-in tools have anything to say.
    pub runtime_state_blocks: Vec<crate::tools::runtime_state::RuntimeStateFragment>,
    /// Active sandbox posture for system-prompt injection (codex-inspired).
    /// Populated by the orchestrator from `Sandbox::summary()` before
    /// prompt assembly; rendered by `SecurityLayer` (priority 600). `None`
    /// keeps the sandbox section absent (e.g. mock sandboxes in tests).
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
    pub execution_plan: Option<String>,
    /// Active standing-goal summary, rendered by `StandingGoalLayer`
    /// (priority 1755) as `<standing_goal>`. Populated from `GoalStore` in
    /// the harness bridge; `None` (no active goal) emits nothing.
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
    pub timer_loop: Option<String>,
    /// Full `<strategy>` body for `StrategyLayer` (priority 70, Stable),
    /// rendered once from the session's active `Strategy` via
    /// `render_strategy_summary`. Populated in the harness bridge from
    /// `active_strategy`; `None` (no planned Strategy) emits nothing, leaving
    /// the cacheable stable prefix byte-identical.
    pub strategy: Option<String>,
    /// Guardrail lines for `StrategyPointerLayer` (priority 1757, Dynamic),
    /// rendered from the same `Strategy` via `render_guardrails_only` and
    /// echoed near the read head every turn to fight goal-drift. Populated in
    /// the harness bridge; `None` emits nothing (byte-identical tail).
    pub strategy_guardrails: Option<String>,
    /// Voice context for this session, rendered by `VoiceModeLayer`
    /// (priority 1710) as the spoken-reply guidelines. Populated in the harness
    /// bridge from `voice::voice_mode` (written by the gateway inbound
    /// router). [`VoiceContext::Off`] keeps the section absent — the prompt is
    /// byte-identical for non-voice turns.
    pub voice: VoiceContext,
    /// Domain-vocabulary hint (`[voice] vocabulary`) carried alongside
    /// [`Self::voice`], rendered by `VoiceModeLayer` on ASR-transcribed turns
    /// only: the same term list that biased the recognizer is shown to the
    /// model so it repairs misrecognized words toward the configured terms
    /// (one dictionary, two consumers). `None` when no vocabulary is
    /// configured or the turn is not voice — the layer then emits the plain
    /// repair rule, keeping the prompt byte-identical.
    pub voice_vocabulary: Option<String>,
    /// Active execution-permission tier (Ask / Auto / Full), rendered by
    /// `OperatingEnvelopeLayer` (priority 1758, **Dynamic**) as the
    /// `Approval mode:` line. This is the
    /// approval half of the operating envelope — the complement of
    /// [`Self::sandbox_summary`]'s filesystem/network half — and the Aleph
    /// equivalent of codex's `<approval_policy>`. Populated in the harness
    /// bridge from the turn's resolved [`ExecTier`](crate::config::types::policies::ExecTier)
    /// (request pill > session > global, after the channel clamp — the exact
    /// tier the tool gate will enforce). `None` on internal / subagent dispatch
    /// that carries no resolved tier, keeping their prompt byte-identical.
    ///
    /// It lives in a **Dynamic** layer, not in `SecurityLayer` @600 (Stable),
    /// because the user can flip the composer pill mid-conversation: a Stable-zone
    /// byte change invalidates the whole conversation's prompt cache.
    pub approval_tier: Option<crate::config::types::policies::ExecTier>,

    /// Active session usage mode (chat / work / code), rendered by
    /// `OperatingEnvelopeLayer` (priority 1758, **Dynamic**) beside the approval
    /// line as the `Usage mode:` line —
    /// the presentation half of the operating envelope: which register the
    /// session runs in and how its tool surface was partitioned
    /// (schema-resident vs deferred). Populated in the harness bridge from
    /// the turn's resolved
    /// [`SessionMode`](crate::config::types::policies::SessionMode)
    /// (request pill > session > global). `None` on internal / subagent
    /// dispatch, keeping their prompt byte-identical.
    pub session_mode: Option<crate::config::types::policies::SessionMode>,
}

/// Reconciles the interaction manifest with the security context into the
/// [`EnvironmentContract`], and seeds every per-turn fragment to "absent".
///
/// The caller (`harness_bridge::prompt_build::resolve_prompt_context`) then
/// fills the fragments it can resolve for this session. See the module docs for
/// why this is not a tool gate.
pub struct ContextAggregator;

impl ContextAggregator {
    /// Resolve the environment contract and hand back a [`ResolvedContext`]
    /// whose per-turn fragments are all absent, ready for the caller to fill.
    #[must_use]
    pub fn resolve(
        interaction: &InteractionManifest,
        security: &SecurityContext,
    ) -> ResolvedContext {
        ResolvedContext {
            environment_contract: Self::build_contract(interaction, security),
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
            voice_vocabulary: None,
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
            elevated_policy_note: security.elevated_policy_note(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_environment_contract() {
        let interaction = InteractionManifest::new(InteractionParadigm::CLI);
        let security = SecurityContext::strict_readonly(PathBuf::from("/workspace"));

        let resolved = ContextAggregator::resolve(&interaction, &security);

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
        let ctx = ContextAggregator::resolve(&interaction, &SecurityContext::permissive());
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
        let ctx_a = ContextAggregator::resolve(&a, &SecurityContext::permissive());
        let ctx_b = ContextAggregator::resolve(&b, &SecurityContext::permissive());
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
        );
        // Both strategy surfaces default to absent so the prompt is
        // byte-identical for sessions with no planned Strategy.
        assert!(ctx.strategy.is_none());
        assert!(ctx.strategy_guardrails.is_none());
    }
}
