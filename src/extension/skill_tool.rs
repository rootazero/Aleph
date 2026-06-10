//! Skill Tool - LLM-callable skill invocation
//!
//! This module provides the core logic for invoking skills as LLM tools:
//! template rendering and structured results. Tool-level access control
//! lives in the dispatch pipeline (`tools/scoped/dispatch.rs`), driven by
//! the merged `[policies.tool_permissions]` policy — not here.

use super::error::ExtensionResult;
use super::template::SkillTemplate;
use super::types::{ExtensionSkill, SkillMetadata, SkillToolResult};
use tracing::debug;

/// Invoke a skill and return structured result
///
/// This is the core function called when LLM invokes the skill tool.
pub async fn invoke_skill(
    skill: &ExtensionSkill,
    arguments: &str,
) -> ExtensionResult<SkillToolResult> {
    let qualified_name = skill.qualified_name();

    // Create template and render
    let template = SkillTemplate::new(&skill.content, &skill.source_path);
    let rendered_content = template.render(arguments).await?;

    // Build result
    let result = SkillToolResult {
        title: format!("Loaded skill: {}", skill.name),
        content: rendered_content,
        base_dir: template.base_dir().to_path_buf(),
        metadata: SkillMetadata {
            name: skill.name.clone(),
            qualified_name,
            source: skill.source,
        },
    };

    debug!(
        "Skill {} invoked successfully with base_dir: {:?}",
        result.metadata.qualified_name, result.base_dir
    );

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::DiscoverySource;
    use std::path::PathBuf;

    fn create_test_skill() -> ExtensionSkill {
        ExtensionSkill {
            name: "test-skill".to_string(),
            plugin_name: Some("my-plugin".to_string()),
            skill_type: crate::extension::SkillType::Skill,
            description: "A test skill".to_string(),
            content: "Hello $ARGUMENTS!".to_string(),
            disable_model_invocation: false,
            scope: crate::extension::PromptScope::System,
            bound_tool: None,
            source_path: PathBuf::from("/test/skill/SKILL.md"),
            source: DiscoverySource::AlephGlobal,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_invoke_skill() {
        let skill = create_test_skill();

        let result = invoke_skill(&skill, "World").await.unwrap();

        assert_eq!(result.title, "Loaded skill: test-skill");
        assert_eq!(result.content, "Hello World!");
        assert_eq!(result.metadata.name, "test-skill");
        assert_eq!(result.metadata.qualified_name, "my-plugin:test-skill");
    }
}
