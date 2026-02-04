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

    /// Get hint for a field path, supporting wildcard matching.
    ///
    /// Looks up hints in the following order:
    /// 1. Exact match on the full path
    /// 2. Wildcard patterns, with longer matches preferred
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use aethecore::config::ui_hints::ConfigUiHints;
    ///
    /// let hints = aethecore::config::ui_hints::build_ui_hints();
    ///
    /// // Exact match
    /// let hint = hints.get_hint("general.language");
    ///
    /// // Wildcard match: "providers.*.api_key" matches "providers.openai.api_key"
    /// let hint = hints.get_hint("providers.openai.api_key");
    /// ```
    pub fn get_hint(&self, path: &str) -> Option<&FieldHint> {
        // Try exact match first
        if let Some(hint) = self.fields.get(path) {
            return Some(hint);
        }

        // Try wildcard patterns (longest match first)
        let parts: Vec<&str> = path.split('.').collect();
        let mut best_match: Option<(&str, &FieldHint)> = None;

        for (pattern, hint) in &self.fields {
            if Self::matches_pattern(pattern, &parts)
                && (best_match.is_none() || pattern.len() > best_match.unwrap().0.len()) {
                    best_match = Some((pattern.as_str(), hint));
                }
        }

        best_match.map(|(_, hint)| hint)
    }

    /// Check if a pattern matches the path parts.
    ///
    /// Pattern segments can be:
    /// - `*`: matches any single segment
    /// - Any other string: must match exactly
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

    /// Merge another set of UI hints into this one.
    ///
    /// Fields and groups from `other` will override those in `self`.
    pub fn merge(&mut self, other: ConfigUiHints) {
        self.groups.extend(other.groups);
        self.fields.extend(other.fields);
    }

    /// Get all groups sorted by order.
    pub fn sorted_groups(&self) -> Vec<(&String, &GroupMeta)> {
        let mut groups: Vec<_> = self.groups.iter().collect();
        groups.sort_by_key(|(_, meta)| meta.order);
        groups
    }

    /// Get all field hints for a specific group, sorted by order.
    pub fn fields_in_group(&self, group_id: &str) -> Vec<(&String, &FieldHint)> {
        let mut fields: Vec<_> = self
            .fields
            .iter()
            .filter(|(_, hint)| hint.group.as_deref() == Some(group_id))
            .collect();
        fields.sort_by_key(|(_, hint)| hint.order.unwrap_or(i32::MAX));
        fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let mut hints = ConfigUiHints::new();
        hints.fields.insert(
            "general.language".to_string(),
            FieldHint {
                label: Some("Language".to_string()),
                ..Default::default()
            },
        );

        let hint = hints.get_hint("general.language");
        assert!(hint.is_some());
        assert_eq!(hint.unwrap().label, Some("Language".to_string()));
    }

    #[test]
    fn test_wildcard_match() {
        let mut hints = ConfigUiHints::new();
        hints.fields.insert(
            "providers.*.api_key".to_string(),
            FieldHint {
                label: Some("API Key".to_string()),
                sensitive: true,
                ..Default::default()
            },
        );

        let hint = hints.get_hint("providers.openai.api_key");
        assert!(hint.is_some());
        assert!(hint.unwrap().sensitive);

        let hint2 = hints.get_hint("providers.claude.api_key");
        assert!(hint2.is_some());
    }

    #[test]
    fn test_exact_beats_wildcard() {
        let mut hints = ConfigUiHints::new();
        hints.fields.insert(
            "providers.*.model".to_string(),
            FieldHint {
                label: Some("Model (generic)".to_string()),
                ..Default::default()
            },
        );
        hints.fields.insert(
            "providers.openai.model".to_string(),
            FieldHint {
                label: Some("Model (OpenAI)".to_string()),
                ..Default::default()
            },
        );

        let hint = hints.get_hint("providers.openai.model");
        assert_eq!(hint.unwrap().label, Some("Model (OpenAI)".to_string()));

        let hint2 = hints.get_hint("providers.claude.model");
        assert_eq!(hint2.unwrap().label, Some("Model (generic)".to_string()));
    }

    #[test]
    fn test_no_match() {
        let hints = ConfigUiHints::new();
        assert!(hints.get_hint("nonexistent.path").is_none());
    }

    #[test]
    fn test_merge() {
        let mut hints1 = ConfigUiHints::new();
        hints1.fields.insert(
            "field1".to_string(),
            FieldHint {
                label: Some("Field 1".to_string()),
                ..Default::default()
            },
        );

        let mut hints2 = ConfigUiHints::new();
        hints2.fields.insert(
            "field2".to_string(),
            FieldHint {
                label: Some("Field 2".to_string()),
                ..Default::default()
            },
        );

        hints1.merge(hints2);
        assert!(hints1.fields.contains_key("field1"));
        assert!(hints1.fields.contains_key("field2"));
    }

    #[test]
    fn test_sorted_groups() {
        let mut hints = ConfigUiHints::new();
        hints.groups.insert(
            "z_group".to_string(),
            GroupMeta {
                label: "Z Group".to_string(),
                order: 10,
                icon: None,
            },
        );
        hints.groups.insert(
            "a_group".to_string(),
            GroupMeta {
                label: "A Group".to_string(),
                order: 5,
                icon: None,
            },
        );

        let sorted = hints.sorted_groups();
        assert_eq!(sorted[0].0, "a_group");
        assert_eq!(sorted[1].0, "z_group");
    }

    #[test]
    fn test_fields_in_group() {
        let mut hints = ConfigUiHints::new();
        hints.fields.insert(
            "field1".to_string(),
            FieldHint {
                group: Some("group_a".to_string()),
                order: Some(2),
                ..Default::default()
            },
        );
        hints.fields.insert(
            "field2".to_string(),
            FieldHint {
                group: Some("group_a".to_string()),
                order: Some(1),
                ..Default::default()
            },
        );
        hints.fields.insert(
            "field3".to_string(),
            FieldHint {
                group: Some("group_b".to_string()),
                ..Default::default()
            },
        );

        let group_a_fields = hints.fields_in_group("group_a");
        assert_eq!(group_a_fields.len(), 2);
        assert_eq!(group_a_fields[0].0, "field2"); // order 1 comes first
        assert_eq!(group_a_fields[1].0, "field1"); // order 2 comes second
    }
}
