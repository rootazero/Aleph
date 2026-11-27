//! Operator bindings for ephemeral thought translation.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThoughtTranslationConfig {
    /// Exact canonical MCP Tool name. The tool accepts text + target_language
    /// and returns one MCP text block containing JSON with a `text` string.
    #[serde(default)]
    pub machine_tool: Option<String>,
    /// Explicit provider/model key; no default-provider fallback is permitted.
    #[serde(default)]
    pub ai_model: Option<String>,
}
