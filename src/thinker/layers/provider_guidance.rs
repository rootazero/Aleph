//! `ProviderGuidanceLayer` — family-specific operational directives (priority 810).
//!
//! Dispatches on the resolved governance **behavior name**
//! (`resolve_behavior`: anthropic / openai / gemini / ollama / strict /
//! unknown) rather than the raw wire protocol, so a weak model on the
//! anthropic protocol is steered as the weak model it is. Every
//! non-anthropic family receives the shared `TOOL_USE_ENFORCEMENT`
//! baseline; every family (Anthropic included) receives the
//! `TOOL_PERSISTENCE_DOCTRINE`. The per-family coaching delta — and the
//! L2 elicitation that extracts more capability — is the overridable
//! `.md` content threaded in via `LayerInput::model_behavior_delta`
//! (`model_behaviors/{behavior}.md`, user-overridable at
//! `~/.aleph/model_behaviors/`), appended verbatim after the baseline.
//!
//! Stability: Stable (behavior is fixed per run; cacheable in the
//! prompt prefix).
//! Mode: Full only.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct ProviderGuidanceLayer;

impl PromptLayer for ProviderGuidanceLayer {
    fn name(&self) -> &'static str {
        "provider_guidance"
    }

    fn priority(&self) -> u32 {
        // Sits immediately after OperationalGuidelinesLayer (800) so
        // family-specific directives layer on top of the paradigm
        // baseline.
        810
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Stable
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let behavior = match input.behavior_name {
            Some(b) => b,
            None => return,
        };
        // Shared baseline, selected by behavior family. Anthropic skips the
        // heavy tool-use enforcement (native tool_use is well-trained); every
        // family — Anthropic included — gets the persistence doctrine.
        if behavior != "anthropic" {
            output.push_str(TOOL_USE_ENFORCEMENT);
            output.push_str("\n\n");
        }
        output.push_str(TOOL_PERSISTENCE_DOCTRINE);
        output.push_str("\n\n");
        // Per-family delta (OpenAI tail / Google directives / ollama rails /
        // strict control / L2 elicitation) — data-driven, user-overridable at
        // `~/.aleph/model_behaviors/{behavior}.md`.
        if let Some(delta) = input.model_behavior_delta {
            let delta = delta.trim();
            if !delta.is_empty() {
                output.push_str(delta);
                output.push_str("\n\n");
            }
        }
    }
}

/// Universal tool-use enforcement, applied to every protocol except
/// Anthropic. Ported from Hermes' `TOOL_USE_ENFORCEMENT_GUIDANCE`
/// (see hermes-agent/agent/prompt_builder.py:254).
const TOOL_USE_ENFORCEMENT: &str = "## Tool-Use Enforcement\n\
\n\
Use tools to act — never just describe or promise action. When you say you will do something \
(\"I'll run the tests\", \"Let me check the file\"), make the tool call in the SAME response; \
never end a turn on a promise. Keep going until the task is complete: every response either makes \
progress via tool calls or delivers a final result.";

/// Protocol-neutral persistence doctrine — emitted to every provider
/// including Anthropic. Captures the "keep trying alternative tools and
/// sources" behaviour that every modern LLM benefits from. Refactored out
/// of the former `OPENAI_EXECUTION_DISCIPLINE` block when a real Anthropic
/// failure mode (Reuters 401 on scheduled briefing) showed that Claude
/// also needs the persistence steering.
const TOOL_PERSISTENCE_DOCTRINE: &str = "## Execution Discipline — Persistence\n\
\n\
**Tool persistence**\n\
- Use tools whenever they improve correctness, completeness, or grounding; don't stop \
early while another call would materially improve the result.\n\
- On empty / partial / error results (401, 403, 404, timeout, rate-limit), retry with a \
DIFFERENT query, tool, or source. One source erroring is a routing signal, not a verdict \
on the goal.\n\
- Distinguish *method failure* (one URL/tool/keyword failed) from *goal failure* (every \
reasonable alternative tried). Only the latter justifies giving up.\n\
- Keep going until the task is complete AND the result is verified.\n\
\n\
**Web/external-data fallback ladder** — when fetching online content:\n\
1. `search` with refined keywords (synonyms, broader/narrower scope, other engines).\n\
2. `web_fetch` a canonical URL of the same resource.\n\
3. `web_fetch` an alternate reputable source (BBC, AP, Wikipedia, mirrors, official feeds).\n\
4. Browser tools (chrome-devtools MCP, playwright, `autocli` skill) for API-/headless-gated sites.\n\
Climb at least TWO rungs before conceding.\n\
\n\
**Ceiling** — ~3 distinct routes failing the same way (403/404/anti-bot/JS-only) means \
the datum is gated; more same-class URLs aren't progress. Escalate tool CLASS once \
(browser/`autocli`/skill on the user's logged-in session); if still blocked, deliver what \
you DID obtain and state the gap. A blocked source is method failure — never grounds to \
abandon the goal or loop forever switching sources.\n\
\n\
**Mandatory tool use** — NEVER answer from memory: arithmetic / hashes / encodings → run \
a tool; time / date / timezone → query the system; file contents / sizes / line counts → \
read the files; system state (OS, CPU, processes, ports) → query live; current facts \
(weather, news, versions) → search.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn metadata_and_priority() {
        let layer = ProviderGuidanceLayer;
        assert_eq!(layer.name(), "provider_guidance");
        assert_eq!(layer.priority(), 810);
        assert!(matches!(layer.stability(), LayerStability::Stable));
    }

    #[test]
    fn supports_full_only() {
        let layer = ProviderGuidanceLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(!layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn participates_in_every_non_minimal_path() {
        let paths = ProviderGuidanceLayer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Hydration));
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(paths.contains(&AssemblyPath::Cached));
    }

    #[test]
    fn silent_when_behavior_missing() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn anthropic_baseline_is_persistence_only() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_behavior_name_opt(Some("anthropic"));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Execution Discipline — Persistence"));
        assert!(!out.contains("## Tool-Use Enforcement"));
    }

    #[test]
    fn non_anthropic_baseline_has_tool_use_and_persistence() {
        for behavior in ["openai", "gemini", "ollama", "strict", "unknown"] {
            let layer = ProviderGuidanceLayer;
            let config = PromptConfig::default();
            let tools = vec![];
            let input = LayerInput::basic(&config, &tools).with_behavior_name_opt(Some(behavior));
            let mut out = String::new();
            layer.inject(&mut out, &input);
            assert!(out.contains("## Tool-Use Enforcement"), "{behavior}");
            assert!(
                out.contains("## Execution Discipline — Persistence"),
                "{behavior}"
            );
        }
    }

    #[test]
    fn delta_is_appended_after_baseline() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let delta = "## Execution Discipline — OpenAI Family\nAct, don't ask";
        let input = LayerInput::basic(&config, &tools)
            .with_behavior_name_opt(Some("openai"))
            .with_model_behavior_delta_opt(Some(delta));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        let baseline_at = out.find("## Tool-Use Enforcement").unwrap();
        let delta_at = out.find("Act, don't ask").unwrap();
        assert!(delta_at > baseline_at, "delta must follow the baseline");
    }
}
