//! Skill Registry Events
//!
//! Event types for broadcasting changes in the SkillsRegistry.
//! These events enable the ToolIndexCoordinator to keep the tool
//! index synchronized with skill changes.

use serde::{Deserialize, Serialize};

/// Events emitted by the SkillsRegistry
///
/// These events are broadcast via tokio::sync::broadcast channels
/// to notify interested parties of skill lifecycle changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillRegistryEvent {
    /// A single skill was loaded or updated
    SkillLoaded {
        /// The skill ID that was loaded
        skill_id: String,
        /// The skill name
        skill_name: String,
    },

    /// A single skill was removed
    SkillRemoved {
        /// The skill ID that was removed
        skill_id: String,
    },

    /// All skills were reloaded (bulk operation)
    AllReloaded {
        /// Number of skills after reload
        count: usize,
        /// List of skill IDs that were loaded
        skill_ids: Vec<String>,
    },
}

impl SkillRegistryEvent {
    /// Create a SkillLoaded event
    pub fn loaded(skill_id: impl Into<String>, skill_name: impl Into<String>) -> Self {
        Self::SkillLoaded {
            skill_id: skill_id.into(),
            skill_name: skill_name.into(),
        }
    }

    /// Create a SkillRemoved event
    pub fn removed(skill_id: impl Into<String>) -> Self {
        Self::SkillRemoved {
            skill_id: skill_id.into(),
        }
    }

    /// Create an AllReloaded event
    pub fn all_reloaded(count: usize, skill_ids: Vec<String>) -> Self {
        Self::AllReloaded { count, skill_ids }
    }

    /// Get the skill ID if this is a single-skill event
    pub fn skill_id(&self) -> Option<&str> {
        match self {
            Self::SkillLoaded { skill_id, .. } => Some(skill_id),
            Self::SkillRemoved { skill_id } => Some(skill_id),
            Self::AllReloaded { .. } => None,
        }
    }

    /// Check if this is a bulk reload event
    pub fn is_bulk_reload(&self) -> bool {
        matches!(self, Self::AllReloaded { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_loaded_event() {
        let event = SkillRegistryEvent::loaded("refine-text", "Refine Text");

        assert!(matches!(
            &event,
            SkillRegistryEvent::SkillLoaded { skill_id, skill_name }
            if skill_id == "refine-text" && skill_name == "Refine Text"
        ));
        assert_eq!(event.skill_id(), Some("refine-text"));
        assert!(!event.is_bulk_reload());
    }

    #[test]
    fn test_skill_removed_event() {
        let event = SkillRegistryEvent::removed("old-skill");

        assert!(matches!(
            &event,
            SkillRegistryEvent::SkillRemoved { skill_id }
            if skill_id == "old-skill"
        ));
        assert_eq!(event.skill_id(), Some("old-skill"));
        assert!(!event.is_bulk_reload());
    }

    #[test]
    fn test_all_reloaded_event() {
        let event = SkillRegistryEvent::all_reloaded(
            3,
            vec!["skill-a".to_string(), "skill-b".to_string(), "skill-c".to_string()],
        );

        assert!(matches!(
            &event,
            SkillRegistryEvent::AllReloaded { count: 3, skill_ids }
            if skill_ids.len() == 3
        ));
        assert_eq!(event.skill_id(), None);
        assert!(event.is_bulk_reload());
    }

    #[test]
    fn test_event_serialization() {
        let event = SkillRegistryEvent::loaded("test", "Test Skill");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"skill_loaded\""));
        assert!(json.contains("\"skill_id\":\"test\""));

        let deserialized: SkillRegistryEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.skill_id(), Some("test"));
    }
}
