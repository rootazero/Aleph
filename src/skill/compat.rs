//! Compatibility types — bridges between v2 skill system and legacy consumers.

use crate::domain::skill::{PromptScope, SkillManifest};
use crate::domain::Entity;
use serde::{Deserialize, Serialize};

/// Simplified skill info for tool registry and UI display.
/// Mirrors the legacy `crate::skills::SkillInfo` for backward compatibility.
///
/// This is a *projection*, not the model: `scope` and `version` were added
/// (I-4) because downstream callers needed at least the skill's prompt
/// routing and declared version to make display / registration decisions —
/// the previous three-field shape forced every consumer to re-derive scope
/// from nothing. Fuller details (eligibility, install specs) stay on
/// [`SkillManifest`]; callers that need them should hold the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    /// How the skill's content is injected into prompts. Serialized via
    /// `PromptScope`'s lowercase serde names (`system` / `tool` /
    /// `standalone` / `disabled`).
    pub scope: PromptScope,
    /// Frontmatter-declared skill version, when the author declares one.
    /// Absent for skills without a `version:` key and for plugin *commands*
    /// (which are registered through this same shape but carry no manifest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl From<SkillManifest> for SkillInfo {
    fn from(manifest: SkillManifest) -> Self {
        Self {
            id: manifest.id().as_str().to_string(),
            name: manifest.name().to_string(),
            description: manifest.description().to_string(),
            scope: manifest.scope().clone(),
            version: manifest.version().map(str::to_string),
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
        let mut manifest = SkillManifest::new(
            "test:skill",
            "Test Skill",
            "A test",
            SkillContent::new("c"),
            SkillSource::Bundled,
        );
        manifest.set_scope(PromptScope::Tool);
        manifest.set_version("1.2.3".to_string());
        let info: SkillInfo = manifest.into();
        assert_eq!(info.id, "test:skill");
        assert_eq!(info.name, "Test Skill");
        assert_eq!(info.scope, PromptScope::Tool);
        assert_eq!(info.version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn skill_info_defaults_when_manifest_declares_nothing() {
        let manifest = SkillManifest::new(
            "test:plain",
            "Plain",
            "No extras",
            SkillContent::new("c"),
            SkillSource::Global,
        );
        let info: SkillInfo = manifest.into();
        assert_eq!(info.scope, PromptScope::System);
        assert!(info.version.is_none());
    }
}
