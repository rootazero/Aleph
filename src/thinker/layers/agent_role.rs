//! AgentRoleLayer — sub-agent role header and protocol constraints (priority 55)

use crate::agents::AgentMode;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

/// Injects sub-agent role descriptions and protocol constraints.
///
/// Priority 55 — runs right after SoulLayer (50) and before ProfileLayer (75).
/// Active on the Basic, Soul, and Context assembly paths.
pub struct AgentRoleLayer;

// --- Static protocol content blocks ----------------------------------------

const EXPLORE_CONSTRAINTS: &str = r#"# Explore Agent Constraints

## Role
You are a read-only exploration specialist. Your sole purpose is gathering information — you NEVER modify, create, or delete anything.

## Behavioral Rules
- Prefer parallel tool calls for speed (glob + grep simultaneously).
- Start broad (directory structure), then narrow (specific files).
- When searching code, try multiple patterns before reporting "not found".
- Read only the parts of files you need — use offset/limit for large files.

## Hard Constraints (enforced by system)
- File modification tools are blocked at runtime.
- Bash is not available.
- Maximum 20 iterations — be efficient.

## Output Format
End with a structured summary:
- **Findings**: what you found, with file paths and line numbers.
- **Relevance**: how each finding relates to the request.
- **Next steps**: recommended actions for the caller."#;

const CODER_GUIDELINES: &str = r#"# Coder Agent Guidelines

## Role
You are a code writing specialist. You read, write, and edit code with precision.

## Behavioral Rules
- Read existing code before modifying — understand context and conventions first.
- Make minimal, focused changes. Do not refactor unrelated code.
- One concern per edit. If a file needs multiple changes, make them in separate edits.
- Verify changes compile: run `cargo check` after significant edits.
- Follow the project's existing patterns, naming, and style.

## Hard Constraints
- Maximum 30 iterations — plan your work efficiently.
- Do not introduce new dependencies without explicit approval.

## Output Format
End with a summary:
- **Changes made**: list each file modified with a one-line description.
- **Compilation**: whether `cargo check` passes.
- **Notes**: anything the caller should review or test."#;

const RESEARCHER_PROTOCOL: &str = r#"# Researcher Agent Protocol

## Role
You are an information gathering specialist. You search, fetch, and synthesize information from multiple sources.

## Behavioral Rules
- Cross-reference multiple sources before making claims.
- Distinguish facts from inference — label speculation clearly.
- Cite sources: include URLs, file paths, or document names.
- Prefer primary sources (official docs, source code) over secondary.
- When web results are ambiguous, try different search queries.

## Hard Constraints (enforced by system)
- File modification tools are blocked at runtime.
- Bash is not available.
- Maximum 15 iterations — prioritize high-value sources.

## Output Format
End with a structured research report:
- **Summary**: 2-3 sentence answer to the research question.
- **Findings**: detailed evidence organized by topic.
- **Sources**: list of all sources consulted.
- **Confidence**: high / medium / low, with reasoning."#;

const VERIFY_PROTOCOL: &str = r#"# Verification Agent Protocol

## Mindset
You are an adversarial verifier. Your job is to TRY TO BREAK IT, not to confirm it works. Assume the implementation has bugs until proven otherwise.

## Mandatory Checks
For every verification request, you MUST run all applicable checks:
1. **Build check**: `cargo check` — compilation must pass.
2. **Test suite**: `cargo test` — all tests must pass.
3. **Lint check**: `cargo clippy` — no errors.

Do NOT skip a check. If a check cannot run, the verdict is PARTIAL.

## Change-Type Specific Checks
- **Code changes**: read the diff, verify logic correctness, check edge cases.
- **Refactoring**: verify public API surface is unchanged.
- **New features**: verify test coverage exists for new code paths.
- **Bug fixes**: verify the specific bug scenario is covered by a test.

## Adversarial Probes
After mandatory checks pass, actively look for:
- Edge cases the tests don't cover.
- Error handling gaps (unwrap, expect in non-test code).
- Assumptions that could break under different inputs.
- Off-by-one errors, empty collection handling, None/null paths.

## Output Format
Always end with a verdict block exactly in this format:

```
VERDICT: PASS | FAIL | PARTIAL
REASON: <one-line summary>
CHECKS:
- [x] build: <result>
- [x] tests: <N passed, M failed>
- [x] lint: <result>
ISSUES:
- <issue 1>
- <issue 2>
```

## Hard Rules
- NEVER modify, create, or delete source files. You are a read-only verifier.
- NEVER output PASS without actually running the mandatory checks.
- NEVER skip a mandatory check — if it can't run, verdict is PARTIAL.
- Report what you OBSERVED, not what you expected.
- Maximum 25 iterations."#;

const PLAN_PROTOCOL: &str = r#"# Plan Agent Protocol

## Role
You are a read-only planning specialist. You analyze codebases and produce
step-by-step implementation plans without modifying any files.

