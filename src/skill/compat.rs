//! Compatibility types — bridges between v2 skill system and legacy consumers.

use crate::domain::skill::SkillManifest;
use crate::domain::Entity;
use serde::{Deserialize, Serialize};

/// Simplified skill info for tool registry and UI display.
/// Mirrors the legacy `crate::skills::SkillInfo` for backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

impl From<SkillManifest> for SkillInfo {
    fn from(manifest: SkillManifest) -> Self {
        Self {
            id: manifest.id().as_str().to_string(),
            name: manifest.name().to_string(),
            description: manifest.description().to_string(),
        }
    }
}

impl From<&SkillManifest> for SkillInfo {
    fn from(manifest: &SkillManifest) -> Self {
        Self::from(manifest.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{SkillContent, SkillSource};

    #[test]
    fn skill_info_from_manifest() {
        let manifest = SkillManifest::new(
            "test:skill",
            "Test Skill",
            "A test",
            SkillContent::new("c"),
            SkillSource::Bundled,
        );
        let info: SkillInfo = manifest.into();
        assert_eq!(info.id, "test:skill");
        assert_eq!(info.name, "Test Skill");
    }
}
