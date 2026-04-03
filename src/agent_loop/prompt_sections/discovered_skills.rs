//! Discovered skills section — dynamically discovered skill metadata.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};
use crate::agent_loop::skill_prefetch::SkillInfo;

pub fn render(skills: &[SkillInfo]) -> PromptSection {
    let content = if skills.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> = skills
            .iter()
            .map(|s| format!("- **{}**: {}", s.name, s.description))
            .collect();
        format!("## Discovered Skills\n{}", lines.join("\n"))
    };

    PromptSection {
        name: "discovered_skills".into(),
        stability: Stability::Dynamic,
        priority: 1800,
        protected: false,
        content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_skills_produces_empty_content() {
        let section = render(&[]);
        assert!(section.content.is_empty());
        assert_eq!(section.name, "discovered_skills");
        assert_eq!(section.priority, 1800);
    }

    #[test]
    fn renders_skill_list() {
        let skills = vec![
            SkillInfo {
                name: "code-review".into(),
                description: "Review code quality".into(),
                schema: None,
            },
            SkillInfo {
                name: "tdd-guide".into(),
                description: "Test-driven development".into(),
                schema: None,
            },
        ];
        let section = render(&skills);
        assert!(section
            .content
            .contains("**code-review**: Review code quality"));
        assert!(section
            .content
            .contains("**tdd-guide**: Test-driven development"));
    }
}
