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
        // Loop-mechanics doctrine only. Persistence, the external-data fallback
        // ladder, and the method-vs-goal distinction live once in
        // `ProviderGuidanceLayer`'s TOOL_PERSISTENCE_DOCTRINE (priority 810,
        // emitted to every provider including Anthropic, always co-fires with
        // this layer in Full mode) — they used to be duplicated here as rules
        // 5/12/13/13b. `SpecialActionsLayer` owns the `fail` preconditions.
        output.push_str("## Guidelines\n");
        output.push_str("1. One action per turn — observe its result before choosing the next.\n");
        output.push_str(
            "2. Let tool results drive your next decision; never act against evidence you just saw.\n",
        );
        output.push_str("3. Ask the user only when ambiguity changes what you'd do: rival approaches, unclear requirements, or a needed confirmation.\n");
        output.push_str(
            "4. Complete when the task is done or the requested information is delivered.\n",
        );
        output.push_str("5. Close the loop — every turn ends with a reply to your caller; never finish on a bare tool call.\n");
        output.push_str("6. 2-strike rule — if the SAME tool with equivalent arguments fails twice, stop that variant and SWITCH (different tool, source, parameters, or keywords). Switching counts as a fresh attempt.\n");
        output.push_str("7. Subagent error/timeout → report it; never spawn a replacement subagent yourself. Summarize all sub-results (successes, errors, timeouts) to your caller.\n");
        output.push_str("8. Suspected typo > silent search — if a caller-supplied path/identifier doesn't resolve, list near matches and ask; don't fan out guessing.\n");
        output.push_str("9. No-repeat — before a tool call, scan THIS turn's results for an identical (tool, arguments) pair and reuse it. The harness short-circuits duplicates, so re-issuing wastes a turn.\n");
        output.push_str("10. Aggregate before iterating — to count lines/files/bytes in a directory use `file_ops` `stats` (one call returns per-file + total); don't loop `file_read`.\n");
        output.push_str("11. Delegate heavy research — multi-source research, surveys, and long reports flood your context with raw pages and starve synthesis. Fan out via `subagent` `batch_tasks`: each thread returns a digest, keeping your context lean for the answer.\n");
        output.push_str("12. Converge — gathering is a means, never the goal. Once iterations stop yielding materially new information, STOP and deliver; the moment you have what the task needs, your VERY NEXT action is the delivering one (write/generate or final reply), not another search/fetch/read. Announcing intent isn't progress; reaching the iteration limit while still gathering is a failure.\n");
        output.push_str("13. Never fabricate — cite only results you actually obtained; never invent file contents, output, hits, URLs, figures, or quotes, and never claim an action (\"I ran the tests\", \"I saved the file\") the tool didn't return. If a tool failed or returned nothing, say so and state your next move. Confabulating an excuse is itself a failure.\n");
        output.push_str("14. Narrate as you work — before a tool call (or batch) write ONE short line of intent (what + why); summarize key results in a sentence; recap briefly at the end. Every line must carry a finding, decision, or reason; empty \"now I'll…\" announcements are noise (rule 12).\n\n");
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
        assert!(out.contains("One action per turn"));
        // Loop-mechanics rules retained here: close-the-loop, 2-strike,
        // subagent escalation, typo-first, no-repeat, aggregate.
        assert!(out.contains("Close the loop"));
        assert!(out.contains("2-strike rule"));
        // Rule 6 must teach switching, not just stopping.
        assert!(out.contains("SWITCH"));
        assert!(out.contains("Subagent error/timeout"));
        assert!(out.contains("Suspected typo"));
        assert!(out.contains("No-repeat"));
        assert!(out.contains("Aggregate before iterating"));
        // Rule 11: delegate heavy multi-source research to parallel sub-agents
        // that return digests, so the main context stays lean for synthesis.
        assert!(out.contains("Delegate heavy research"));
        assert!(out.contains("batch_tasks"));
        // Rule 12: converge — every open-ended task (report, bug hunt,
        // research) must move toward a conclusion once returns diminish.
        assert!(out.contains("12. Converge"));
        assert!(out.contains("VERY NEXT action is the delivering one"));
        // Rule 13: evidence integrity — never invent tool output or claim an
        // action you didn't perform; report real failures/limitations.
        assert!(out.contains("13. Never fabricate"));
        assert!(out.contains("Confabulating an excuse"));
        // Rule 14: narrate each step — one short line of intent, summarize key
        // results, recap at the end.
        assert!(out.contains("14. Narrate as you work"));
        // De-duplication guard: persistence / fallback-ladder / goal-vs-method
        // doctrine now lives ONLY in ProviderGuidanceLayer's persistence block
        // (always co-fires in Full mode). It must NOT reappear here.
        assert!(!out.contains("fallback ladder"));
        assert!(!out.contains("Goal vs method"));
        assert!(!out.contains("Fail ONLY when every reasonable"));
    }

    #[test]
    fn test_guidelines_priority_and_paths() {
        assert_eq!(GuidelinesLayer.priority(), 1300);
        let paths = GuidelinesLayer.paths();
        assert_eq!(paths.len(), 5);
        assert!(paths.contains(&AssemblyPath::Cached));
    }
}
