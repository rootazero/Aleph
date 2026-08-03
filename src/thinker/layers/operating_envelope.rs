//! `OperatingEnvelopeLayer` — the per-run/per-turn envelope facts (approval
//! tier, usage mode, writable roots) at priority 1758 (Dynamic).
//!
//! These two lines used to live in `SecurityLayer` (@600), which declares no
//! `stability()` and therefore inherits `LayerStability::Stable` — putting them in
//! the **cacheable prefix**, ahead of every message-level prompt-cache breakpoint.
//! But both are per-turn knobs the user (or the model, via `session_set_mode` /
//! `self_config`) can flip mid-conversation: `resolve_turn_permissions` re-resolves
//! the tier on every request and the composer pills exist precisely so they can
//! change. Flipping either on turn 40 of a long session rewrote a byte inside the
//! Stable part and invalidated the whole conversation's cached prefix — the growing
//! history got re-WRITTEN at 1.25x instead of READ at 0.1x, for a one-word change.
//!
//! Flipping `SecurityLayer::stability()` was not an option: at priority 600 it sits
//! among Stable layers running to ~1700, and `stable_layers_come_before_dynamic`
//! requires the two zones not to interleave. So the volatile half moves out to the
//! Dynamic tail instead, alongside `voice_mode` (1710) / `runtime_context` (1720) /
//! `strategy_pointer` (1757) — the same Stable-head / Dynamic-echo split
//! `StrategyLayer` @70 and `StrategyPointerLayer` @1757 already use.
//!
//! `SecurityLayer` keeps the genuinely session-stable half: the paradigm-derived
//! security notes and the sandbox posture.
//!
//! **`Writable roots` joined them later (§2.18 ledger item 9), for the same
//! reason one layer down.** `SandboxSummary::isolated_worktree` mints a worktree
//! path — with a fresh UUID — for every isolated run, and `SecurityLayer` was
//! rendering it from inside the cacheable prefix. That is worse than the tier
//! flip above: the tier at least stays put unless someone touches a pill, while
//! the worktree id is guaranteed to differ on every isolated run, so no two of
//! them could share a prefix at all. A team fan-out of N sub-agents wrote the
//! same prefix N times. The posture (`Sandbox: git/worktree (isolated)`,
//! network, memory ceiling) is process-invariant and stays Stable; only the
//! *where* moves here. The two halves come from one source —
//! `SandboxSummary::{posture_lines, writable_roots_line}` — so they cannot drift
//! in wording, and `sandbox-debug` still prints the whole picture.
//!
//! R10/R9-safe: both strings are constants owned by the enum that defines the rule
//! they describe (`ExecTier::approval_prompt_line`, `SessionMode::prompt_line`), so
//! nothing is re-judged here and the copy cannot drift from the rule. Both `None`
//! (internal / sub-agent / token-estimate dispatch) emits nothing, leaving the
//! dynamic tail byte-identical.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct OperatingEnvelopeLayer;

