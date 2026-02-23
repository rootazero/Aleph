//! XML prompt builder — generates `<available_skills>` XML for system prompt injection.

use crate::domain::skill::SkillManifest;

/// Build an XML fragment listing the given skills for injection into a system prompt.
///
/// Returns an empty string if the slice is empty.
///
/// Output format:
/// ```xml
/// <available_skills>
///   <skill>
///     <name>Git Commit</name>
///     <description>Helps write commit messages</description>
///   </skill>
/// </available_skills>
/// ```
pub fn build_skills_prompt_xml(skills: &[&SkillManifest]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut buf = String::from("<available_skills>\n");

    for skill in skills {
        buf.push_str("  <skill>\n");
        buf.push_str("    <name>");
        buf.push_str(&escape_xml(skill.name()));
        buf.push_str("</name>\n");
        buf.push_str("    <description>");
        buf.push_str(&escape_xml(skill.description()));
        buf.push_str("</description>\n");
        buf.push_str("  </skill>\n");
    }

    buf.push_str("</available_skills>");
    buf
}

/// Escape XML special characters.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{
        InvocationPolicy, PromptScope, SkillContent, SkillId, SkillManifest, SkillSource,
    };

    fn make_skill(name: &str, desc: &str) -> SkillManifest {
        SkillManifest::new(
            SkillId::new(name.to_lowercase().replace(' ', "-")),
            name,
            desc,
            SkillContent::new("content"),
            SkillSource::Bundled,
        )
    }

    #[test]
    fn empty_skills_empty_xml() {
        let result = build_skills_prompt_xml(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn single_skill_xml() {
        let skill = make_skill("Git Commit", "Helps write commit messages");
        let xml = build_skills_prompt_xml(&[&skill]);

        assert!(xml.starts_with("<available_skills>"));
        assert!(xml.ends_with("</available_skills>"));
        assert!(xml.contains("<name>Git Commit</name>"));
        assert!(xml.contains("<description>Helps write commit messages</description>"));
    }

    #[test]
    fn multiple_skills_xml() {
        let s1 = make_skill("Git Commit", "Write commits");
        let s2 = make_skill("Docker Build", "Build images");
        let xml = build_skills_prompt_xml(&[&s1, &s2]);

        // Count <skill> occurrences
        let count = xml.matches("<skill>").count();
        assert_eq!(count, 2);

        assert!(xml.contains("<name>Git Commit</name>"));
        assert!(xml.contains("<name>Docker Build</name>"));
    }

    #[test]
    fn disabled_scope_excluded() {
        // Verify is_model_visible correctly identifies disabled skills
        let mut disabled = make_skill("Hidden", "Not visible");
        disabled.set_scope(PromptScope::Disabled);
        assert!(!disabled.is_model_visible());

        let mut model_disabled = make_skill("Model Hidden", "Not for model");
        model_disabled.set_invocation(InvocationPolicy {
            disable_model_invocation: true,
            ..Default::default()
        });
        assert!(!model_disabled.is_model_visible());

        // A visible skill should pass
        let visible = make_skill("Visible", "Can be seen");
        assert!(visible.is_model_visible());

        // Only include model-visible skills
        let all = vec![&disabled, &model_disabled, &visible];
        let visible_only: Vec<&&SkillManifest> =
            all.iter().filter(|s| s.is_model_visible()).collect();
        assert_eq!(visible_only.len(), 1);

        let xml = build_skills_prompt_xml(
            &visible_only.into_iter().copied().collect::<Vec<_>>(),
        );
        assert!(xml.contains("<name>Visible</name>"));
        assert!(!xml.contains("Hidden"));
        assert!(!xml.contains("Model Hidden"));
    }

    #[test]
    fn xml_escaping() {
        let skill = make_skill("A & B", "Uses <tags> & stuff");
        let xml = build_skills_prompt_xml(&[&skill]);
        assert!(xml.contains("<name>A &amp; B</name>"));
        assert!(xml.contains("&lt;tags&gt;"));
    }
}
