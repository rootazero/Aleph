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
        // Sub-agent binding state for `## Operating Envelope` — three facts the
        // layer may render independently: the subagent run id, the network
        // posture (open / restricted / none), and the policy profile id.
        // Each is `None` by default; primary user-facing sessions see none of
        // them and the prompt is byte-identical.
        let run_id_line = ctx
            .run_id
            .as_deref()
            .map(|id| format!("- Run id: `{id}`\n"));
        let network_line = ctx
            .sandbox_summary
            .as_ref()
            .map(crate::sandbox::SandboxSummary::network_prompt_line)
            .unwrap_or_default();
        let policy_profile_line = ctx
            .sandbox_summary
            .as_ref()
            .and_then(crate::sandbox::SandboxSummary::permission_profile_prompt_line)
            .map(|line| format!("- {line}\n"));

        // The read-only planning floor, when it is on. `Building` — which is
        // what an ordinary session resolves to — renders `None`, so this line
        // costs zero bytes for everyone who never asked to plan, and the gate
        // below still asks the same question the body answers.
        let plan_line = ctx
            .plan_phase
            .and_then(crate::config::types::policies::PlanPhase::prompt_line);

        // Nothing resolved (internal / sub-agent / estimate dispatch): emit
        // nothing rather than a guessed default.
        if ctx.approval_tier.is_none()
            && ctx.session_mode.is_none()
            && plan_line.is_none()
            && writable_roots.is_none()
            && run_id_line.is_none()
            && network_line.is_none()
            && policy_profile_line.is_none()
        {
            return;
        }

        output.push_str("## Operating Envelope\n\n");

        // The planning floor goes FIRST, ahead of the approval line, because it
        // is the only one of the two that can make the other's promise
        // inapplicable: "auto — routine calls run without interruption" is
        // misleading read before "nothing that changes anything runs at all".
        // Order in a bullet list is the only tool this layer has for saying
        // which rule wins, and it costs nothing to use it correctly.
        if let Some(line) = plan_line {
            output.push_str(&format!("- {line}\n"));
        }

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
        // Network posture — surfaced here (Dynamic) not in `SecurityLayer`
        // @600 (Stable) because the same cache-reasoning applies: a network
        // rule change mid-conversation would otherwise re-key the prefix.
        // The line is *added* when the layer above reports a posture; an
        // Open / Restricted / Air-gapped dispatcher surfaces the same
        // descriptor the gate enforces, so the prompt can never claim a
        // regime the runtime does not apply.
        if let Some(line) = network_line {
            output.push_str(&format!("- {line}\n"));
        }
        // Permission profile id: a stable, audit-friendly reference so the
        // model (and the log) can tag a tool call against the exact policy
        // it ran under. Empty until the dispatcher hands in a profile id
        // (legacy / mock sandboxes don't), so the byte stream stays
        // byte-identical for the common case.
        if let Some(line) = policy_profile_line {
            output.push_str(&line);
        }
        // Sub-agent run id: lets the model refer to "this run of the
        // explore sub-agent" when talking to the parent session in a long
        // delegation. Kept in this layer (Dynamic, near `run_loop`) so
        // it sits close to the other per-run machinery, far from the
        // Stable / SandboxLayer back-half.
        if let Some(line) = run_id_line {
            output.push_str(&line);
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
    fn a_building_session_spends_no_bytes_on_the_plan_phase() {
        // The overwhelming majority of turns. `Some(Building)` must render
        // byte-identically to `None`, or every install that never heard of this
        // feature pays for it on every request.
        let mut absent = ctx();
        absent.approval_tier = Some(ExecTier::Auto);
        absent.session_mode = Some(SessionMode::Work);
        let baseline = render(&absent);

        let mut building = absent.clone();
        building.plan_phase = Some(crate::config::types::policies::PlanPhase::Building);
        assert_eq!(render(&building), baseline);
    }

    #[test]
    fn planning_renders_ahead_of_the_approval_line() {
        use crate::config::types::policies::PlanPhase;

        let mut c = ctx();
        c.approval_tier = Some(ExecTier::Auto);
        c.session_mode = Some(SessionMode::Work);
        c.plan_phase = Some(PlanPhase::Planning);
        let out = render(&c);

        let plan_at = out
            .find(PlanPhase::Planning.prompt_line().expect("planning speaks"))
            .expect("the planning line must render");
        let tier_at = out
            .find(ExecTier::Auto.approval_prompt_line())
            .expect("the approval line must still render");
        // Order is the only tool a bullet list has for saying which rule wins,
        // and "auto — routine calls run without interruption" read BEFORE
        // "nothing that changes anything runs at all" is actively misleading.
        assert!(
            plan_at < tier_at,
            "the planning floor must precede the approval regime:\n{out}"
        );
    }

    #[test]
    fn the_planning_line_is_the_one_the_floor_owns() {
        use crate::config::types::policies::PlanPhase;

        // Single-source pin, same shape as the sandbox test below: the layer
        // must print the enum's own copy, not a paraphrase of it, so the rule
        // and its description cannot drift.
        let mut c = ctx();
        c.plan_phase = Some(PlanPhase::Planning);
        let out = render(&c);
        assert!(out.contains(PlanPhase::Planning.prompt_line().unwrap()));
    }

    // SandboxCapabilities::strict is used to construct a posture with
    // "Network: denied" — verify the bullet appears using the exact
    // wording from `SandboxSummary::network_prompt_line`. This is a
    // single-source test: change the wording and the assertion updates
    // too.
    #[test]
    fn sandbox_network_line_uses_wording_from_summary_module() {
        use crate::sandbox::{SandboxCapabilities, SandboxSummary};

        // Render the line directly and make sure it matches what the layer
        // prints — pin so any drift between the two surfaces is caught.
        let summary =
            SandboxSummary::from_baseline("macos/seatbelt", &SandboxCapabilities::strict());
        let expected = summary.network_prompt_line().expect("strict = denied");
        let mut c = ctx();
        c.approval_tier = Some(ExecTier::Auto);
        c.sandbox_summary = Some(summary);
        let out = render(&c);
        assert!(
            out.contains(&expected),
            "layer output: {out}\nexpected: {expected}"
        );
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

    /// A read-only posture alone — no exec tier, no session mode, no
    /// writable root, no profile id, no run id — **does** open the
    /// `## Operating Envelope` section now: the `Network:` line is part
    /// of the per-run envelope (same codex `<environment_context>`
    /// shape as `writable_roots`). Update the old "stays silent" pin
    /// to test the network-only render: `writable_roots` must stay
    /// absent in the read-only posture (the Stable-layer back-half still
    /// carries "Sandbox: … (read-only)" separately).
    #[test]
    fn read_only_posture_prints_network_line_but_no_writable_root() {
        use crate::sandbox::{SandboxCapabilities, SandboxSummary};

        let mut c = ctx();
        c.sandbox_summary = Some(SandboxSummary::from_baseline(
            "macos/seatbelt",
            &SandboxCapabilities::strict(),
        ));
        let out = render(&c);
        // Network posture now rides in Dynamic (paired with codex's
        // `<environment_context>` separation): the section opens so
        // the model sees the runtime posture it must respect.
        assert!(out.contains("## Operating Envelope"), "{out}");
        assert!(out.contains("Network: denied"), "{out}");
        // `Writable roots:` is the half this layer re-renders from
        // Dynamic; read-only posture has none and the bullet must
        // stay absent (its complement is the Stable `Sandbox:` line
        // printed by SecurityLayer, not this layer).
        assert!(!out.contains("Writable roots"), "{out}");
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

    #[test]
    fn renders_run_id_line_when_subagent_provides_one() {
        use crate::thinker::context::EnvelopeParent;

        let mut c = ctx();
        c.approval_tier = Some(ExecTier::Auto);
        c.run_id = Some("subagent-7c1f".to_string());
        c.envelope_parent = Some(EnvelopeParent {
            kind: "subagent".to_string(),
            id: "session-X".to_string(),
        });
        let out = render(&c);

        // Run id is its own bullet (backticked so downstream prompt tools
        // can match on it), distinct from the parent element rendered by
        // `RuntimeContextLayer` — different layer, different surface, both
        // intentional.
        assert!(out.contains("Run id: `subagent-7c1f`"), "{out}");
    }

    #[test]
    fn renders_network_line_when_sandbox_advertises_posture() {
        use crate::sandbox::{SandboxCapabilities, SandboxSummary};

        let mut c = ctx();
        c.approval_tier = Some(ExecTier::Auto);
        c.sandbox_summary = Some(SandboxSummary::from_baseline(
            "macos/seatbelt",
            &SandboxCapabilities::strict(),
        ));
        let out = render(&c);
        // No exact-string compare (the wording lives in
        // `SandboxSummary::network_prompt_line`, the test below) — just
        // verify the bullet appears, since a `strict()` profile yields
        // `Network: denied` deterministically.
        assert!(out.contains("Network: "), "{out}");
    }

    #[test]
    fn renders_permission_profile_line_only_when_profile_id_resolves() {
        use crate::sandbox::{SandboxCapabilities, SandboxSummary};

        let mut c = ctx();
        c.approval_tier = Some(ExecTier::Auto);
        c.sandbox_summary = Some(
            SandboxSummary::from_baseline("test/backend", &SandboxCapabilities::strict())
                .with_permission_profile_id("policy-strict-v3"),
        );
        let out = render(&c);
        assert!(
            out.contains("Permission profile: policy-strict-v3"),
            "{out}"
        );

        // Without a profile id the line stays absent (legacy / mock sandboxes).
        c.sandbox_summary = Some(SandboxSummary::from_baseline(
            "test/backend",
            &SandboxCapabilities::strict(),
        ));
        let out = render(&c);
        assert!(!out.contains("Permission profile"), "{out}");
    }

    #[test]
    fn section_stays_silent_when_envelope_is_completely_empty() {
        // Regression for `Envelope -> is_empty` fast path: a layer that
        // *only* contributes sandbox posture (no tier, no mode, no profile
        // id, no run_id) must not double-render a header for the parent
        // binding alone. The runtime_CONTEXT layer carries the parent
        // element; this layer only echoes the run id.
        let mut c = ctx();
        c.envelope_parent = Some(crate::thinker::context::EnvelopeParent {
            kind: "subagent".to_string(),
            id: "session-X".to_string(),
        });
        // Bypass: parent alone never opens `## Operating Envelope`. Only
        // the runtime_context layer renders `<parent>`. Pin so neither
        // leaks.
        assert!(render(&c).is_empty());
    }
}
