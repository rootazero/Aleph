//! XML prompt builder — generates `<available_skills>` XML for system prompt injection.

use crate::domain::skill::SkillManifest;

/// Deferred loading guidance appended after skill index in system prompts.
/// Tells the LLM to call `skill_read` before executing a skill.
pub const DEFERRED_LOADING_GUIDANCE: &str =
    "To use a skill, first call the `skill_read` tool with the skill name \
     to load its full instructions, then follow those instructions. \
     Use `skill_list` to discover available skills if needed.\n\n\
     When a user's request matches a skill's <when> trigger, proactively \
     invoke that skill without waiting for an explicit request.";

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
        if let Some(when) = skill.when_to_use() {
            buf.push_str("    <when>");
            buf.push_str(&escape_xml(when));
            buf.push_str("</when>\n");
        }
        buf.push_str("  </skill>\n");
    }

    buf.push_str("</available_skills>");
    buf
}

use crate::thinker::xml_util::escape_xml;

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
        let all = [&disabled, &model_disabled, &visible];
        let visible_only: Vec<&&SkillManifest> =
            all.iter().filter(|s| s.is_model_visible()).collect();
        assert_eq!(visible_only.len(), 1);

        let xml = build_skills_prompt_xml(&visible_only.into_iter().copied().collect::<Vec<_>>());
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

    #[test]
    fn xml_includes_when_to_use() {
        let mut skill = make_skill("Code Review", "Reviews code quality");
        skill.set_when_to_use("When code has been modified".to_string());
        let xml = build_skills_prompt_xml(&[&skill]);

        assert!(xml.contains("<when>When code has been modified</when>"));
    }

    #[test]
    fn xml_omits_when_tag_if_none() {
        let skill = make_skill("Simple", "A simple skill");
        let xml = build_skills_prompt_xml(&[&skill]);

        assert!(!xml.contains("<when>"));
        assert!(xml.contains("<name>Simple</name>"));
    }

    #[test]
    fn xml_escapes_when_to_use() {
        let mut skill = make_skill("Test", "Test skill");
        skill.set_when_to_use("When <user> asks & needs help".to_string());
        let xml = build_skills_prompt_xml(&[&skill]);

        assert!(xml.contains("<when>When &lt;user&gt; asks &amp; needs help</when>"));
    }

    #[test]
    fn deferred_loading_guidance_includes_proactive_trigger() {
        assert!(
            DEFERRED_LOADING_GUIDANCE.contains("proactively"),
            "Guidance should mention proactive invocation"
        );
        assert!(
            DEFERRED_LOADING_GUIDANCE.contains("<when>"),
            "Guidance should reference the <when> trigger tag"
        );
    }
}
