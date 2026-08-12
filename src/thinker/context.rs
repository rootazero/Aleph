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
    /// Sub-agent dispatch ONLY: the parent session that spawned this run.
    ///
    /// `Some((parent_kind, parent_id))` for a subagent / team dispatch whose
    /// turn envelope carries a *parent* binding — used by
    /// `RuntimeContext::to_environment_context_block` to emit
    /// `<parent kind="…">id</parent>` so the
    /// model can disambiguate "I am the explore sub-agent of session X" from
    /// "I am the user's main session X". `None` on every primary dispatch —
    /// the printed prompt stays byte-identical for the common path.
    ///
    /// `parent_kind` is a stable, machine-readable discriminator chosen by the
    /// dispatcher (`"subagent"` / `"team"` / `"background"` …) so the model
    /// does not have to guess from a fragile id shape.
    pub parent: Option<EnvelopeParent>,
    /// Sub-agent dispatch ONLY: a stable, cheap correlation handle for the run
    /// (NOT the session key — the session is a session, this is "this turn of
    /// this sub-agent", which can be reset between tool-call retries). Used
    /// by `OperatingEnvelopeLayer` to print `<run_id>` in the dynamic tail so
    /// the model can refer to its current task in long delegations without
    /// `==`-name-matching against tool outputs. `None` on primary dispatch.
    pub run_id: Option<String>,
    /// Per-turn response language override — when `Some(lang)`, pushed onto
    /// `LanguageLayer`'s StackedLayer input as a *runtime fact* (so the layer
    /// reads it from the envelope the same place it reads cwd and exec_tier,
    /// not from a config field that travels by IncidentalThreading). `None`
    /// means "follow the agent's `[general] language`" — the legacy behaviour
    /// and identical prompt bytes.
    pub response_language: Option<String>,
    /// Whether this turn's prompt gets memory injected — curated memory, the
    /// wiki orientation index and per-query recall.
    ///
    /// Resolved by `execution_engine::turn_memory` (request > session >
    /// `[memory] enabled`) and consumed by `harness_bridge::prompt_build` at
    /// the one point those three envelopes converge. It rides the envelope
    /// rather than a new parameter because `build_system_prompt` already
    /// receives this struct, and because the fact belongs to the same family
    /// the envelope exists for: what regime this turn runs under.
    ///
    /// `None` means "not resolved" and behaves as **on** — every dispatch path
    /// that does not set it (sub-flows, token estimation, tests) must keep the
    /// behaviour it had before this knob existed. `Some(Off)` is the only value
    /// that suppresses anything, and it is also the only one the prompt
    /// mentions: the model needs to know its memory is muted, or it will
    /// explain its own amnesia by inventing a reason.
    pub memory_mode: Option<crate::memory::session_memory_mode::MemoryMode>,
}

impl TurnEnvelope {
    /// Whether the memory envelopes should be built for this turn.
    ///
    /// One derivation for the "unset means on" rule. Spelling it as
    /// `!matches!(env.memory_mode, Some(Off))` at each consumer is how a second
    /// consumer eventually gets the polarity backwards on the `None` case,
    /// which silently strips memory from every dispatch path that does not set
    /// the field.
    #[must_use]
    pub fn injects_memory(&self) -> bool {
        self.memory_mode
            .is_none_or(crate::memory::session_memory_mode::MemoryMode::injects)
    }
}

/// Sub-agent binding carried by [`TurnEnvelope::parent`]. Newtype so a future
/// field added here (parent_cwd, parent_model) can land without breaking the
/// public struct shape — a tuple of `String`s is not forward-compatible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeParent {
    /// Stable discriminator (`"subagent"` / `"team"` / `"background"` …).
    /// Lowercase ASCII, no spaces; passed straight into the prompt's
    /// `parent=<kind>` segment, so it must already be the form the model
    /// should see (no further escaping done by the printing side).
    pub kind: String,
    /// Parent session id in its stringified canonical form. Same shape as
    /// `SessionId::to_key_string()`.
    pub id: String,
}

impl TurnEnvelope {
    /// Envelope for dispatch paths that resolve no per-turn facts (internal
    /// tooling, sub-flows, token estimation). Named so a call site states the
    /// intent instead of spelling five `None`s.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// True iff every field is `None` — i.e. the dispatch path resolved no
    /// per-turn facts and the envelope contributes zero bytes to the prompt.
    /// Lets prompt-building code take a fast path (skip parent dispatch,
    /// skip the run-id line, etc.) without an `is_none` chain on every field.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.exec_tier.is_none()
            && self.session_mode.is_none()
            && self.cwd.is_none()
            && self.serving_model.is_none()
            && self.parent.is_none()
            && self.run_id.is_none()
            && self.response_language.is_none()
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

