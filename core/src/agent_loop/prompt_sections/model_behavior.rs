//! Model behavior section — model-specific behavioral guidance.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render(content: &str) -> PromptSection {
    PromptSection {
        name: "model_behavior".into(),
        stability: Stability::Stable,
        priority: 200,
        protected: false,
        content: format!("# Model Behavior\n\n{}", content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_correct() {
        let section = render("Think step by step.");
        assert_eq!(section.name, "model_behavior");
        assert_eq!(section.priority, 200);
        assert!(!section.protected);
        assert!(section.content.contains("Think step by step."));
    }
}
