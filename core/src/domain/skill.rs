//! Skill domain types for the Aleph skill system.
//!
//! Defines identity types (`SkillId`, `PluginId`), provenance (`SkillSource`),
//! value objects (eligibility, install, invocation specs), and the
//! `SkillManifest` aggregate root.

use serde::{Deserialize, Serialize};
use std::fmt;

use super::{Entity, AggregateRoot, ValueObject};

// ---------------------------------------------------------------------------
// SkillId
// ---------------------------------------------------------------------------

/// Unique identifier for a skill, following the convention `plugin::skill_name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillId(String);

impl SkillId {
    /// Create a new `SkillId` from any string-like value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Return the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the plugin prefix (part before `::`) if present.
    pub fn plugin_prefix(&self) -> Option<&str> {
        self.0.split_once("::").map(|(prefix, _)| prefix)
    }

    /// Return the skill name (part after `::`, or the whole id if no prefix).
    pub fn skill_name(&self) -> &str {
        self.0.split_once("::").map_or(self.0.as_str(), |(_, name)| name)
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for SkillId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for SkillId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

// ---------------------------------------------------------------------------
// PluginId
// ---------------------------------------------------------------------------

/// Unique identifier for a plugin that can provide skills.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginId(String);

impl PluginId {
    /// Create a new `PluginId`.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Return the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for PluginId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for PluginId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

// ---------------------------------------------------------------------------
// SkillSource
// ---------------------------------------------------------------------------

/// Where a skill originates from. Determines override priority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillSource {
    /// Shipped with the binary.
    Bundled,
    /// Installed in the global `~/.aleph/skills/` directory.
    Global,
    /// Defined in a workspace `.aleph/skills/` directory.
    Workspace,
    /// Provided by a plugin.
    Plugin(PluginId),
}

impl SkillSource {
    /// Priority for override resolution. Higher value wins.
    ///
    /// Bundled=1 < Global=2 < Plugin=3 < Workspace=4
    pub fn priority(&self) -> u8 {
        match self {
            Self::Bundled => 1,
            Self::Global => 2,
            Self::Plugin(_) => 3,
            Self::Workspace => 4,
        }
    }
}

impl ValueObject for SkillSource {}

#[cfg(test)]
mod tests {
    use super::*;

    // === Task 1 tests ===

    #[test]
    fn test_skill_id_display() {
        let id = SkillId::new("git::commit");
        assert_eq!(format!("{}", id), "git::commit");
        assert_eq!(id.plugin_prefix(), Some("git"));
        assert_eq!(id.skill_name(), "commit");
    }

    #[test]
    fn test_skill_id_equality() {
        let a = SkillId::new("git::commit");
        let b = SkillId::new("git::commit");
        let c = SkillId::new("git::push");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_skill_id_from_string() {
        let from_str: SkillId = "hello::world".into();
        let from_string: SkillId = String::from("hello::world").into();
        assert_eq!(from_str, from_string);

        // No prefix
        let bare = SkillId::new("standalone");
        assert_eq!(bare.plugin_prefix(), None);
        assert_eq!(bare.skill_name(), "standalone");
    }

    #[test]
    fn test_skill_source_priority() {
        assert_eq!(SkillSource::Bundled.priority(), 1);
        assert_eq!(SkillSource::Global.priority(), 2);
        assert_eq!(SkillSource::Plugin(PluginId::new("foo")).priority(), 3);
        assert_eq!(SkillSource::Workspace.priority(), 4);

        // Workspace should always beat Bundled
        assert!(SkillSource::Workspace.priority() > SkillSource::Bundled.priority());
    }
}
