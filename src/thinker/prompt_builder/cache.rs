//! Cached prompt building for Anthropic prompt caching optimization
//!
//! Leverages the [`LayerStability`] classification to partition the
//! system prompt into a stable prefix (cacheable) and a dynamic suffix
//! that changes per request.

use crate::tools::info::ToolInfo;

use super::{PromptBuilder, SystemPromptPart};
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput};
use crate::thinker::prompt_mode::PromptMode;

impl PromptBuilder {
    /// Build two-part system prompt for Anthropic cache optimization.
    ///
    /// Returns a vector of [`SystemPromptPart`]s where:
    /// - Part 1: Stable layers (cacheable) — persona, tools, security, skills, etc.
    /// - Part 2: Dynamic layers (not cacheable) — inbound context, runtime, memory, etc.
    ///
    /// The stable/dynamic boundary is determined by each layer's
    /// [`stability()`](crate::thinker::prompt_layer::PromptLayer::stability)
    /// declaration, so adding new layers automatically classifies them.
    ///
    /// Threads `mode` through the layer pipeline so token-constrained
    /// deployments can opt into a leaner system prompt — `Compact` / `Minimal`
    /// shed the heavy guidance layers that declare `supports_mode(mode) ==
    /// false`, while `Full` reproduces the legacy assembly byte-for-byte. The
    /// stable/dynamic split (and thus the prompt-cache breakpoint) is
    /// preserved across all modes. (A no-mode `build_system_prompt_cached`
    /// wrapper used to alias `Full`; every caller migrated to this entry and
    /// the wrapper was removed per YAGNI.)
    pub fn build_system_prompt_cached_with_mode(
        &self,
        tools: &[ToolInfo],
        mode: PromptMode,
    ) -> Vec<SystemPromptPart> {
        let input = self.build_cached_input(tools, mode);
        let stable = self
            .pipeline
            .execute_stable_with_mode(AssemblyPath::Cached, &input, mode);
        let mut dynamic =
            self.pipeline
                .execute_dynamic_with_mode(AssemblyPath::Cached, &input, mode);
        let resolved_strategy = self
            .resolved_context
            .as_ref()
            .and_then(|context| context.strategy.as_deref());
        if let (Some(body), true) = (self.strategy.as_deref(), resolved_strategy.is_none()) {
            dynamic.push_str("<strategy>\n");
            dynamic.push_str(body);
            dynamic.push_str("\n</strategy>\n\n");
        }

        // Enforce the system-prompt token budget. The stable prefix is a
        // protected floor (persona / tools / security) and is left untouched so
        // the Anthropic prefix cache — whose breakpoint sits at the
        // stable/dynamic boundary — stays valid; only the per-request dynamic
        // suffix is head/tail trimmed, with a model-visible truncation notice
        // appended. A no-op (byte-identical) for normal prompts under budget.
        let dynamic = crate::thinker::prompt_budget::fit_dynamic_suffix(
            stable.chars().count(),
            dynamic,
            &self.config.token_budget,
        );

        vec![
            SystemPromptPart {
                content: stable,
                cache: true,
            },
            SystemPromptPart {
                content: dynamic,
                cache: false,
            },
        ]
    }

    /// Construct the [`LayerInput`] shared by the cached stable/dynamic builds.
    ///
    /// Threads every builder field into the layer input — the same set
    /// `build_system_prompt` wires. This method is the harness bridge's
    /// production entry point: before this threading, the bridge's
    /// `with_identity_files` / `with_curated_envelope` / `with_agent` /
    /// `with_resolved_context` / `with_chain_context` /
    /// `with_behavior_name` / `with_iteration_cap` / `with_extra_files`
    /// calls were silently dropped here (the input was built bare), so the
    /// corresponding layers rendered nothing on the production cached path.
    fn build_cached_input<'a>(&'a self, tools: &'a [ToolInfo], mode: PromptMode) -> LayerInput<'a> {
        let input = LayerInput::basic(&self.config, tools)
            .with_mode(mode)
            .with_identity_files_opt(self.identity_files.as_ref())
            .with_extra_files_opt(self.extra_files.as_deref());
        let input = match &self.agent_def {
            Some(agent) => input.with_agent_def(agent),
            None => input,
        };
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        input
            .with_curated_envelope(self.curated_memory_envelope.clone())
            .with_chain_context_opt(self.chain_context.as_ref())
            .with_resolved_context_opt(self.resolved_context.as_ref())
            .with_behavior_name_opt(self.behavior_name.as_deref())
            .with_model_behavior_delta_opt(self.model_behavior_delta.as_deref())
            .with_iteration_cap_opt(self.iteration_cap)
            .with_session_summaries(self.has_session_summaries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
    use crate::thinker::prompt_mode::PromptMode;

    fn total_len(parts: &[SystemPromptPart]) -> usize {
        parts.iter().map(|p| p.content.len()).sum()
    }

    #[test]
    fn minimal_sheds_heavy_layers_on_cached_path() {
        let builder = PromptBuilder::new(PromptConfig::default());
        let full = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);
        let minimal = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Minimal);
        // `MemoryProtocolLayer` participates in the Cached path and declares
        // `supports_mode(Minimal) == false`, so Minimal must be strictly
        // leaner than Full even on the default config.
        assert!(
            total_len(&full) > total_len(&minimal),
            "expected Minimal prompt ({}) to be leaner than Full ({})",
            total_len(&minimal),
            total_len(&full),
        );
        // The stable/dynamic split (prompt-cache breakpoint) is preserved.
        assert_eq!(minimal.len(), 2);
        assert!(minimal[0].cache && !minimal[1].cache);
    }

