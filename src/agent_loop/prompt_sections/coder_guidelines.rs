//! Behavioral guidelines for the Coder agent.
use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render() -> PromptSection {
    PromptSection {
        name: "coder_guidelines".into(),
        stability: Stability::Dynamic,
        priority: 60,
        protected: false,
        content: r#"# Coder Agent Guidelines

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
- **Notes**: anything the caller should review or test."#.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn render_has_correct_metadata() {
        let section = render();
        assert_eq!(section.name, "coder_guidelines");
        assert_eq!(section.priority, 60);
        assert!(section.content.contains("code writing specialist"));
    }
}
