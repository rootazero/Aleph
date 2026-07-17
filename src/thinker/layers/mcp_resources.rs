//! `McpResourceIndexLayer` — indexes available MCP resources & prompts (priority 1704)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

/// Injects a compact catalog of the resources and prompt templates advertised by
/// connected MCP servers, sitting right after [`super::McpInstructionsLayer`]
/// (which carries only free-text server guidance). Without this index the model
/// cannot discover which `mcp_read_resource` URIs / `mcp_get_prompt` names exist,
/// so it falls back to `cat`/`file_read` on guessed `file://` paths — the exact
/// anti-pattern this closes, mirroring how `<available_skills>` makes `skill_read`
/// the obvious route.
///
/// The section is pre-rendered in the harness bridge (`prompt_build.rs`) from the
/// already-existing `aggregate_resources()`/`aggregate_prompts()` handle methods
/// via [`crate::mcp::format_mcp_resource_index`], and threaded through
/// `PromptConfig.mcp_resource_index` — the same owned-`String` pattern as
/// `runtime_capabilities`. Pure data wiring, no cognition (R10-safe).
pub struct McpResourceIndexLayer;

impl PromptLayer for McpResourceIndexLayer {
    fn name(&self) -> &'static str {
        "mcp_resources"
    }
    fn priority(&self) -> u32 {
        1704
    }
    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(index) = input
            .config
            .mcp_resource_index
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            output.push_str(index);
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn injects_when_index_present() {
        let layer = McpResourceIndexLayer;
        let config = PromptConfig {
            mcp_resource_index: Some(
                "## Available MCP Resources & Prompts\n\n- `fs:file:///a`".to_string(),
            ),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("Available MCP Resources & Prompts"));
        assert!(out.contains("`fs:file:///a`"));
    }

    #[test]
    fn skips_when_absent() {
        let layer = McpResourceIndexLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn skips_when_empty_string() {
        let layer = McpResourceIndexLayer;
        let config = PromptConfig {
            mcp_resource_index: Some(String::new()),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn stability_is_dynamic() {
        assert_eq!(McpResourceIndexLayer.stability(), LayerStability::Dynamic);
    }
}
