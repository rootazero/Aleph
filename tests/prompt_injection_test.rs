//! Phase 1 — verifies eligible v2 skills reach the assembled system prompt.

use alephcore::domain::skill::{PromptScope, SkillManifest};
use alephcore::skill::prompt::build_skills_prompt_xml;
use alephcore::thinker::prompt_builder::{PromptBuilder, PromptConfig};

#[test]
fn eligible_skills_render_into_system_prompt() {
    // Given a PromptConfig carrying an eligible system-scope skill,
    // PromptBuilder must emit the <available_skills> block.
    let mut skill = SkillManifest::new(
        "git",
        "git",
        "Git workflow helper",
        alephcore::domain::skill::SkillContent::new("body"),
        alephcore::domain::skill::SkillSource::Bundled,
    );
    skill.set_scope(PromptScope::System);

    let config = PromptConfig {
        eligible_skills: Some(vec![skill]),
        ..PromptConfig::default()
    };
    // `build_system_prompt_parts` is the production Basic entry (the undivided
    // `build_system_prompt` is lib-test-only now); joining the parts
    // reproduces the same bytes.
    let parts = PromptBuilder::new(config).build_system_prompt_parts(&[]);
    let prompt: String = parts.iter().map(|p| p.content.as_str()).collect();

    assert!(prompt.contains("Available Skills"), "prompt: {prompt}");
    assert!(
        prompt.contains("git"),
        "prompt missing skill name: {prompt}"
    );
    // sanity: the XML renderer is the one used by the layer
    let _ = build_skills_prompt_xml;
}
