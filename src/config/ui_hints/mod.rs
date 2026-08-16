//! UI Hints types for configuration field metadata.
//!
//! The producer half (the `build_ui_hints()` builder and all 686 lines of
//! declarative field-path literals in `definitions.rs`/`macros.rs`) was found
//! to have zero consumers in the 2026-08-12 static audit: the CLI is the only
//! production caller of `config.schema` and discards the `ui_hints` field, and
//! the Panel frontend never calls `config.schema` at all. The DTO is retained
//! so the wire shape (`config.schema.ui_hints`) stays stable for any future
//! schema-driven settings form, but every field is empty until such a consumer
//! is actually wired.

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
