//! Lightweight tool info for prompt building.

/// Lightweight tool info for prompt building.
///
/// (`usage_hint: Option<ToolUsageHint>` was removed 2026-07-26 together with
/// `ToolUsageGrammarLayer`: no production `ToolInfo` ever set it, so the layer
/// that rendered "prefer X over Y" lines had nothing to render.)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// Optional JSON Schema for tool parameters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_schema: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_info_omits_absent_schema() {
        let tool = ToolInfo {
            name: "test".into(),
            description: "test".into(),
            parameters_schema: None,
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(!json.contains("parameters_schema"));
    }
}
