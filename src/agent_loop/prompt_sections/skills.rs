//! Skills section — lists available skills from SkillManifest registry.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};
use crate::domain::skill::{PromptScope, SkillManifest};
use crate::skill::prompt::build_skills_prompt_xml;

pub fn render(skills: &[SkillManifest], active_tool_names: &[&str]) -> PromptSection {
    let filtered: Vec<&SkillManifest> = skills
        .iter()
        .filter(|s| match *s.scope() {
            PromptScope::System => true,
            PromptScope::Tool => s
                .bound_tool()
                .is_some_and(|bound| active_tool_names.contains(&bound)),
            PromptScope::Standalone | PromptScope::Disabled => false,
        })
        .collect();

    let content = if filtered.is_empty() {
        String::new()
    } else {
        let xml = build_skills_prompt_xml(&filtered);
        format!(
            "# Available Skills\n\n\
             You can invoke skills using the `skill` tool. \
             Skills provide specialized instructions for specific tasks.\n\
             {}\n\n{}",
            crate::skill::prompt::DEFERRED_LOADING_GUIDANCE,
            xml
        )
    };

    PromptSection {
        name: "skills".into(),
        stability: Stability::Stable,
        priority: 1000,
        protected: false,
        content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_skills_produces_empty_content() {
        let section = render(&[], &[]);
        assert!(section.content.is_empty());
        assert_eq!(section.name, "skills");
        assert_eq!(section.priority, 1000);
    }
}
