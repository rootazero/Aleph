//! Lightweight tool info for prompt building.

/// Lightweight tool info for prompt building.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// Optional JSON Schema for tool parameters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_schema: Option<serde_json::Value>,
}
