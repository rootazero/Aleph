//! Custom instructions section — user-provided additional instructions.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render(instructions: &str) -> PromptSection {
    PromptSection {
        name: "custom_instructions".into(),
        stability: Stability::Stable,
        priority: 1200,
        protected: false,
        content: format!("# Additional Instructions\n\n{}", instructions),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_correct() {
        let section = render("Always reply in French.");
        assert_eq!(section.name, "custom_instructions");
        assert_eq!(section.priority, 1200);
        assert!(!section.protected);
        assert!(section.content.contains("Always reply in French."));
    }
}
