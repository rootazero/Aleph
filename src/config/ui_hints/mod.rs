//! UI Hints system for configuration field metadata.
//!
//! This module provides a system for attaching UI-related metadata to configuration fields,
//! including labels, help text, grouping, ordering, and sensitivity flags. This metadata
//! enables UI components to render configuration forms with proper context and organization.

mod definitions;
mod macros;

pub use definitions::build_ui_hints;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metadata for a configuration group.
///
/// Groups organize related configuration fields together in the UI.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GroupMeta {
    /// Display label for the group.
    pub label: String,
    /// Sort order (lower = higher priority).
    pub order: i32,
    /// Optional icon identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Hint metadata for a single configuration field.
///
/// Provides UI-related information for rendering configuration fields,
/// including labels, help text, sensitivity flags, and grouping.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct FieldHint {
    /// Human-readable label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Help text / tooltip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Group this field belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Sort order within group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    /// Whether this is an advanced option (hidden by default).
    #[serde(default)]
    pub advanced: bool,
    /// Whether this field contains sensitive data.
    #[serde(default)]
    pub sensitive: bool,
    /// Placeholder text for input fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

/// Complete UI hints for configuration rendering.
///
/// Contains both group definitions and field-level hints. Supports wildcard
/// matching for field paths using `*` as a path segment placeholder.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ConfigUiHints {
    /// Group definitions: id -> metadata.
    pub groups: HashMap<String, GroupMeta>,
    /// Field hints: path -> hint.
    pub fields: HashMap<String, FieldHint>,
}

impl ConfigUiHints {
    /// Create empty UI hints.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Test-only read accessors.
///
/// `build_ui_hints()` is shipped to the Panel wholesale (`gateway/handlers/
/// config.rs`), which does its own field lookup — so production never needed
/// these, and `6e63cabe2` correctly cut them from the production surface.
/// That cut left their tests behind, which broke the whole `alephcore` lib-test
/// build; they are restored here under `#[cfg(test)]` so the wildcard-matching
/// contract stays covered without re-exposing an unused public API.
#[cfg(test)]
impl ConfigUiHints {
    /// Look up a field hint, falling back to wildcard patterns (`*` matches one
    /// path segment). Longest matching pattern wins.
    pub(crate) fn get_hint(&self, path: &str) -> Option<&FieldHint> {
        if let Some(hint) = self.fields.get(path) {
            return Some(hint);
        }

        let parts: Vec<&str> = path.split('.').collect();
        let mut best_match: Option<(&str, &FieldHint)> = None;

        for (pattern, hint) in &self.fields {
            if Self::matches_pattern(pattern, &parts)
                && best_match.is_none_or(|(best, _)| pattern.len() > best.len())
            {
                best_match = Some((pattern.as_str(), hint));
            }
        }

        best_match.map(|(_, hint)| hint)
    }

    /// Whether `pattern` (with `*` segment wildcards) matches the split path.
    fn matches_pattern(pattern: &str, path_parts: &[&str]) -> bool {
        let pattern_parts: Vec<&str> = pattern.split('.').collect();
        if pattern_parts.len() != path_parts.len() {
            return false;
        }

        pattern_parts
            .iter()
            .zip(path_parts.iter())
            .all(|(p, t)| *p == "*" || p == t)
    }

    /// All groups sorted by their declared display order.
    pub(crate) fn sorted_groups(&self) -> Vec<(&String, &GroupMeta)> {
        let mut groups: Vec<_> = self.groups.iter().collect();
        groups.sort_by_key(|(_, meta)| meta.order);
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_hints() {
        let hints = ConfigUiHints::new();
        assert!(hints.groups.is_empty());
        assert!(hints.fields.is_empty());
    }
}
