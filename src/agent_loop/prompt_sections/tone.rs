//! Tone section — communication style guidance.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render(tone: &str) -> PromptSection {
    PromptSection {
        name: "tone".into(),
        stability: Stability::Stable,
        priority: 100,
        protected: false,
        content: format!("# Communication Style\n\n{}", tone),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_correct() {
        let section = render("Be concise.");
        assert_eq!(section.name, "tone");
        assert_eq!(section.priority, 100);
        assert!(!section.protected);
        assert!(section.content.contains("Be concise."));
    }
}
