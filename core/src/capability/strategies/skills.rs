//! Skills capability strategy.
//!
//! This strategy injects skill instructions from matched SKILL.md files
//! into the payload context for prompt assembly.

use crate::capability::strategy::CapabilityStrategy;
use crate::domain::skill::SkillId;
use crate::domain::Entity;
use crate::error::Result;
use crate::payload::{AgentPayload, Capability};
use crate::skill::SkillSystem;
use async_trait::async_trait;
use tracing::{debug, info, warn};

/// Skills capability strategy
///
/// Matches user input against loaded skills and injects instructions
/// from the matched SKILL.md into the payload context.
pub struct SkillsStrategy {
    /// Skills system containing loaded skills
    skill_system: Option<SkillSystem>,
}

impl SkillsStrategy {
    /// Create a new skills strategy
    pub fn new(skill_system: Option<SkillSystem>) -> Self {
        Self { skill_system }
    }

    /// Update the skills system
    pub fn set_system(&mut self, system: SkillSystem) {
        self.skill_system = Some(system);
    }
}

#[async_trait]
impl CapabilityStrategy for SkillsStrategy {
    fn capability_type(&self) -> Capability {
        Capability::Skills
    }

    fn priority(&self) -> u32 {
        4 // Skills execute last (after Memory=0, Search=1, Mcp=2, Video=3)
    }

    fn is_available(&self) -> bool {
        // Available if system is configured
        self.skill_system.is_some()
    }

    fn validate_config(&self) -> Result<()> {
        // SkillSystem manages its own directories internally
        Ok(())
    }

    async fn health_check(&self) -> Result<bool> {
        if !self.is_available() {
            return Ok(false);
        }

        // Check if system has any skills loaded
        if let Some(system) = &self.skill_system {
            let count = system.list_skills().await.len();
            debug!(skill_count = count, "Skills health check: skills loaded");
            // Skills is healthy as long as system exists
            // (could have 0 skills but still be operational)
            return Ok(true);
        }

        Ok(false)
    }

    fn status_info(&self) -> std::collections::HashMap<String, String> {
        let mut info = std::collections::HashMap::new();
        info.insert("capability".to_string(), "Skills".to_string());
        info.insert("name".to_string(), "skills".to_string());
        info.insert("priority".to_string(), "4".to_string());
        info.insert("available".to_string(), self.is_available().to_string());
        info.insert(
            "has_system".to_string(),
            self.skill_system.is_some().to_string(),
        );
        // Note: status_info is sync, cannot call async list_skills here.
        // We omit skill_count and skills_dir to keep this sync-safe.
        info
    }

    async fn execute(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        let Some(system) = &self.skill_system else {
            warn!("Skills capability requested but no system configured");
            return Ok(payload);
        };

        // Hybrid Mode: Slash Commands vs Progressive Disclosure
        //
        // 1. Explicit slash command (Intent::Skills) → Pre-load instructions for immediate execution
        // 2. General chat → Progressive Disclosure (Agent uses read_skill tool on-demand)
        //
        // This preserves good UX for slash commands while following Claude's official
        // Progressive Disclosure pattern for Agent-driven skill discovery.

        // Check if this is an explicit skill invocation via slash command
        if let crate::payload::Intent::Skills(skill_id) = &payload.meta.intent {
            // Explicit slash command: pre-load the specific skill's instructions
            if let Some(skill) = system.get_skill(&SkillId::new(skill_id)).await {
                info!(
                    skill_id = %skill_id,
                    "Explicit skill invocation via slash command - pre-loading instructions"
                );
                payload.context.skill_instructions = Some(skill.content().as_str().to_string());
                // Also populate available_skills for consistency
                payload.context.available_skills = Some(vec![crate::payload::SkillMetadata {
                    id: skill.id().as_str().to_string(),
                    description: skill.description().to_string(),
                }]);
                return Ok(payload);
            } else {
                warn!(
                    skill_id = %skill_id,
                    "Requested skill not found in system"
                );
                // Fall through to Progressive Disclosure mode
            }
        }

        // Progressive Disclosure Pattern (Claude Official Skills Architecture):
        // - Level 1: Only metadata (id + description) goes into system prompt
        // - Level 2: Full instructions loaded on-demand via read_skill tool
        // - Level 3: Additional resources loaded as needed
        //
        // This ensures the agent actively reads skill instructions (treating them
        // as task directives) rather than passively receiving them (treating as context).

        // Collect skill metadata for all available skills
        let skills = system.list_skills().await;
        if !skills.is_empty() {
            let metadata: Vec<crate::payload::SkillMetadata> = skills
                .iter()
                .map(|s| crate::payload::SkillMetadata {
                    id: s.id().as_str().to_string(),
                    description: s.description().to_string(),
                })
                .collect();

            info!(
                skill_count = metadata.len(),
                "Populated available_skills metadata for Progressive Disclosure"
            );
            payload.context.available_skills = Some(metadata);
        } else {
            debug!("No skills available in system");
        }

        // For general chat, agent will use read_skill tool to load instructions when needed.
        // This is the key change for Progressive Disclosure pattern.

        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};
    use std::fs;
    use tempfile::TempDir;