    #[test]
    fn default_budget_leaves_production_prompt_untouched() {
        // The default 80K-char budget far exceeds an empty-config prompt, so
        // the production entry must pass through unchanged (no notice, cache
        // breakpoint intact) — the backward-compatible common path.
        let builder = PromptBuilder::new(PromptConfig::default());
        let parts = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);
        assert_eq!(parts.len(), 2);
        assert!(parts[0].cache && !parts[1].cache);
        assert!(!parts[1].content.contains("<system-reminder>"));
    }

    #[test]
    fn tiny_budget_protects_stable_and_warns() {
        // A pathologically small budget forces the dynamic suffix to be trimmed
        // while the stable prefix (the protected floor) is preserved verbatim.
        let baseline = PromptBuilder::new(PromptConfig::default())
            .build_system_prompt_cached_with_mode(&[], PromptMode::Full);
        let stable_floor = baseline[0].content.clone();

        let mut cfg = PromptConfig::default();
        cfg.token_budget.max_total_chars = 64; // below the stable floor
        let builder = PromptBuilder::new(cfg);
        let parts = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);

        // Stable prefix is never trimmed — cache stays valid.
        assert_eq!(parts[0].content, stable_floor);
        assert!(parts[0].cache && !parts[1].cache);
        // When the baseline had a non-empty dynamic suffix, it must now carry
        // the truncation notice; an empty baseline suffix stays empty.
        if !baseline[1].content.is_empty() {
            assert!(parts[1].content.contains("<system-reminder>"));
        }
    }

    #[test]
    fn cached_full_prompt_carries_role_and_citation_standards() {
        // Regression guard for the wiring fix: RoleLayer (100, Think->Act role
        // framing) and CitationStandardsLayer (900, memory-attribution rules)
        // are Stable, input-independent layers that MUST participate in the
        // live `AssemblyPath::Cached` production path. They previously omitted
        // `Cached` from `paths()` and so vanished from every main-loop prompt.
        // This locks them into the cacheable stable prefix.
        let builder = PromptBuilder::new(PromptConfig::default());
        let parts = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);
        // Both are Stable → they ride part 0 (the cacheable prefix).
        assert!(
            parts[0].content.contains("You are an AI assistant"),
            "cached Full prompt must carry the base role framing"
        );
        assert!(
            parts[0].content.contains("## Citation Standards"),
            "cached Full prompt must carry memory citation standards"
        );
    }

    #[test]
    fn cached_full_prompt_welds_strategy_into_stable_and_dynamic() {
        // Regression guard mirroring the role/citation vanish test: with a
        // Strategy present, the full `<strategy>` body MUST land in the stable
        // cacheable prefix (StrategyLayer @70, Stable) and the guardrail echo
        // in the dynamic suffix (StrategyPointerLayer @1756, Dynamic). Catches
        // both a missing `Cached` path (silent vanish) and a wrong `stability()`.
        use crate::thinker::context::{ContextAggregator, ResolvedContext};
        use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
        use crate::thinker::security_context::SecurityContext;

        let mut ctx: ResolvedContext = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.strategy = Some(
            "Objective: ship the strategic planner\nApproach: plan-first, adapt as you learn"
                .to_string(),
        );
        ctx.strategy_guardrails =
            Some("- don't refactor unrelated modules\n- don't add speculative config".to_string());

        let builder = PromptBuilder::new(PromptConfig::default()).with_resolved_context(ctx);
        let parts = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);

        // Stable prefix (part 0) carries the full `<strategy>` body.
        assert!(
            parts[0].content.contains("<strategy>"),
            "cached Full prompt must weld the <strategy> body into the stable prefix"
        );
        assert!(parts[0].content.contains("ship the strategic planner"));
        // Dynamic suffix (part 1) carries the guardrail echo.
        assert!(
            parts[1].content.contains("<strategy_reminder>"),
            "cached Full prompt must echo guardrails in the dynamic suffix"
        );
        assert!(parts[1]
            .content
            .contains("don't refactor unrelated modules"));
        // The `<strategy>` body must NOT leak into the dynamic suffix, and the
        // reminder must NOT leak into the stable prefix.
        assert!(!parts[1].content.contains("<strategy>"));
        assert!(!parts[0].content.contains("<strategy_reminder>"));
    }

    #[test]
    fn cached_full_prompt_injects_soul_and_agents_identity_files() {
        // Regression guard (feature 1.3, identity wiring): SOUL.md (persona,
        // SoulLayer @50) and AGENTS.md (project context, ProfileLayer @75) are
        // user-editable identity files that `IdentityFilesLayer` defers to
        // those two layers (`HANDLED_ELSEWHERE`). If SoulLayer / ProfileLayer
        // omit `AssemblyPath::Cached`, both files vanish from every main-loop
        // prompt — the live path is `build_system_prompt_cached_with_mode` —
        // even though `self_config` / `identity.*` write them correctly. This
        // locks both into the cacheable stable prefix (same class of fix the
        // Role / Citation layers already carry).
        use crate::thinker::identity_files::{IdentityFile, IdentityFiles};

        let files = IdentityFiles {
            identity_dir: std::path::PathBuf::from("/tmp/test-agent"),
            files: vec![
                IdentityFile {
                    name: "SOUL.md",
                    content: Some("You are Aleph, a calm and precise assistant.".to_string()),
                    truncated: false,
                    original_size: 44,
                },
                IdentityFile {
                    name: "AGENTS.md",
                    content: Some("This project uses Rust and tokio.".to_string()),
                    truncated: false,
                    original_size: 33,
                },
            ],
        };
        let builder = PromptBuilder::new(PromptConfig::default()).with_identity_files(files);
        let parts = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);

        // Both layers are Stable → they ride part 0 (the cacheable prefix).
        assert!(
            parts[0].content.contains("# Soul"),
            "cached Full prompt must carry the SOUL.md persona header"
        );
        assert!(
            parts[0].content.contains("calm and precise assistant"),
            "cached Full prompt must carry SOUL.md content"
        );
        assert!(
            parts[0].content.contains("## Project Context"),
            "cached Full prompt must carry the AGENTS.md project context header"
        );
        assert!(
            parts[0].content.contains("Rust and tokio"),
            "cached Full prompt must carry AGENTS.md content"
        );
    }

    #[test]
    fn cached_full_prompt_carries_subagent_role_and_protocol() {
        // Regression guard (wiring fix): AgentRoleLayer @55 injects a registered
        // sub-agent's role header + protocol constraints. The harness-bridge
        // runner resolves `agent_def` from the registry, threads it via
        // `with_agent`, then walks the Cached path — so a registered SubAgent
        // (verify / explore / coder / …) running as the loop agent (agent-switch
        // or team dispatch) needs this layer on `AssemblyPath::Cached` or its
        // role + protocol blocks silently vanish. AgentRoleLayer previously
        // omitted `Cached`; this locks it into the cacheable stable prefix
        // (same class of fix the Soul / Profile / Role / Citation layers carry).
        use crate::agents::{AgentDef, AgentMode};

        let def = AgentDef::new("verify", AgentMode::SubAgent)
            .with_prompt_sections(vec!["verify_protocol".to_string()]);
        let builder = PromptBuilder::new(PromptConfig::default()).with_agent(def);
        let parts = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);

        // AgentRoleLayer is Stable → it rides part 0 (the cacheable prefix).
        assert!(
            parts[0].content.contains("# Sub-Agent Role"),
            "cached Full prompt must carry the sub-agent role header"
        );
        assert!(
            parts[0].content.contains("Adversarial verifier"),
            "cached Full prompt must carry the sub-agent's verify protocol block"
        );
    }

    #[test]
    fn with_strategy_flows_to_cached_path_when_no_resolved_context_strategy() {
        let body = "Objective: ship the parser.\nGuardrails:\n- no network calls";
        let builder = PromptBuilder::new(PromptConfig::default()).with_strategy(body.to_string());
        let parts = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);

        let dynamic = &parts[1].content;
        assert!(
            dynamic.contains("<strategy>\n"),
            "welded strategy must land in the dynamic suffix: {dynamic:?}"
        );
        assert!(dynamic.contains("</strategy>\n"));
        assert!(dynamic.contains("ship the parser."));
        assert!(
            dynamic.ends_with("</strategy>\n\n"),
            "weld must mirror StrategyLayer wrap exactly"
        );
    }

    #[test]
    fn with_strategy_skipped_on_cached_path_when_resolved_context_strategy_present() {
        use crate::thinker::context::ContextAggregator;
        use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
        use crate::thinker::security_context::SecurityContext;

        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.strategy = Some("Objective: from resolved context".to_string());
        let body = "Objective: from with_strategy body — must be skipped";
        let builder = PromptBuilder::new(PromptConfig::default())
            .with_resolved_context(ctx)
            .with_strategy(body.to_string());
        let parts = builder.build_system_prompt_cached_with_mode(&[], PromptMode::Full);

        let combined = format!("{}{}", parts[0].content, parts[1].content);
        assert!(combined.contains("from resolved context"));
        assert!(
            !combined.contains("from with_strategy body"),
            "weld must not duplicate StrategyLayer's render"
        );
        assert_eq!(combined.matches("<strategy>").count(), 1);
    }
}
