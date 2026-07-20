//! `AgentRoleLayer` — sub-agent role header and protocol constraints (priority 55)

use crate::agents::AgentMode;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

/// Injects sub-agent role descriptions and protocol constraints.
///
/// Priority 55 — runs right after `SoulLayer` (50) and before `ProfileLayer` (75).
/// Active on the Basic, Soul, and Context assembly paths.
pub struct AgentRoleLayer;

// --- Static protocol content blocks ----------------------------------------

// Each block is a lean role framing only. Tool availability enforces the
// read-only / no-Bash constraints at runtime, `SessionBudgetLayer` surfaces the
// iteration cap, and a capable model handles its own behavioral rules and
// output shape — so the old per-agent how-to manuals + output-format templates
// (notably the VERDICT form) were removed as a few-shot cage (R7/R9). Keep only
// what a model cannot infer from its task and tools: the specialist role and,
// for verify, the adversarial mindset that counters default confirmation bias.

const EXPLORE_CONSTRAINTS: &str =
    "# Explore Agent\nRead-only exploration specialist: gather information and report; \
     do not modify, create, or delete (your tool set enforces this).";

const CODER_GUIDELINES: &str =
    "# Coder Agent\nCode-writing specialist: make focused, correct edits that fit the \
     project's existing conventions.";

const RESEARCHER_PROTOCOL: &str =
    "# Researcher Agent\nInformation-gathering specialist: search, fetch, and synthesize \
     across sources. Separate fact from inference and cite what you rely on.";

const VERIFY_PROTOCOL: &str =
    "# Verification Agent\nAdversarial verifier: try to break the implementation, don't \
     confirm it. Run the checks the task warrants, report what you actually observed (not \
     what you expected), and end with a clear pass/fail verdict and the evidence behind it. \
     Read-only.";

const PLAN_PROTOCOL: &str =
    "# Plan Agent\nRead-only planning specialist: analyze the codebase and produce an \
     actionable, ordered implementation plan without modifying files.";

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
You are **{agent_id}**, a specialized sub-agent of Aleph. Complete the delegated task within your declared tool set and end with a concise report — what you did, key findings or changes, and recommended next steps. If your tools can't finish it, say what's missing rather than guessing."#
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
        // `Cached` is the live main-loop path
        // (`build_system_prompt_cached_with_mode`). The harness-bridge runner
        // resolves `agent_def` from the registry and threads it via
        // `with_agent(def)` before walking the Cached path (see
        // `harness_bridge/prompt_build.rs`), so a registered SubAgent
        // (explore / coder / researcher / plan / verify) running as the loop
        // agent — via agent-switch or team dispatch — needs this layer on the
        // Cached path or its role header + protocol constraints silently vanish
        // (same class of bug the Soul / Profile / Role / Citation layers were
        // fixed for). For a primary agent the layer no-ops (mode != SubAgent,
        // no `prompt_sections`), so this is a no-op on the main path and stays
        // cache-safe (Stable, byte-stable per agent). `Basic` is kept for the
        // inline `subagent_spawner` prompt build.
        &[
            AssemblyPath::Basic,
            AssemblyPath::Soul,
            AssemblyPath::Cached,
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
            out.contains("Adversarial verifier"),
            "Should contain verify role framing"
        );
        assert!(
            out.contains("pass/fail verdict"),
            "Should retain the verdict expectation"
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
        // Cached is the live main-loop path: a registered sub-agent running via
        // the harness-bridge runner must get its role header + protocol blocks.
        assert!(paths.contains(&AssemblyPath::Cached));
        assert_eq!(paths.len(), 3);
    }
}
