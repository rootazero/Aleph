use crate::error::{AlephError, Result};
use crate::exec::sandbox::{
    capabilities::Capabilities,
    presets::PresetRegistry,
    capability_resolver::{apply_overrides, bind_parameters},
};
use super::tool_generator::GeneratedToolDefinition;

/// Resolve final capabilities for a tool execution
pub fn resolve_tool_capabilities(
    tool_def: &GeneratedToolDefinition,
    parameters: &serde_json::Value,
) -> Result<Capabilities> {
    // Get required capabilities
    let required_caps = tool_def.required_capabilities.as_ref().ok_or_else(|| {
        AlephError::InvalidConfig {
            message: "Tool has no required_capabilities".to_string(),
            suggestion: Some("Add required_capabilities to tool definition".to_string()),
        }
    })?;

    // Load preset
    let registry = PresetRegistry::default();
    let preset = registry.get(&required_caps.base_preset).ok_or_else(|| {
        AlephError::InvalidConfig {
            message: format!("Unknown preset: {}", required_caps.base_preset),
            suggestion: Some("Use a valid preset name".to_string()),
        }
    })?;

    // Start with preset capabilities
    let mut caps = preset.capabilities.clone();

    // Apply overrides
    caps = apply_overrides(caps, &required_caps.overrides, &preset.immutable_fields)?;

    // Bind parameters
    bind_parameters(&mut caps, &required_caps.parameter_bindings, parameters)?;

    Ok(caps)
}

/// Infer appropriate preset from tool purpose
pub fn infer_preset_from_purpose(purpose: &str) -> String {
    let purpose_lower = purpose.to_lowercase();

    if purpose_lower.contains("web") || purpose_lower.contains("http") || purpose_lower.contains("scrape") {
        "web_scraper".to_string()
    } else if purpose_lower.contains("code") || purpose_lower.contains("analyze") || purpose_lower.contains("lint") {
        "code_analyzer".to_string()
    } else if purpose_lower.contains("data") || purpose_lower.contains("transform") || purpose_lower.contains("convert") {
        "data_transformer".to_string()
    } else {
        "file_processor".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::sandbox::parameter_binding::RequiredCapabilities;
    use crate::skill_evolution::tool_generator::GenerationMetadata;

    #[test]
    fn test_resolve_tool_capabilities() {
        let tool_def = GeneratedToolDefinition {
            name: "test_tool".to_string(),
            description: "Test".to_string(),
            input_schema: serde_json::json!({}),
            runtime: "python".to_string(),
            entrypoint: "entrypoint.py".to_string(),
            self_tested: false,
            requires_confirmation: true,
            required_capabilities: Some(RequiredCapabilities {
                base_preset: "file_processor".to_string(),
                description: "Test".to_string(),
                overrides: Default::default(),
                parameter_bindings: Default::default(),
            }),
            approval_metadata: None,
            success_manifest: None,
            generated: GenerationMetadata {
                pattern_id: "test".to_string(),
                confidence: 0.9,
                generated_at: 0,
                generator_version: "1.0".to_string(),
            },
        };

        let params = serde_json::json!({});
        let result = resolve_tool_capabilities(&tool_def, &params);
        assert!(result.is_ok());
    }

    #[test]
    fn test_infer_preset_from_purpose() {
        assert_eq!(infer_preset_from_purpose("web scraper"), "web_scraper");
        assert_eq!(infer_preset_from_purpose("code analyzer"), "code_analyzer");
        assert_eq!(infer_preset_from_purpose("data transformer"), "data_transformer");
        assert_eq!(infer_preset_from_purpose("file processor"), "file_processor");
    }
}
