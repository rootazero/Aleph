//! Prompt configuration types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for extra files injected into system prompt.
///
/// Equivalent to registering an `ExtraFilesHook` without writing code.
///
/// ```toml
/// [prompt.extra_files]
/// enabled = true
/// paths = ["docs/API.md", "docs/ARCHITECTURE.md"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PromptExtraFilesConfig {
    /// Whether extra files injection is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Paths to extra files (relative to workspace directory).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// Top-level prompt configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PromptSectionConfig {
    /// Extra files to inject into the system prompt.
    #[serde(default)]
    pub extra_files: PromptExtraFilesConfig,
}