    fn create_test_skill(
        dir: &std::path::Path,
        name: &str,
        description: &str,
        instructions: &str,
    ) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();

        let content = format!(
            r#"---
name: {}
description: {}
---

{}
"#,
            name, description, instructions
        );

        fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    async fn make_system(skills_dir: std::path::PathBuf) -> SkillSystem {
        let system = SkillSystem::new();
        system.init(vec![skills_dir]).await.unwrap();
        system
    }

    #[tokio::test]
    async fn test_skills_strategy_available() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();
        let system = make_system(skills_dir).await;

        let strategy = SkillsStrategy::new(Some(system));
        assert!(strategy.is_available());
    }

    #[tokio::test]
    async fn test_skills_strategy_not_available() {
        let strategy = SkillsStrategy::new(None);
        assert!(!strategy.is_available());
    }

    #[tokio::test]
    async fn test_skills_strategy_explicit_slash_command_preloads() {
        // Hybrid Mode: Explicit slash command (Intent::Skills) should pre-load instructions
        // for immediate execution, preserving good UX
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(
            &skills_dir,
            "refine-text",
            "Improve and polish writing",
            "# Refine Text\n\nWhen refining text, focus on clarity and conciseness.",
        );

        let system = make_system(skills_dir).await;

        let strategy = SkillsStrategy::new(Some(system));

        let anchor = ContextAnchor::new(None);
        let payload = PayloadBuilder::new()
            .meta(Intent::Skills("refine-text".to_string()), 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Skills],
                ContextFormat::Markdown,
            )
            .user_input("Please improve this text".to_string())
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();

        // Hybrid Mode: explicit slash command pre-loads instructions
        assert!(result.context.skill_instructions.is_some());
        assert!(result
            .context
            .skill_instructions
            .as_ref()
            .unwrap()
            .contains("focus on clarity and conciseness"));

        // Also populates available_skills for consistency
        assert!(result.context.available_skills.is_some());
        let skills = result.context.available_skills.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id, "refine-text");
    }

    #[tokio::test]
    async fn test_skills_strategy_general_chat_progressive_disclosure() {
        // Progressive Disclosure: general chat should NOT pre-load instructions
        // Agent uses read_skill tool on-demand
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(
            &skills_dir,
            "refine-text",
            "Improve and polish writing",
            "# Refine Text\n\nWhen refining text, focus on clarity and conciseness.",
        );

        let system = make_system(skills_dir).await;

        let strategy = SkillsStrategy::new(Some(system));

        let anchor = ContextAnchor::new(None);
        // GeneralChat instead of Intent::Skills
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Skills],
                ContextFormat::Markdown,
            )
            .user_input("Please improve this text".to_string())
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();

        // Progressive Disclosure: only metadata, no pre-loaded instructions
        assert!(result.context.skill_instructions.is_none());

        // available_skills is populated with metadata
        assert!(result.context.available_skills.is_some());
        let skills = result.context.available_skills.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id, "refine-text");
        assert_eq!(skills[0].description, "Improve and polish writing");
    }

    #[tokio::test]
    async fn test_skills_strategy_multiple_skills() {
        // All available skills should be listed in metadata
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(
            &skills_dir,
            "refine-text",
            "Improve and polish writing",
            "# Refine Text\n\nInstructions.",
        );

        create_test_skill(
            &skills_dir,
            "translate",
            "Translate text between languages",
            "# Translate\n\nInstructions.",
        );

        let system = make_system(skills_dir).await;

        let strategy = SkillsStrategy::new(Some(system));

        let anchor = ContextAnchor::new(None);
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Skills],
                ContextFormat::Markdown,
            )
            .user_input("Hello".to_string())
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();

        // Both skills should be in available_skills
        assert!(result.context.available_skills.is_some());
        let skills = result.context.available_skills.unwrap();
        assert_eq!(skills.len(), 2);

        // No skill_instructions (Progressive Disclosure)
        assert!(result.context.skill_instructions.is_none());
    }

    #[tokio::test]
    async fn test_skills_strategy_no_skills() {
        // Empty skills directory
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        let system = make_system(skills_dir).await;

        let strategy = SkillsStrategy::new(Some(system));

        let anchor = ContextAnchor::new(None);
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Skills],
                ContextFormat::Markdown,
            )
            .user_input("What is the weather today?".to_string())
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();

        // No skills available
        assert!(result.context.available_skills.is_none());
        assert!(result.context.skill_instructions.is_none());
    }

    #[tokio::test]
    async fn test_skills_strategy_empty_registry() {
        // With empty system, available_skills should be None
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        let system = make_system(skills_dir).await;

        let strategy = SkillsStrategy::new(Some(system));

        let anchor = ContextAnchor::new(None);
        let payload = PayloadBuilder::new()
            .meta(Intent::Skills("nonexistent".to_string()), 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Skills],
                ContextFormat::Markdown,
            )
            .user_input("Use nonexistent skill".to_string())
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();

        // No skills in system, so no available_skills
        assert!(result.context.available_skills.is_none());
        // No skill_instructions (agent uses read_skill tool, which will fail)
        assert!(result.context.skill_instructions.is_none());
    }
}
