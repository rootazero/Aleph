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
            // thinking, and prompt caching are well-tuned, so the heavy
            // TOOL_USE_ENFORCEMENT block stays off. But Claude *does*
            // share the early-give-up failure mode when one source
            // returns 401/403/timeout (Reuters-class incidents observed
            // on scheduled briefing jobs), so we still emit the
            // protocol-neutral persistence doctrine.
            "anthropic" => {
                output.push_str(TOOL_PERSISTENCE_DOCTRINE);
                output.push_str("\n\n");
            }

            // OpenAI / Codex / Grok (all reach us via the OpenAI wire
            // protocol). Same failure modes empirically: stops early,
            // hallucinates instead of calling tools, declares "done"
            // without verification.
            "openai" => {
                output.push_str(TOOL_USE_ENFORCEMENT);
                output.push_str("\n\n");
                output.push_str(TOOL_PERSISTENCE_DOCTRINE);
                output.push_str("\n\n");
                output.push_str(OPENAI_EXECUTION_DISCIPLINE_TAIL);
                output.push_str("\n\n");
            }

            // Google Gemini / Gemma — adds path-handling, dependency,
            // and parallel-tool-call discipline on top of the tool-use
            // enforcement + persistence baseline.
            "gemini" => {
                output.push_str(TOOL_USE_ENFORCEMENT);
                output.push_str("\n\n");
                output.push_str(TOOL_PERSISTENCE_DOCTRINE);
                output.push_str("\n\n");
                output.push_str(GOOGLE_OPERATIONAL_DIRECTIVES);
                output.push_str("\n\n");
            }

            // Local / open-weight (ollama and anything else that goes
            // through the OpenAI-compatible chat protocol but isn't
            // matched above) — emit tool-use enforcement plus the
            // persistence doctrine. Better to over-steer a small model
            // than leave it adrift.
            _ => {
                output.push_str(TOOL_USE_ENFORCEMENT);
                output.push_str("\n\n");
                output.push_str(TOOL_PERSISTENCE_DOCTRINE);
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

/// Protocol-neutral persistence doctrine — emitted to every provider
/// including Anthropic. Captures the "keep trying alternative tools and
/// sources" behaviour that every modern LLM benefits from. Refactored out
/// of the former `OPENAI_EXECUTION_DISCIPLINE` block when a real Anthropic
/// failure mode (Reuters 401 on scheduled briefing) showed that Claude
/// also needs the persistence steering.
const TOOL_PERSISTENCE_DOCTRINE: &str = "## Execution Discipline — Persistence\n\
\n\
**Tool persistence**\n\
- Use tools whenever they improve correctness, completeness, or grounding.\n\
- Do not stop early when another tool call would materially improve the result.\n\
- If a tool returns empty / partial / error results (401, 403, 404, timeout, \
rate-limit, empty body), retry with a DIFFERENT query, tool, or source before \
giving up. A single source returning an error is a routing signal, not a \
verdict on the goal.\n\
- Distinguish *method failure* (one URL/tool/keyword failed) from *goal failure* \
(every reasonable alternative has been tried). Only the latter justifies `fail`.\n\
- Keep calling tools until (1) the task is complete AND (2) you have verified the result.\n\
\n\
**Web/external-data fallback ladder** — when fetching online content:\n\
1. `search` with refined keywords (try synonyms, broader/narrower scope, different engines if configured).\n\
2. `web_fetch` on a canonical URL of the same resource.\n\
3. `web_fetch` on an alternate reputable source (BBC, AP, NYT, Wikipedia, mirrors, official feeds, etc.).\n\
4. Browser-based tools (chrome-devtools MCP, playwright, or the `autocli` skill) for sites that gate API or headless access.\n\
At least TWO rungs of this ladder must have been attempted before `fail`.\n\
\n\
**Mandatory tool use** — NEVER answer these from memory:\n\
- Arithmetic / hashes / encodings → run them via a tool.\n\
- Current time, date, timezone → query the system.\n\
- File contents / sizes / line counts → read the files.\n\
- System state (OS, CPU, processes, ports) → query the live system.\n\
- Current facts (weather, news, versions) → search.";

/// OpenAI / Codex / Grok — tail of the original execution-discipline block.
/// Holds the "act, don't ask" + "verify before finalizing" directives that
/// remain OpenAI-family specific (Claude's native tool_use already enforces
/// these by training). Emitted in addition to TOOL_PERSISTENCE_DOCTRINE for
/// OpenAI-protocol providers. Ported from `OPENAI_MODEL_EXECUTION_GUIDANCE`.
const OPENAI_EXECUTION_DISCIPLINE_TAIL: &str = "## Execution Discipline — OpenAI Family\n\
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
- **Conciseness**: keep explanatory text brief — a few sentences, not paragraphs. Still narrate \
each step in one short line (what you're doing and why); just keep it tight.\n\
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
    fn anthropic_emits_persistence_doctrine_only() {
        // Claude doesn't need TOOL_USE_ENFORCEMENT (native tool_use is
        // well-trained) but DOES need the persistence doctrine — a real
        // Reuters-401 failure on scheduled briefing showed Claude gives
        // up on a single source error without trying alternates.
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input =
            LayerInput::basic(&config, &tools).with_provider_protocol_opt(Some("anthropic"));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(
            out.contains("## Execution Discipline — Persistence"),
            "Anthropic must receive the persistence doctrine"
        );
        assert!(out.contains("Tool persistence"));
        assert!(out.contains("fallback ladder"));
        assert!(
            !out.contains("## Tool-Use Enforcement"),
            "Anthropic should NOT get the heavy tool-use enforcement block"
        );
        assert!(
            !out.contains("## Execution Discipline — OpenAI Family"),
            "Anthropic should NOT get OpenAI-family-specific tail"
        );
    }

    #[test]
    fn openai_emits_tool_use_persistence_and_execution_discipline() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_provider_protocol_opt(Some("openai"));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Tool-Use Enforcement"));
        assert!(out.contains("## Execution Discipline — Persistence"));
        assert!(out.contains("## Execution Discipline — OpenAI Family"));
        assert!(out.contains("Tool persistence"));
        assert!(out.contains("Act, don't ask"));
        assert!(!out.contains("Google Model"));
    }

    #[test]
    fn gemini_emits_tool_use_persistence_and_google_directives() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_provider_protocol_opt(Some("gemini"));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Tool-Use Enforcement"));
        assert!(out.contains("## Execution Discipline — Persistence"));
        assert!(out.contains("## Google Model Operational Directives"));
        assert!(out.contains("Absolute paths"));
        assert!(
            !out.contains("## Execution Discipline — OpenAI Family"),
            "Gemini should not get OpenAI-family-specific tail"
        );
    }

    #[test]
    fn unknown_protocol_falls_back_to_tool_use_plus_persistence() {
        let layer = ProviderGuidanceLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_provider_protocol_opt(Some("ollama"));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Tool-Use Enforcement"));
        assert!(out.contains("## Execution Discipline — Persistence"));
        assert!(!out.contains("## Execution Discipline — OpenAI Family"));
        assert!(!out.contains("## Google Model Operational Directives"));
    }
}
