//! Lightweight tool info for prompt building.

/// Hint for tool usage grammar generation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolUsageHint {
    /// Scenarios where this tool should be preferred
    pub prefer_for: Vec<String>,
    /// Alternative tools/commands this tool supersedes
    pub prefer_over: Vec<String>,
}

/// Lightweight tool info for prompt building.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// Optional JSON Schema for tool parameters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_schema: Option<serde_json::Value>,
    /// Optional usage hint for grammar layer generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_hint: Option<ToolUsageHint>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_info_with_usage_hint() {
        let tool = ToolInfo {
            name: "file_read".into(),
            description: "Read a file".into(),
            parameters_schema: None,
            usage_hint: Some(ToolUsageHint {
                prefer_for: vec!["reading file contents".into()],
                prefer_over: vec!["cat".into(), "head".into(), "tail".into()],
            }),
        };
        assert_eq!(tool.usage_hint.as_ref().unwrap().prefer_over.len(), 3);
    }

    #[test]
    fn tool_info_without_hint_serializes_cleanly() {
        let tool = ToolInfo {
            name: "test".into(),
            description: "test".into(),
            parameters_schema: None,
            usage_hint: None,
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(!json.contains("usage_hint"));
    }
}
