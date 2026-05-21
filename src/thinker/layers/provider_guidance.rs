//! ProviderGuidanceLayer — family-specific operational directives (priority 810).
//!
//! Ports Hermes-agent's TOOL_USE_ENFORCEMENT / OPENAI_EXECUTION /
//! GOOGLE_MODEL_OPERATIONAL guidance blocks into a single
//! protocol-dispatched layer. The wire protocol reported by
//! `AiProvider::protocol()` ("anthropic" / "openai" / "gemini" /
//! "ollama" / custom) selects the appropriate block. Anthropic is
//! left silent — Claude's native tool_use is well-behaved enough
//! that the extra steering does more harm than good.
//!
//! Stability: Stable (protocol is fixed per run; cacheable in the
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
        let protocol = match input.provider_protocol {
            Some(p) => p,
            None => return,
        };
        match protocol {
            // Claude / Anthropic protocol — native tool_use, extended
            // thinking, and prompt caching are already well-tuned. No
            // extra steering needed; emit nothing to keep the prompt
            // prefix lean.
            "anthropic" => {}

            // OpenAI / Codex / Grok (all reach us via the OpenAI wire
            // protocol). Same failure modes empirically: stops early,
            // hallucinates instead of calling tools, declares "done"
            // without verification.
            "openai" => {
                output.push_str(TOOL_USE_ENFORCEMENT);
                output.push_str("\n\n");
                output.push_str(OPENAI_EXECUTION_DISCIPLINE);
                output.push_str("\n\n");
            }

            // Google Gemini / Gemma — adds path-handling, dependency,
            // and parallel-tool-call discipline on top of the tool-use
            // enforcement baseline.
            "gemini" => {
                output.push_str(TOOL_USE_ENFORCEMENT);
                output.push_str("\n\n");
                output.push_str(GOOGLE_OPERATIONAL_DIRECTIVES);
                output.push_str("\n\n");
            }

            // Local / open-weight (ollama and anything else that goes
            // through the OpenAI-compatible chat protocol but isn't
            // matched above) — emit only the baseline tool-use
            // enforcement. Better to over-steer a small model than
            // leave it adrift.
            _ => {
                output.push_str(TOOL_USE_ENFORCEMENT);
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
You MUST use your tools to take action — do not describe what you would do or plan to do \
without actually doing it. When you say you will perform an action (\"I will run the tests\", \
\"Let me check the file\"), you MUST immediately make the corresponding tool call in the same \
response. Never end your turn with a promise of future action — execute it now.\n\
\n\
Keep working until the task is actually complete. Every response should either (a) contain tool \
calls that make progress, or (b) deliver a final result. Responses that only describe intentions \
without acting are not acceptable.";

/// OpenAI / Codex / Grok — addresses early-stopping, hallucination in
/// place of tool calls, and "done" claims without verification. Ported
/// from `OPENAI_MODEL_EXECUTION_GUIDANCE`.
const OPENAI_EXECUTION_DISCIPLINE: &str = "## Execution Discipline\n\
\n\
**Tool persistence**\n\
- Use tools whenever they improve correctness, completeness, or grounding.\n\
- Do not stop early when another tool call would materially improve the result.\n\
- If a tool returns empty or partial results, retry with a different query or strategy before giving up.\n\
- Keep calling tools until (1) the task is complete AND (2) you have verified the result.\n\
\n\
**Mandatory tool use** — NEVER answer these from memory:\n\
- Arithmetic / hashes / encodings → run them via a tool.\n\
- Current time, date, timezone → query the system.\n\
- File contents / sizes / line counts → read the files.\n\
- System state (OS, CPU, processes, ports) → query the live system.\n\
- Current facts (weather, news, versions) → search.\n\
\n\
**Act, don't ask** — when a question has an obvious default interpretation, act on it. Only \
ask for clarification when the ambiguity genuinely changes what tool you would call.\n\
\n\
**Verify before finalizing**: correctness, grounding (factual claims backed by tool outputs), \
formatting, safety (confirm scope before side-effecting actions).";

/// Gemini / Gemma — operational rules that empirically reduce common
/// failure modes (relative paths, missing dep checks, verbose narration,
/// sequential tool calls when parallel would do). Ported from
/// `GOOGLE_MODEL_OPERATIONAL_GUIDANCE`.
const GOOGLE_OPERATIONAL_DIRECTIVES: &str = "## Google Model Operational Directives\n\
\n\
- **Absolute paths**: always construct and use absolute file paths for all file-system operations.\n\
- **Verify first**: read the file or search the project before making changes — never guess at \
contents.\n\
- **Dependency checks**: never assume a library is available; check package.json / requirements.txt \
/ Cargo.toml first.\n\
- **Conciseness**: keep explanatory text brief — a few sentences, not paragraphs. Actions and \
results beat narration.\n\
- **Parallel tool calls**: when independent operations are needed (reading several files, for \
example), make all the calls in a single response rather than sequentially.\n\
- **Non-interactive commands**: pass flags like `-y`, `--yes`, `--non-interactive` to prevent CLI \
tools from hanging on prompts.\n\
- **Keep going**: work autonomously until the task is fully resolved — don't stop with a plan, \
execute it.";

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
    fn silent_when_protocol_missing() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn silent_for_anthropic() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input =
            LayerInput::basic(&config, &tools).with_provider_protocol_opt(Some("anthropic"));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(
            out.is_empty(),
            "Anthropic protocol must not emit guidance (Claude is well-behaved)"
        );
    }

    #[test]
    fn openai_emits_tool_use_and_execution_discipline() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_provider_protocol_opt(Some("openai"));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Tool-Use Enforcement"));
        assert!(out.contains("## Execution Discipline"));
        assert!(out.contains("Tool persistence"));
        assert!(!out.contains("Google Model"));
    }

    #[test]
    fn gemini_emits_tool_use_and_google_directives() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_provider_protocol_opt(Some("gemini"));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Tool-Use Enforcement"));
        assert!(out.contains("## Google Model Operational Directives"));
        assert!(out.contains("Absolute paths"));
        assert!(!out.contains("Execution Discipline"));
    }

    #[test]
    fn unknown_protocol_falls_back_to_tool_use_baseline() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_provider_protocol_opt(Some("ollama"));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Tool-Use Enforcement"));
        assert!(!out.contains("## Execution Discipline"));
        assert!(!out.contains("## Google Model Operational Directives"));
    }
}
