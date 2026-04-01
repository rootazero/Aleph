//! Skill system events — broadcast notifications of skill lifecycle changes.

use serde::{Deserialize, Serialize};

/// Events emitted by the SkillSystem when skills change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillSystemEvent {
    /// A single skill was loaded or updated
    SkillLoaded {
        skill_id: String,
        skill_name: String,
    },
    /// A single skill was removed
    SkillRemoved { skill_id: String },
    /// All skills were reloaded (bulk operation)
    AllReloaded {
        count: usize,
        skill_ids: Vec<String>,
    },
}

impl SkillSystemEvent {
    pub fn loaded(skill_id: impl Into<String>, skill_name: impl Into<String>) -> Self {
        Self::SkillLoaded {
            skill_id: skill_id.into(),
            skill_name: skill_name.into(),
        }
    }
    pub fn removed(skill_id: impl Into<String>) -> Self {
        Self::SkillRemoved {
            skill_id: skill_id.into(),
        }
    }
    pub fn all_reloaded(count: usize, skill_ids: Vec<String>) -> Self {
        Self::AllReloaded { count, skill_ids }
    }
    pub fn skill_id(&self) -> Option<&str> {
        match self {
            Self::SkillLoaded { skill_id, .. } | Self::SkillRemoved { skill_id } => Some(skill_id),
            Self::AllReloaded { .. } => None,
        }
    }
    pub fn is_bulk_reload(&self) -> bool {
        matches!(self, Self::AllReloaded { .. })
    }
}
