//! Skills capability strategy.
//!
//! This strategy injects skill instructions from matched SKILL.md files
//! into the payload context for prompt assembly.

use crate::capability::strategy::CapabilityStrategy;
use crate::error::Result;
use crate::payload::{AgentPayload, Capability};
use crate::skills::SkillsRegistry;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Skills capability strategy
///
/// Matches user input against loaded skills and injects instructions
/// from the matched SKILL.md into the payload context.
pub struct SkillsStrategy {
    /// Skills registry containing loaded skills
    skills_registry: Option<Arc<SkillsRegistry>>,
}

impl SkillsStrategy {
    /// Create a new skills strategy
    pub fn new(skills_registry: Option<Arc<SkillsRegistry>>) -> Self {
        Self { skills_registry }
    }

    /// Update the skills registry
    pub fn set_registry(&mut self, registry: Arc<SkillsRegistry>) {
        self.skills_registry = Some(registry);
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
        // Available if registry is configured
        self.skills_registry.is_some()
    }

    fn validate_config(&self) -> Result<()> {
        // Validate skills directory exists if registry is configured
        if let Some(registry) = &self.skills_registry {
            let dir = registry.skills_dir();
            if !dir.exists() {
                return Err(crate::error::AetherError::config(format!(
                    "Skills directory does not exist: {}",
                    dir.display()
                )));
            }
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<bool> {
        if !self.is_available() {
            return Ok(false);
        }

        // Check if registry has any skills loaded
        if let Some(registry) = &self.skills_registry {
            let count = registry.count();
            debug!(skill_count = count, "Skills health check: skills loaded");
            // Skills is healthy as long as registry exists
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
            "has_registry".to_string(),
            self.skills_registry.is_some().to_string(),
        );
        if let Some(registry) = &self.skills_registry {
            info.insert("skill_count".to_string(), registry.count().to_string());
            info.insert(
                "skills_dir".to_string(),
                registry.skills_dir().display().to_string(),
            );
        }
        info
    }

    async fn execute(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        let Some(registry) = &self.skills_registry else {
            warn!("Skills capability requested but no registry configured");
            return Ok(payload);
        };

        // Check if a skill is explicitly specified via Intent::Skills
        let skill_id = payload.meta.intent.skills_id();

        // If skill_id is specified, look up directly
        if let Some(id) = skill_id {
            if let Some(skill) = registry.get_skill(id) {
                info!(
                    skill_id = %id,
                    skill_name = %skill.name(),
                    "Injecting skill instructions from explicit /skill command"
                );
                payload.context.skill_instructions = Some(skill.instructions.clone());
                return Ok(payload);
            } else {
                warn!(
                    skill_id = %id,
                    "Skill not found in registry"
                );
                return Ok(payload);
            }
        }

        // Otherwise, try to auto-match skill based on user input
        if let Some(skill) = registry.find_matching(&payload.user_input) {
            info!(
                skill_id = %skill.id,
                skill_name = %skill.name(),
                "Auto-matched skill based on user input"
            );
            payload.context.skill_instructions = Some(skill.instructions.clone());
        } else {
            debug!("No skill matched for user input");
        }

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
        dir: &std::path::PathBuf,
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

    #[tokio::test]
    async fn test_skills_strategy_available() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();
        let registry = Arc::new(SkillsRegistry::new(skills_dir));
        registry.load_all().unwrap();

        let strategy = SkillsStrategy::new(Some(registry));
        assert!(strategy.is_available());
    }

    #[tokio::test]
    async fn test_skills_strategy_not_available() {
        let strategy = SkillsStrategy::new(None);
        assert!(!strategy.is_available());
    }

    #[tokio::test]
    async fn test_skills_strategy_explicit_skill() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(
            &skills_dir,
            "refine-text",
            "Improve and polish writing",
            "# Refine Text\n\nWhen refining text, focus on clarity and conciseness.",
        );

        let registry = Arc::new(SkillsRegistry::new(skills_dir));
        registry.load_all().unwrap();

        let strategy = SkillsStrategy::new(Some(registry));

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
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
        assert!(result.context.skill_instructions.is_some());
        let instructions = result.context.skill_instructions.unwrap();
        assert!(instructions.contains("Refine Text"));
        assert!(instructions.contains("clarity and conciseness"));
    }

    #[tokio::test]
    async fn test_skills_strategy_auto_match() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(
            &skills_dir,
            "refine-text",
            "Improve and polish writing",
            "# Refine Text\n\nWhen refining text, focus on clarity.",
        );

        let registry = Arc::new(SkillsRegistry::new(skills_dir));
        registry.load_all().unwrap();

        let strategy = SkillsStrategy::new(Some(registry));

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
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
        assert!(result.context.skill_instructions.is_some());
    }

    #[tokio::test]
    async fn test_skills_strategy_no_match() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(
            &skills_dir,
            "refine-text",
            "Improve and polish writing",
            "# Refine Text\n\nInstructions here.",
        );

        let registry = Arc::new(SkillsRegistry::new(skills_dir));
        registry.load_all().unwrap();

        let strategy = SkillsStrategy::new(Some(registry));

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
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
        assert!(result.context.skill_instructions.is_none());
    }

    #[tokio::test]
    async fn test_skills_strategy_skill_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        let registry = Arc::new(SkillsRegistry::new(skills_dir));
        registry.load_all().unwrap();

        let strategy = SkillsStrategy::new(Some(registry));

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
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
        assert!(result.context.skill_instructions.is_none());
    }
}
