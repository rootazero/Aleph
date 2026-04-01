//! Output efficiency section — concise output guidance.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

const OUTPUT_EFFICIENCY: &str = "\
# Output Efficiency

IMPORTANT: Go straight to the point. Try the simplest approach first without going in circles. Do not overdo it. Be extra concise.

Keep your text output brief and direct. Lead with the answer or action, not the reasoning. Skip filler words, preamble, and unnecessary transitions. Do not restate what the user said — just do it. When explaining, include only what is necessary for the user to understand.

Focus text output on:
- Decisions that need the user's input
- High-level status updates at natural milestones
- Errors or blockers that change the plan

If you can say it in one sentence, don't use three. Prefer short, direct sentences over long explanations. This does not apply to code or tool calls.";

pub fn render() -> PromptSection {
    PromptSection {
        name: "output_efficiency".into(),
        stability: Stability::Stable,
        priority: 800,
        protected: false,
        content: OUTPUT_EFFICIENCY.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_correct() {
        let section = render();
        assert_eq!(section.name, "output_efficiency");
        assert_eq!(section.priority, 800);
        assert!(!section.protected);
        assert!(section.content.contains("Output Efficiency"));
    }
}
