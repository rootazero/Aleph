//! Tone and style section — output formatting and communication style rules.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

const TONE_AND_STYLE: &str = "\
# Tone and Style

- Only use emojis if the user explicitly requests it. Avoid using emojis in all communication unless asked.
- Your responses should be short and concise.
- When referencing specific functions or pieces of code include the pattern file_path:line_number to allow the user to easily navigate to the source code location.
- Do not use a colon before tool calls. Your tool calls may not be shown directly in the output, so text like \"Let me read the file:\" followed by a tool call should just be \"Let me read the file.\" with a period.";

pub fn render() -> PromptSection {
    PromptSection {
        name: "tone_and_style".into(),
        stability: Stability::Stable,
        priority: 700,
        protected: false,
        content: TONE_AND_STYLE.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_correct() {
        let section = render();
        assert_eq!(section.name, "tone_and_style");
        assert_eq!(section.priority, 700);
        assert!(!section.protected);
        assert!(section.content.contains("Tone and Style"));
    }
}
