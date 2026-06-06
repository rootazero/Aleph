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
        output.push_str("13. Web/external-data fallback ladder — when one route to online content fails, escalate in order: (a) refine the `search` query (different keywords, narrower/broader scope, different engine if multiple are configured); (b) `web_fetch` a known canonical URL of the same resource; (c) `web_fetch` an alternate reputable source (BBC, AP, NYT, Wikipedia, Reuters mirrors, government feeds, etc.); (d) browser-based tools (chrome-devtools MCP, playwright, or the `autocli` skill) for sites that gate API/headless access. At least TWO rungs of this ladder must have been attempted before `fail` is justified.\n");
        output.push_str("14. Delegate heavy research to protect your own context — when a task needs many searches and page-fetches across multiple sources (multi-source research, surveys, \"gather/analyze everything about X\", long report-writing), running them all in your own loop piles every raw page into your context and starves the final synthesis (the longer your context, the worse your writing and the higher the risk of a turn timeout). Fan out instead: call the `subagent` tool with `batch_tasks` to run independent research threads in parallel; each sub-agent does its own searching/fetching and returns only a distilled digest, so your main context holds digests — not raw pages — when you write the answer.\n");
        output.push_str("15. Converge — every loop must move toward a conclusion, not just accumulate. Gathering (searching, fetching, reading files, running diagnostics, grepping) is a means to an answer, never the goal itself. Watch for diminishing returns: once new iterations stop yielding materially new information, STOP gathering and commit to the deliverable — write the report, give the answer, state the bug's root cause, propose the fix. Announcing intent — \"now I'll write it up\", \"let me check one more file\" — is NOT progress: the moment you have what the task needs, your VERY NEXT action must be the delivering action (call the write/generate tool, or give the final reply), not another `search`/`web_fetch`/`file_read`. This applies to every open-ended task: a report once the core facts are in → produce it now and enrich later; a bug hunt once the cause is reproduced and located → report it, don't keep grepping; research once you can answer → synthesize. Shipping with what you have beats deferring for a nice-to-have. Reaching the iteration limit while still gathering is a FAILURE — you gathered too long and delivered too late.\n");
        output.push_str("16. Never fabricate — every result you cite must be one you actually obtained. Do not invent file contents, command output, search hits, URLs, figures, or quotes; do not claim you performed an action (\"I ran the tests\", \"I saved the file\", \"I generated the chart\") unless the tool call actually executed and returned that result. If a tool failed, was unavailable, or returned nothing, SAY SO plainly and state what you will try instead — a stated failure or limitation is always more useful than a plausible-looking invention. Confabulating an excuse (e.g. \"the tool was disabled\") instead of reporting the real situation is itself a failure; the user needs the truth to decide the next move.\n");
        output.push_str("17. Narrate only when it earns its place — for routine, low-risk calls (a read, a search, a status check) just make them; do not prefix every call with \"now I'll…\". Reserve a one-line narration for complex, destructive, or explicitly-requested actions where the user benefits from knowing your intent before you act. Repeated \"now I'll create the X\" narration while X is never actually produced is noise that masquerades as progress (see rule 15) — let the delivered result, not the announcement, speak.\n\n");
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
        // Rule 14: delegate heavy multi-source research to parallel sub-agents
        // that return digests, so the main context stays lean for synthesis.
        assert!(out.contains("Delegate heavy research"));
        assert!(out.contains("batch_tasks"));
        // Rule 15: converge — every open-ended task (report, bug hunt,
        // research) must move toward a conclusion once returns diminish, not
        // gather forever. Counters the "announce-then-gather-again" loop that
        // burns the iteration budget without ever delivering.
        assert!(out.contains("15. Converge"));
        assert!(out.contains("VERY NEXT action must be the delivering action"));
        // Rule 16: evidence integrity — never invent tool output or claim an
        // action you didn't perform; report real failures/limitations instead
        // of confabulating an excuse (the "tool was disabled" failure mode).
        assert!(out.contains("16. Never fabricate"));
        assert!(out.contains("Confabulating an excuse"));
        // Rule 17: narration discipline — don't narrate routine calls; reserve
        // narration for complex/destructive/requested actions. Counters the
        // over-narration that lets a gather-loop dodge the loop verifier.
        assert!(out.contains("17. Narrate only when it earns its place"));
    }

    #[test]
    fn test_guidelines_priority_and_paths() {
        assert_eq!(GuidelinesLayer.priority(), 1300);
        let paths = GuidelinesLayer.paths();
        assert_eq!(paths.len(), 5);
        assert!(paths.contains(&AssemblyPath::Cached));
    }
}
