//! Tool Definition Types
//!
//! Contains tool definition for LLM function calling.

use super::category::ToolCategory;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tool definition for LLM function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub requires_confirmation: bool,
    pub category: ToolCategory,
    #[serde(default)]
    pub strict: bool,
}

impl ToolDefinition {
    #[allow(deprecated)]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        category: ToolCategory,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            requires_confirmation: false,
            category,
            strict: false,
        }
    }

    #[must_use]
    pub const fn with_confirmation(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    #[must_use]
    pub const fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_builders_round_trip() {
        let def = ToolDefinition::new(
            "search",
            "Search the web",
            serde_json::json!({"type": "object"}),
            ToolCategory::Builtin,
        )
        .with_confirmation(true)
        .with_strict(true);
        assert_eq!(def.name, "search");
        assert!(def.requires_confirmation);
        assert!(def.strict);
    }
}
