//! GuidelinesLayer — general operational guidelines (priority 1300)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct GuidelinesLayer;

impl PromptLayer for GuidelinesLayer {
    fn name(&self) -> &'static str {
        "guidelines"
    }
    fn priority(&self) -> u32 {
        1300
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
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("## Guidelines\n");
        output.push_str("1. Take ONE action at a time, observe the result, then decide next\n");
        output.push_str("2. Use tool results to inform subsequent decisions\n");
        output.push_str(
            "3. Ask user when: multiple valid approaches, unclear requirements, need confirmation\n",
        );
        output.push_str(
            "4. Complete when: task is done, or you've provided the requested information\n",
        );
        output.push_str("5. Fail ONLY when every reasonable alternative tool/source has been tried — a single 401/403/timeout/empty-result from one source is a routing signal, not a verdict. Switch tools or sources before invoking `fail`.\n");
        output.push_str("6. Always close the loop — every turn ends with a reply to your caller; never finish on a tool call alone\n");
        output.push_str("7. 2-strike rule — if the SAME tool with substantially equivalent arguments fails twice, stop that exact variant and SWITCH (different tool, different source, different parameters, different keywords). Switching counts as a fresh attempt; the goal is not blocked until every reasonable alternative is exhausted.\n");
        output.push_str("8. Subagent error/timeout → report, never spawn replacement subagents on your own; summarize all sub-results (success + errors + timeouts) back to your caller\n");
        output.push_str("9. Suspected typo > silent search — if a caller-supplied path/identifier doesn't exist on first lookup, list nearby candidates and ask, don't fan out tool calls guessing\n");
        output.push_str("10. No-repeat rule — before issuing a tool call, scan THIS turn's tool_results for an identical (tool_name, arguments) pair; if you already have the answer in context, use it directly instead of re-calling. The harness will short-circuit duplicates anyway, so re-issuing wastes a turn\n");
        output.push_str("11. Aggregate before iterating — for \"count lines / files / bytes in directory\" use `file_ops` operation `stats` (one call returns per-file + total); do NOT loop over `file_read` to count\n");
        output.push_str("12. Goal vs method — the goal (e.g. \"get latest news on X\", \"summarize page Y\") is independent of any specific tool or source. When one source is blocked, refocus on the goal and pick another method that achieves it; do not mistake a method failure for a goal failure.\n");
        output.push_str("13. Web/external-data fallback ladder — when one route to online content fails, escalate in order: (a) refine the `search` query (different keywords, narrower/broader scope, different engine if multiple are configured); (b) `web_fetch` a known canonical URL of the same resource; (c) `web_fetch` an alternate reputable source (BBC, AP, NYT, Wikipedia, Reuters mirrors, government feeds, etc.); (d) browser-based tools (chrome-devtools MCP, playwright, or the `autocli` skill) for sites that gate API/headless access. At least TWO rungs of this ladder must have been attempted before `fail` is justified.\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_guidelines_content() {
        let layer = GuidelinesLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Guidelines"));
        assert!(out.contains("Take ONE action at a time"));
        // Rule 5: fail only after exhausting alternatives — the user-facing
        // perseverance doctrine. A single 401/403/timeout is a routing
        // signal, not a verdict.
        assert!(out.contains("Fail ONLY when every reasonable alternative"));
        // Fail-fast bullets (Phase-3): close-the-loop, 2-strike, subagent
        // error escalation, typo-first. These extend the layer to make
        // every agent's loop terminate with a user-visible reply instead
        // of silently looping on bad input.
        assert!(out.contains("Always close the loop"));
        assert!(out.contains("2-strike rule"));
        // Rule 7 must teach switching, not just stopping.
        assert!(out.contains("SWITCH"));
        assert!(out.contains("Subagent error/timeout"));
        assert!(out.contains("Suspected typo"));
        assert!(out.contains("No-repeat rule"));
        assert!(out.contains("Aggregate before iterating"));
        // Rules 12–13: goal/method separation + web-research fallback ladder.
        assert!(out.contains("Goal vs method"));
        assert!(out.contains("fallback ladder"));
        assert!(out.contains("chrome-devtools"));
    }

    #[test]
    fn test_guidelines_priority_and_paths() {
        assert_eq!(GuidelinesLayer.priority(), 1300);
        let paths = GuidelinesLayer.paths();
        assert_eq!(paths.len(), 5);
        assert!(paths.contains(&AssemblyPath::Cached));
    }
}
