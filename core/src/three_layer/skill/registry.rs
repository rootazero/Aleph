//! Skill Registry - manages available skills

use super::SkillDefinition;
use crate::three_layer::safety::Capability;
use std::collections::HashMap;

/// Registry of available skills
#[derive(Debug, Default)]
pub struct SkillRegistry {
    /// Builtin skills (Rust implementations)
    builtin: HashMap<String, SkillDefinition>,
    /// Custom skills (loaded from YAML)
    custom: HashMap<String, SkillDefinition>,
}

impl SkillRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a skill
    pub fn register(&mut self, skill: SkillDefinition) {
        self.builtin.insert(skill.id.clone(), skill);
    }

    /// Register a custom skill (from YAML)
    pub fn register_custom(&mut self, skill: SkillDefinition) {
        self.custom.insert(skill.id.clone(), skill);
    }

    /// Get a skill by ID
    pub fn get(&self, id: &str) -> Option<&SkillDefinition> {
        self.builtin.get(id).or_else(|| self.custom.get(id))
    }

    /// List all skills that require a specific capability
    pub fn list_by_capability(&self, capability: &Capability) -> Vec<&SkillDefinition> {
        self.builtin
            .values()
            .chain(self.custom.values())
            .filter(|s| s.required_capabilities.contains(capability))
            .collect()
    }

    /// List all registered skills
    pub fn list_all(&self) -> Vec<&SkillDefinition> {
        self.builtin.values().chain(self.custom.values()).collect()
    }

    /// Clear custom skills (for hot reload)
    pub fn clear_custom(&mut self) {
        self.custom.clear();
    }

    /// Check if a skill exists
    pub fn contains(&self, id: &str) -> bool {
        self.builtin.contains_key(id) || self.custom.contains_key(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = SkillRegistry::new();

        let skill = SkillDefinition::new(
            "test".to_string(),
            "Test Skill".to_string(),
            "A test skill".to_string(),
        );

        registry.register(skill);

        assert!(registry.get("test").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_list_by_capability() {
        let mut registry = SkillRegistry::new();

        let skill1 = SkillDefinition::new("s1".to_string(), "S1".to_string(), "".to_string())
            .with_capabilities(vec![Capability::FileRead]);

        let skill2 = SkillDefinition::new("s2".to_string(), "S2".to_string(), "".to_string())
            .with_capabilities(vec![Capability::FileRead, Capability::WebSearch]);

        let skill3 = SkillDefinition::new("s3".to_string(), "S3".to_string(), "".to_string())
            .with_capabilities(vec![Capability::WebSearch]);

        registry.register(skill1);
        registry.register(skill2);
        registry.register(skill3);

        let file_skills = registry.list_by_capability(&Capability::FileRead);
        assert_eq!(file_skills.len(), 2);

        let web_skills = registry.list_by_capability(&Capability::WebSearch);
        assert_eq!(web_skills.len(), 2);
    }

    #[test]
    fn test_registry_list_all() {
        let mut registry = SkillRegistry::new();

        registry.register(SkillDefinition::new(
            "a".to_string(),
            "A".to_string(),
            "".to_string(),
        ));
        registry.register(SkillDefinition::new(
            "b".to_string(),
            "B".to_string(),
            "".to_string(),
        ));

        assert_eq!(registry.list_all().len(), 2);
    }
}
