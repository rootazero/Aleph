//! Directives section — behavioral directives, anti-patterns, and expertise.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render(directives: &[String], anti_patterns: &[String], expertise: &[String]) -> PromptSection {
    let mut bullets: Vec<String> = Vec::new();

    for d in directives {
        bullets.push(format!("- {}", d));
    }
    for a in anti_patterns {
        bullets.push(format!("- NEVER: {}", a));
    }
    if !expertise.is_empty() {
        bullets.push(format!("- Your areas of expertise: {}", expertise.join(", ")));
    }

    let content = if bullets.is_empty() {
        String::new()
    } else {
        format!("# Directives\n\n{}", bullets.join("\n"))
    };

    PromptSection {
        name: "directives".into(),
        stability: Stability::Stable,
        priority: 150,
        protected: false,
        content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_when_no_inputs() {
        let section = render(&[], &[], &[]);
        assert!(section.content.is_empty());
    }

    #[test]
    fn renders_directives() {
        let section = render(&["Be helpful".into()], &[], &[]);
        assert!(section.content.contains("- Be helpful"));
    }

    #[test]
    fn renders_anti_patterns() {
        let section = render(&[], &["lie to users".into()], &[]);
        assert!(section.content.contains("- NEVER: lie to users"));
    }

    #[test]
    fn renders_expertise() {
        let section = render(&[], &[], &["Rust".into(), "Python".into()]);
        assert!(section.content.contains("Your areas of expertise: Rust, Python"));
    }

    #[test]
    fn renders_all_combined() {
        let section = render(
            &["Be helpful".into()],
            &["lie".into()],
            &["Rust".into()],
        );
        assert!(section.content.contains("- Be helpful"));
        assert!(section.content.contains("- NEVER: lie"));
        assert!(section.content.contains("Your areas of expertise: Rust"));
    }
}