    /// Set only when this turn's memory injection is **muted**
    /// (`memory_mode = off`), rendered by `OperatingEnvelopeLayer` as the
    /// `Memory:` line.
    ///
    /// `false` — the overwhelmingly common case — renders nothing, so every
    /// prompt that does not mute memory stays byte-identical. The muted case
    /// must be stated: a model whose curated memory and note index were
    /// silently withheld does not conclude "they were withheld", it concludes
    /// it never knew, and then explains its own amnesia by inventing a reason
    /// or by re-asking things the user already told it.
    pub memory_muted: bool,
    /// Sub-agent dispatch ONLY: the parent session that spawned this run.
    /// Rendered by `RuntimeContextLayer` (priority 1720, Dynamic) as a
    /// nested `<parent kind="…">…</parent>` element inside the
    /// `<environment_context>` block, so the model can disambiguate "I am
    /// the explore sub-agent of session X" from the user's primary session.
    /// `None` on primary / user-facing sessions — the printed block stays
    /// byte-identical for the common path.
    pub envelope_parent: Option<EnvelopeParent>,
    /// Sub-agent dispatch ONLY: a stable, cheap correlation handle for the
    /// *current run* of the sub-agent — NOT the session key (a session can
    /// have many sub-agent invocations) and not the parent session (which
    /// already has `envelope_parent`). Rendered by
    /// `OperatingEnvelopeLayer` (priority 1758, Dynamic) as `- Run id: …`
    /// so the model can refer to its current task in long delegations
    /// without `==`-name-matching against tool outputs. `None` on primary
    /// dispatch.
    pub run_id: Option<String>,
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
            memory_muted: false,
            envelope_parent: None,
            run_id: None,
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

/// Unit tests for the envelope's own derivations.
#[cfg(test)]
mod memory_mode_envelope_tests {
    use super::TurnEnvelope;
    use crate::memory::session_memory_mode::MemoryMode;

    /// `None` must behave as ON. Every dispatch path that predates this knob
    /// leaves the field unset, and the polarity getting flipped here would
    /// silently strip memory from all of them — a change with no error, no
    /// failing test of its own, and a symptom ("the model forgot") that reads
    /// as a model problem.
    #[test]
    fn an_unset_memory_mode_still_injects() {
        assert!(TurnEnvelope::default().injects_memory());
    }

    #[test]
    fn only_off_suppresses_injection() {
        let on = TurnEnvelope {
            memory_mode: Some(MemoryMode::On),
            ..TurnEnvelope::default()
        };
        assert!(on.injects_memory());
        let off = TurnEnvelope {
            memory_mode: Some(MemoryMode::Off),
            ..TurnEnvelope::default()
        };
        assert!(!off.injects_memory());
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

#[cfg(test)]
mod envelope_tests {
    use super::*;

    #[test]
    fn none_is_fully_empty_for_a_zero_byte_default() {
        // The fast-path `is_empty` contract the prompt builder relies on
        // for the "no per-turn facts" dispatch paths (internal tooling,
        // sub-agent dispatch, token estimation). Defaulting `None` must
        // return `true`, every field set must return `false`.
        let env = TurnEnvelope::none();
        assert!(env.is_empty());
        assert_eq!(env, TurnEnvelope::default());
    }

    #[test]
    fn is_empty_returns_false_when_any_single_field_is_set() {
        // Boundary check: `is_empty` is per-field, not `Option<Enum>`. A
        // single parent binding is enough to make the envelope contribute
        // bytes to the prompt. Test one field per `Option<…>` to keep the
        // pin set tight.
        let mut env = TurnEnvelope::none();
        assert!(env.is_empty());

        env.exec_tier = Some(crate::config::types::policies::ExecTier::Ask);
        assert!(!env.is_empty(), "exec_tier must make envelope non-empty");

        env.exec_tier = None;
        env.session_mode = Some(crate::config::types::policies::SessionMode::Chat);
        assert!(!env.is_empty(), "session_mode must make envelope non-empty");

        env.session_mode = None;
        env.cwd = Some(std::path::PathBuf::from("/tmp"));
        assert!(!env.is_empty(), "cwd must make envelope non-empty");

        env.cwd = None;
        env.serving_model = Some("claude".into());
        assert!(
            !env.is_empty(),
            "serving_model must make envelope non-empty"
        );

        env.serving_model = None;
        env.parent = Some(EnvelopeParent {
            kind: "subagent".into(),
            id: "s-X".into(),
        });
        assert!(!env.is_empty(), "parent must make envelope non-empty");

        env.parent = None;
        env.run_id = Some("run-1".into());
        assert!(!env.is_empty(), "run_id must make envelope non-empty");

        env.run_id = None;
        env.response_language = Some("zh-Hans".into());
        assert!(
            !env.is_empty(),
            "response_language must make envelope non-empty"
        );
    }

    #[test]
    fn envelope_parent_carries_kind_and_id_separately() {
        // The struct is a newtype so future fields (parent_cwd, parent_run_id)
        // can land without breaking call sites; verify both members
        // round-trip independently through `PartialEq`.
        let parent = EnvelopeParent {
            kind: "subagent".into(),
            id: "session-X".into(),
        };
        assert_eq!(parent.kind, "subagent");
        assert_eq!(parent.id, "session-X");
        assert_eq!(parent, parent.clone());
    }
}