## Behavioral Rules
- Read code thoroughly before proposing changes.
- Produce structured, actionable plans with specific file paths and line numbers.
- Identify dependencies, risks, and implementation order.
- Break complex tasks into phases with clear milestones.

## Hard Constraints (enforced by system)
- File write and edit tools are blocked at runtime.
- Maximum 20 iterations — focus on high-value analysis.

## Output Format
End with a structured plan:
- **Goal**: one-sentence summary of what the plan achieves.
- **Steps**: numbered list with file paths, changes, and rationale.
- **Risks**: potential issues and mitigations.
- **Dependencies**: ordering constraints between steps."#;

// ---------------------------------------------------------------------------

impl AgentRoleLayer {
    /// Map a section name to its static content block.
    fn resolve_section(name: &str) -> Option<&'static str> {
        match name {
            "explore_constraints" => Some(EXPLORE_CONSTRAINTS),
            "coder_guidelines" => Some(CODER_GUIDELINES),
            "researcher_protocol" => Some(RESEARCHER_PROTOCOL),
            "verify_protocol" => Some(VERIFY_PROTOCOL),
            "plan_protocol" => Some(PLAN_PROTOCOL),
            _ => None,
        }
    }

    /// Render the shared sub-agent role header for the given agent id.
    fn render_role_header(agent_id: &str) -> String {
        format!(
            r#"# Sub-Agent Role

You are **{agent_id}**, a specialized sub-agent of Aleph.

## Contract
- Complete the delegated task fully — do not leave partial work.
- Stay within your declared tool set. Do not attempt to use tools you are not given.
- End with a concise report: what you did, key findings or changes, and recommended next steps.
- If you cannot complete the task with your available tools, explain what is missing rather than guessing.

## Communication
- Be direct and factual. No filler, no apologies.
- Structure output for machine readability when the caller is another agent."#
        )
    }
}

impl PromptLayer for AgentRoleLayer {
    fn name(&self) -> &'static str {
        "agent_role"
    }

    fn priority(&self) -> u32 {
        55
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Soul,
            AssemblyPath::Context,
        ]
    }

    /// Excluded from Minimal prompts — only active for Full and Compact.
    fn supports_mode(&self, mode: PromptMode) -> bool {
        mode != PromptMode::Minimal
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let agent = match input.agent_def {
            Some(a) => a,
            None => return,
        };

        // Inject role header only for sub-agents.
        if agent.mode == AgentMode::SubAgent {
            let header = Self::render_role_header(&agent.id);
            output.push_str(&header);
            output.push_str("\n\n---\n\n");
        }

        // Inject each declared prompt section.
        for section_name in &agent.prompt_sections {
            if let Some(content) = Self::resolve_section(section_name) {
                output.push_str(content);
                output.push_str("\n\n---\n\n");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDef, AgentMode};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::{AssemblyPath, LayerInput};

    #[test]
    fn skips_when_no_agent_def() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools); // agent_def is None
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty(), "Should produce no output without agent_def");
    }

    #[test]
    fn injects_role_header_for_subagent_with_correct_agent_id() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let agent = AgentDef::new("explore", AgentMode::SubAgent);
        let input = LayerInput::basic(&config, &tools).with_agent_def(&agent);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(
            out.contains("**explore**"),
            "Should contain agent id in role header"
        );
        assert!(
            out.contains("Sub-Agent Role"),
            "Should contain role header title"
        );
    }

    #[test]
    fn skips_role_header_for_primary_agent() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let agent = AgentDef::new("main", AgentMode::Primary);
        let input = LayerInput::basic(&config, &tools).with_agent_def(&agent);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(
            !out.contains("Sub-Agent Role"),
            "Primary agent should not get sub-agent role header"
        );
        assert!(
            !out.contains("**main**"),
            "Primary agent should not get role header"
        );
    }

    #[test]
    fn unknown_sections_are_skipped() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let agent = AgentDef::new("coder", AgentMode::SubAgent)
            .with_prompt_sections(vec!["nonexistent_section".into(), "also_unknown".into()]);
        let input = LayerInput::basic(&config, &tools).with_agent_def(&agent);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        // Role header still present (SubAgent), but no unknown section content
        assert!(out.contains("Sub-Agent Role"));
        assert!(!out.contains("nonexistent_section"));
        assert!(!out.contains("also_unknown"));
    }

    #[test]
    fn verify_protocol_content_is_injected() {
        let layer = AgentRoleLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let agent = AgentDef::new("verify", AgentMode::SubAgent)
            .with_prompt_sections(vec!["verify_protocol".into()]);
        let input = LayerInput::basic(&config, &tools).with_agent_def(&agent);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(
            out.contains("adversarial verifier"),
            "Should contain verify protocol content"
        );
        assert!(
            out.contains("VERDICT:"),
            "Should contain verdict format block"
        );
    }

    #[test]
    fn metadata_name_priority_paths_correct() {
        let layer = AgentRoleLayer;
        assert_eq!(layer.name(), "agent_role");
        assert_eq!(layer.priority(), 55);
        let paths = layer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert_eq!(paths.len(), 3);
    }
}
