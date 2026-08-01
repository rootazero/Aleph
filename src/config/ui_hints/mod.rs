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
