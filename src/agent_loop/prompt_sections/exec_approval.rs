//! Exec Approval section — instructs LLM how to emit approval decisions alongside tool calls.

use std::collections::HashMap;

use crate::agent_loop::prompt_builder::{PromptSection, Stability};
use crate::exec::approval::types::TrustStage;

/// Context for rendering the exec approval prompt section.
#[derive(Debug, Clone, Default)]
pub struct ExecApprovalContext {
    /// Map of tool name to its current TrustStage.
    pub trust_stages: HashMap<String, TrustStage>,
    /// Tools that require mandatory confirmation regardless of LLM decision.
    pub always_confirm: Vec<String>,
}

impl ExecApprovalContext {
    /// Create a new context with the given trust stages and always-confirm list.
    pub fn new(trust_stages: HashMap<String, TrustStage>, always_confirm: Vec<String>) -> Self {
        Self {
            trust_stages,
            always_confirm,
        }
    }
}

/// Render the exec approval prompt section.
pub fn render(ctx: &ExecApprovalContext) -> PromptSection {
    let mut content = String::new();

    content.push_str("# Tool Execution Approval\n\n");
    content.push_str(
        "When you decide to use a tool, you MUST emit an approval decision tag immediately\n",
    );
    content.push_str("after your reasoning and before or alongside your tool_call.\n\n");

    content.push_str("## Approval Tag Format\n\n");
    content.push_str("```\n");
    content.push_str("<exec-approval>{\"action\":\"auto_execute|ask_user|block\",\"reason\":\"...\"[,,\"block_action\":\"notify|retry\"]}</exec-approval>\n");
    content.push_str("```\n\n");

    content.push_str("## Decision Guidelines\n\n");

    content
        .push_str("- **auto_execute**: Use ONLY for clearly safe operations on trusted tools.\n");
    content.push_str("  - Tool is at Verified trust stage AND operation is routine.\n");
    content.push_str(
        "  - Examples: reading files, querying information, non-destructive operations.\n\n",
    );

    content.push_str(
        "- **ask_user**: Use when uncertain, tool is at Draft/Trial stage, or operation\n",
    );
    content.push_str("  has meaningful side effects.\n\n");

    content.push_str(
        "- **block + notify**: Use for dangerous operations (file deletion, credential access,\n",
    );
    content.push_str("  network calls to external services).\n\n");

    content.push_str("- **block + retry**: Use when a safer alternative approach is available.\n");
    content.push_str("  The system will present your alternative to the user.\n\n");

    content.push_str("## Critical Rules\n\n");
    content.push_str(
        "- NEVER reproduce tool parameter values (paths, URLs, tokens, IDs) in the reason.\n",
    );
    content.push_str(
        "- The reason should describe WHAT you concluded, not the specific input values.\n",
    );
    content.push_str("- Example BAD: `reason: \"Deleting /home/user/secrets.txt\"\n");
    content.push_str("- Example GOOD: `reason: \"Routine log cleanup\"\n\n");

    // TrustStage aggregate
    if !ctx.trust_stages.is_empty() {
        content.push_str("## Tool Trust Levels\n\n");
        let trust_list: Vec<String> = ctx
            .trust_stages
            .iter()
            .map(|(name, stage)| format!("{}: {}", name, stage_tag(stage)))
            .collect();
        content.push_str(&format!("{}\n\n", trust_list.join(", ")));
    }

    // Always-confirm list
    if !ctx.always_confirm.is_empty() {
        content.push_str("## Mandatory Confirmation Required\n\n");
        content.push_str(
            "These tools ALWAYS require user confirmation regardless of your decision:\n",
        );
        content.push_str(&format!("{}\n\n", ctx.always_confirm.join(", ")));
    }

    content
        .push_str("Your approval decision is advisory — tools in the mandatory list will always\n");
    content.push_str("require user confirmation.\n");

    PromptSection {
        name: "exec_approval".into(),
        stability: Stability::Dynamic,
        priority: 450,
        protected: false,
        content,
    }
}

/// Convert TrustStage to a short tag string.
fn stage_tag(stage: &TrustStage) -> &'static str {
    match stage {
        TrustStage::Draft => "Draft",
        TrustStage::Trial => "Trial",
        TrustStage::Verified => "Verified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hashmap(kvs: &[(&str, TrustStage)]) -> HashMap<String, TrustStage> {
        kvs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn empty_context_renders_instructions_only() {
        let ctx = ExecApprovalContext::default();
        let section = render(&ctx);
        assert!(section.content.contains("auto_execute|ask_user|block"));
        assert!(!section.content.contains("Trust Levels"));
        assert!(!section.content.contains("Mandatory Confirmation"));
        assert_eq!(section.stability, Stability::Dynamic);
        assert_eq!(section.priority, 450);
    }

    #[test]
    fn trust_stages_rendered() {
        let ctx = ExecApprovalContext {
            trust_stages: hashmap(&[
                ("read_file", TrustStage::Verified),
                ("bash_exec", TrustStage::Draft),
            ]),
            always_confirm: vec![],
        };
        let section = render(&ctx);
        assert!(section.content.contains("Trust Levels"));
        assert!(section.content.contains("read_file: Verified"));
        assert!(section.content.contains("bash_exec: Draft"));
    }

    #[test]
    fn always_confirm_renders() {
        let ctx = ExecApprovalContext {
            trust_stages: hashmap(&[]),
            always_confirm: vec!["bash_exec".to_string(), "file_delete".to_string()],
        };
        let section = render(&ctx);
        assert!(section.content.contains("Mandatory Confirmation"));
        assert!(section.content.contains("bash_exec"));
        assert!(section.content.contains("file_delete"));
    }

    #[test]
    fn full_context_renders_all_sections() {
        let ctx = ExecApprovalContext {
            trust_stages: hashmap(&[
                ("read_file", TrustStage::Verified),
                ("write_file", TrustStage::Trial),
            ]),
            always_confirm: vec!["bash_exec".to_string()],
        };
        let section = render(&ctx);
        assert!(section.content.contains("Tool Execution Approval"));
        assert!(section.content.contains("Trust Levels"));
        assert!(section.content.contains("Mandatory Confirmation"));
        assert!(section
            .content
            .contains("NEVER reproduce tool parameter values"));
    }

    #[test]
    fn section_metadata() {
        let ctx = ExecApprovalContext::default();
        let section = render(&ctx);
        assert_eq!(section.name, "exec_approval");
        assert!(!section.protected);
    }
}