impl PromptLayer for OperatingEnvelopeLayer {
    fn name(&self) -> &'static str {
        "operating_envelope"
    }

    fn priority(&self) -> u32 {
        1758
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        // The operating envelope is a hard runtime fact, not chrome: a model that
        // does not know it is in `Ask` will batch destructive calls and stall on
        // approvals. Drop only from the bare Minimal prompt, matching
        // `SecurityLayer`'s own gate.
        !matches!(mode, PromptMode::Minimal)
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let Some(ctx) = input.context else {
            return;
        };
        // The writable-root bullet is computed before the gate so the gate can
        // ask the same question the body answers — a section that renders
        // nothing must not print a header, and a fact that renders must not be
        // gated out by an unrelated knob being absent.
        let writable_roots = ctx
            .sandbox_summary
            .as_ref()
            .and_then(crate::sandbox::SandboxSummary::writable_roots_line);

        // Nothing resolved (internal / sub-agent / estimate dispatch): emit
        // nothing rather than a guessed default.
        if ctx.approval_tier.is_none() && ctx.session_mode.is_none() && writable_roots.is_none() {
            return;
        }

        output.push_str("## Operating Envelope\n\n");

        // Approval regime (codex `<approval_policy>` parity): the complement of
        // `SecurityLayer`'s sandbox posture — sandbox says what the agent may
        // touch, this says whether a mutating touch pauses for the human.
        if let Some(tier) = ctx.approval_tier {
            output.push_str(&format!("- {}\n", tier.approval_prompt_line()));
        }

        // Usage-mode register (chat / work / code): names the partition the tool
        // surface was built with, so the model knows which families are deferred
        // behind `tool_search` instead of discovering absences by failed calls.
        if let Some(mode) = ctx.session_mode {
            output.push_str(&format!("- {}\n", mode.prompt_line()));
        }

        // Where the agent may write. Tagged `(sandbox)` because its other half —
        // which enforcer, which tier — is stated far earlier by `SecurityLayer`
        // @600, and the two are only apart for cache reasons the model has no
        // business knowing about.
        if let Some(line) = writable_roots {
            output.push_str(&format!("- {line} (sandbox)\n"));
        }
        output.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::policies::{ExecTier, SessionMode};
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::security_context::SecurityContext;
    use crate::thinker::{InteractionManifest, InteractionParadigm};

    fn ctx() -> crate::thinker::context::ResolvedContext {
        ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
        )
    }

    fn render(c: &crate::thinker::context::ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(c));
        let mut out = String::new();
        OperatingEnvelopeLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn renders_both_knobs_when_resolved() {
        let mut c = ctx();
        c.approval_tier = Some(ExecTier::Ask);
        c.session_mode = Some(SessionMode::Code);
        let out = render(&c);
        assert!(out.contains("## Operating Envelope"), "{out}");
        assert!(out.contains("Approval mode: ask"), "{out}");
        assert!(out.contains("Usage mode: code"), "{out}");
    }

    #[test]
    fn either_knob_alone_triggers_the_section() {
        let mut c = ctx();
        c.approval_tier = Some(ExecTier::Full);
        let out = render(&c);
        assert!(out.contains("Approval mode: full"), "{out}");
        assert!(!out.contains("Usage mode:"), "{out}");

        let mut c = ctx();
        c.session_mode = Some(SessionMode::Chat);
        let out = render(&c);
        assert!(out.contains("Usage mode: chat"), "{out}");
        assert!(!out.contains("Approval mode:"), "{out}");
    }

    #[test]
    fn silent_without_either_knob() {
        let c = ctx();
        assert!(c.approval_tier.is_none() && c.session_mode.is_none());
        assert!(render(&c).is_empty());
        // No `ResolvedContext` at all (bare assembly path) is silent too.
        let config = PromptConfig::default();
        let mut out = String::new();
        OperatingEnvelopeLayer.inject(&mut out, &LayerInput::basic(&config, &[]));
        assert!(out.is_empty());
    }

    /// The per-run worktree root renders here, and on its own is enough to open
    /// the section — an isolated sub-agent run resolves neither knob (internal
    /// dispatch), so gating it behind them would have left the fact unstated on
    /// exactly the runs that have one.
    #[test]
    fn writable_roots_render_here_and_alone_open_the_section() {
        use crate::sandbox::SandboxSummary;

        let mut c = ctx();
        assert!(c.approval_tier.is_none() && c.session_mode.is_none());
        c.sandbox_summary = Some(SandboxSummary::isolated_worktree(std::path::PathBuf::from(
            "/wt/aleph-6f1c2e9a",
        )));

        let out = render(&c);
        assert!(out.contains("## Operating Envelope"), "{out}");
        assert!(out.contains("Writable roots: /wt/aleph-6f1c2e9a"), "{out}");
        // The posture half stays with `SecurityLayer`; restating it here would
        // put the same fact in two layers.
        assert!(!out.contains("git/worktree"), "{out}");
    }

    /// A read-only posture has no writable root, so it must not open the section
    /// with a bare header.
    #[test]
    fn read_only_sandbox_alone_stays_silent() {
        use crate::sandbox::{SandboxCapabilities, SandboxSummary};

        let mut c = ctx();
        c.sandbox_summary = Some(SandboxSummary::from_baseline(
            "macos/seatbelt",
            &SandboxCapabilities::strict(),
        ));
        assert!(render(&c).is_empty());
    }

    /// The whole reason this layer exists: the volatile knobs must NOT be in the
    /// cacheable prefix. Pins the zone so a future `stability()` edit cannot
    /// silently move them back.
    #[test]
    fn lives_in_the_per_request_dynamic_zone() {
        assert_eq!(OperatingEnvelopeLayer.stability(), LayerStability::Dynamic);
        assert!(
            OperatingEnvelopeLayer.priority() > 1700,
            "must sit in the Dynamic tail zone, after every Stable layer"
        );
    }
}
